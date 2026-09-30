use std::path::Path;
use std::process::ExitCode;

use sqlx::PgPool;

// Eksik ortam degiskeninde None doner, hicbir sey yazmaz; cagiran kendi
// baglamiyla mesaji kendi basar (run()'un uzunlugunu duz tutar, security.md ≤50 satir).
fn require_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub async fn run() -> ExitCode {
    let Some(database_url) = require_env("DATABASE_URL") else {
        eprintln!("migrate: ortam değişkeni eksik: DATABASE_URL");
        return ExitCode::FAILURE;
    };

    let pool = match crate::db::connect_pool(&database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("migrate: veritabanına bağlanılamadı: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = prepare_roles(&pool).await {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }

    let migrator = match sqlx::migrate::Migrator::new(Path::new("./migrations")).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("migrate: migrations dizini okunamadı: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = migrator.run(&pool).await {
        eprintln!("migrate: şema migration'ı başarısız: {e}");
        return ExitCode::FAILURE;
    }

    if let Err(e) = seed_bootstrap_account(&pool).await {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }

    // backend ve worker acilista _sqlx_migrations'a bakarak semanin hazir
    // olup olmadigini kontrol eder (ADR-061 madde 3, crate::db::check_schema_ready)
    for user_var in ["POSTGRES_BACKEND_USER", "POSTGRES_WORKER_USER"] {
        let Some(user) = require_env(user_var) else {
            eprintln!(
                "migrate: migration tablosu izni verilemedi: ortam değişkeni eksik: {user_var}"
            );
            return ExitCode::FAILURE;
        };
        if let Err(e) = grant_migrations_read(&pool, &user).await {
            eprintln!("migrate: migration tablosu izni verilemedi ({user_var}): {e}");
            return ExitCode::FAILURE;
        }
    }

    println!("migrate: tamamlandı");
    ExitCode::SUCCESS
}

// ADR-068: admin/admin parolasi SQL migration'a duz metin gomulmez, hash burada
// hesaplanir; hesap zaten varsa (ikinci calistirmada) dokunulmaz.
pub(crate) async fn seed_bootstrap_account(pool: &PgPool) -> Result<(), String> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT FROM bootstrap_account WHERE id = TRUE)")
            .fetch_one(pool)
            .await
            .map_err(|e| format!("migrate: bootstrap hesabı kontrol edilemedi: {e}"))?;
    if exists {
        return Ok(());
    }

    let password_hash = crate::auth::hash_password("admin")
        .map_err(|e| format!("migrate: bootstrap parolası hash'lenemedi: {e}"))?;
    sqlx::query(
        "INSERT INTO bootstrap_account (id, username, password_hash, must_change_password) \
         VALUES (TRUE, 'admin', $1, TRUE)",
    )
    .bind(password_hash)
    .execute(pool)
    .await
    .map_err(|e| format!("migrate: bootstrap hesabı seed edilemedi: {e}"))?;
    Ok(())
}

async fn prepare_roles(pool: &PgPool) -> Result<(), String> {
    for (user_var, pass_var) in [
        ("POSTGRES_BACKEND_USER", "POSTGRES_BACKEND_PASSWORD"),
        ("POSTGRES_WORKER_USER", "POSTGRES_WORKER_PASSWORD"),
    ] {
        let Some(user) = require_env(user_var) else {
            return Err(format!(
                "migrate: rol hazırlanamadı: ortam değişkeni eksik: {user_var}"
            ));
        };
        let Some(pass) = require_env(pass_var) else {
            return Err(format!(
                "migrate: rol hazırlanamadı: ortam değişkeni eksik: {pass_var}"
            ));
        };
        ensure_role(pool, &user, &pass)
            .await
            .map_err(|e| format!("migrate: rol hazırlanamadı ({user_var}): {e}"))?;
    }
    Ok(())
}

#[derive(Debug)]
enum RoleError {
    InvalidUsername(String),
    Db(sqlx::Error),
}

impl std::fmt::Display for RoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleError::InvalidUsername(name) => write!(f, "geçersiz kullanıcı adı: {name}"),
            RoleError::Db(e) => write!(f, "veritabanı hatası: {e}"),
        }
    }
}

// Postgres CREATE/ALTER ROLE parametreli sorguyu desteklemez (DDL); kullanıcı adı
// bu desenle sınırlanır ve parola tek tırnak kaçışıyla eklenir (ADR-015).
fn validate_role_name(name: &str) -> Result<(), RoleError> {
    let first_ok = name
        .chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false);
    let rest_ok = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if first_ok && rest_ok && name.len() <= 63 {
        Ok(())
    } else {
        Err(RoleError::InvalidUsername(name.to_string()))
    }
}

async fn ensure_role(pool: &PgPool, user: &str, pass: &str) -> Result<(), RoleError> {
    validate_role_name(user)?;
    let escaped_pass = pass.replace('\'', "''");

    let stmt = format!(
        r#"
        DO $opensicil_role$
        BEGIN
          IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{user}') THEN
            EXECUTE format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOCREATEDB NOCREATEROLE', '{user}', '{escaped_pass}');
          ELSE
            EXECUTE format('ALTER ROLE %I LOGIN PASSWORD %L', '{user}', '{escaped_pass}');
          END IF;
          EXECUTE format('GRANT CONNECT ON DATABASE %I TO %I', current_database(), '{user}');
        END
        $opensicil_role$;
        "#
    );

    // Denetlendi: kullanıcı adı validate_role_name ile sınırlandı, parola tek
    // tırnak kaçışıyla eklendi; Postgres CREATE/ALTER ROLE bind parametresi
    // desteklemez (ADR-015).
    sqlx::query(sqlx::AssertSqlSafe(stmt))
        .execute(pool)
        .await
        .map_err(RoleError::Db)?;
    Ok(())
}

async fn grant_migrations_read(pool: &PgPool, user: &str) -> Result<(), RoleError> {
    validate_role_name(user)?;

    // Denetlendi: kullanici adi validate_role_name ile sinirlandi, bind parametresi
    // GRANT'te desteklenmez (ensure_role'daki gibi).
    let stmt = format!("GRANT SELECT ON _sqlx_migrations TO {user}");
    sqlx::query(sqlx::AssertSqlSafe(stmt))
        .execute(pool)
        .await
        .map_err(RoleError::Db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_identifier() {
        assert!(validate_role_name("opensicil_backend").is_ok());
    }

    #[test]
    fn rejects_names_starting_with_digit() {
        assert!(validate_role_name("1backend").is_err());
    }

    #[test]
    fn rejects_quote_injection_attempt() {
        assert!(validate_role_name("a'; DROP ROLE postgres; --").is_err());
    }

    #[test]
    fn rejects_empty_name() {
        assert!(validate_role_name("").is_err());
    }

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    #[tokio::test]
    async fn ensure_role_rejects_invalid_username_before_querying() {
        let pool = lazy_unreachable_pool();
        let result = ensure_role(&pool, "1invalid", "pw").await;
        assert!(matches!(result, Err(RoleError::InvalidUsername(_))));
    }

    #[tokio::test]
    async fn grant_migrations_read_rejects_invalid_username_before_querying() {
        let pool = lazy_unreachable_pool();
        let result = grant_migrations_read(&pool, "1invalid").await;
        assert!(matches!(result, Err(RoleError::InvalidUsername(_))));
    }

    // Gercek Postgres gerektirir (ADR-070): DATABASE_URL, testler icin
    // ayrilmis bir veritabanina isaret etmeli (proje .env'i degil).
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn seed_bootstrap_account_is_idempotent() {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
        let admin_pool = crate::db::connect_pool(&database_url)
            .await
            .expect("admin pool kurulamadı");
        let (pool, db_name) = crate::test_support::create_temp_db(&admin_pool, &database_url).await;

        let migrator = sqlx::migrate::Migrator::new(Path::new("./migrations"))
            .await
            .expect("migrator kurulamadı");
        migrator.run(&pool).await.expect("migration çalışmadı");

        seed_bootstrap_account(&pool)
            .await
            .expect("ilk seed başarısız");
        let hash_after_first: String =
            sqlx::query_scalar("SELECT password_hash FROM bootstrap_account WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();

        seed_bootstrap_account(&pool)
            .await
            .expect("ikinci çağrı başarısız olmamalı");
        let hash_after_second: String =
            sqlx::query_scalar("SELECT password_hash FROM bootstrap_account WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(
            hash_after_first, hash_after_second,
            "hesap zaten varken parola hash'i değişmemeli"
        );

        drop(pool);
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {db_name}")))
            .execute(&admin_pool)
            .await
            .expect("test veritabanı silinemedi");
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn ensure_role_creates_then_alters_and_grants_migrations_read() {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
        let admin_pool = crate::db::connect_pool(&database_url)
            .await
            .expect("admin pool kurulamadı");
        let (pool, db_name) = crate::test_support::create_temp_db(&admin_pool, &database_url).await;

        let role = format!("opensicil_test_role_{}", std::process::id());
        let cleanup = || async {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE IF EXISTS {role}")))
                .execute(&pool)
                .await;
        };
        cleanup().await;

        ensure_role(&pool, &role, "ilk-parola")
            .await
            .expect("rol ilk kez oluşturulamadı");
        ensure_role(&pool, &role, "ikinci-parola")
            .await
            .expect("rol ikinci kez (ALTER) güncellenemedi");

        sqlx::query("CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT)")
            .execute(&pool)
            .await
            .expect("migrations tablosu oluşturulamadı");

        grant_migrations_read(&pool, &role)
            .await
            .expect("migration okuma izni verilemedi");

        cleanup().await;
        drop(pool);
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {db_name}")))
            .execute(&admin_pool)
            .await
            .expect("test veritabanı silinemedi");
    }

    // Gercek Postgres gerektirir (ADR-070): run()'un tum akisini (rol olustur,
    // migrator calistir, izin ver) tek uctan uca testte dogrular.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn run_succeeds_end_to_end() {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
        let backend_user = format!("opensicil_test_backend_{}", std::process::id());
        let worker_user = format!("opensicil_test_worker_{}", std::process::id());

        // SAFETY: bu test dizisi --include-ignored ile ayrı, tek iş parçacıklı
        // bir çalıştırmada kullanılmak üzere tasarlandı; aynı değişkenleri
        // eşzamanlı değiştiren başka bir test yok.
        unsafe {
            std::env::set_var("DATABASE_URL", &database_url);
            std::env::set_var("POSTGRES_BACKEND_USER", &backend_user);
            std::env::set_var("POSTGRES_BACKEND_PASSWORD", "pw1");
            std::env::set_var("POSTGRES_WORKER_USER", &worker_user);
            std::env::set_var("POSTGRES_WORKER_PASSWORD", "pw2");
        }

        let exit_code = run().await;
        assert_eq!(format!("{exit_code:?}"), format!("{:?}", ExitCode::SUCCESS));

        let pool = crate::db::connect_pool(&database_url)
            .await
            .expect("doğrulama pool'u kurulamadı");
        crate::db::check_schema_ready(&pool)
            .await
            .expect("migrate sonrası şema hazır olmalı");

        for user in [&backend_user, &worker_user] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE IF EXISTS {user}")))
                .execute(&pool)
                .await
                .ok();
        }
    }
}
