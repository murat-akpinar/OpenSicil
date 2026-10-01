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

    let (backend_user, worker_user) = match prepare_roles(&pool).await {
        Ok(names) => names,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

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

    if let Err(e) = grant_service_privileges(&pool, &backend_user, &worker_user).await {
        eprintln!("migrate: servis rollerine tablo izni verilemedi: {e}");
        return ExitCode::FAILURE;
    }

    println!("migrate: tamamlandı");
    ExitCode::SUCCESS
}

// Servis rollerinin tablo izinleri (ADR-015): semanin sahibi migrate'i calistiran
// roldur, backend ve worker yalnizca burada verilenleri yapabilir. Yeni tablo
// acan her migration buraya satir ekler; GRANT idempotent, her migrate'te yenilenir.
// - _sqlx_migrations: acilis sema kontrolu (ADR-061 madde 3, db::check_schema_ready)
// - app_settings: worker yalnizca hedef sistem baglanti kolonlarini okur (AD/Zimbra
//   host, bind DN, sifreli parola); OIDC istemci sirri backend'de kalir (ADR-068)
// - audit_log: yalnizca ekleme, performed_by ve id kolonlarina deger verilemez,
//   UPDATE/DELETE yok; niyet sinifi (operation_class) ve sonuc yalnizca worker,
//   aktor yalnizca backend (ADR-016/050/062: sayac worker niyetlerini sayar)
// - identities: backend operator alanlarini yazar; username/email/upn ve
//   silindi temizligi (kisisel veri kolonlari + deleted_at) yalnizca worker
//   (ADR-015, ADR-038, ADR-077). Kimlik satiri hic silinmez, ic ID kalir.
// - identity_additional_roles: suresi dolan atamayi worker siler (ADR-020)
// - catalog_items: yalnizca worker yazar, silmez (kayip isaretler); target_systems
//   satirlari sabit, varsayilanlarini backend gunceller; yetki ogesi ve tek
//   degerli ayar tablolari backend'in (ADR-015, docs/03)
// - account_links: yalnizca worker (ADR-015); backend tek bir kolona yazar:
//   yonetime alma istegi (ADR-018/087, modu yine worker cevirir); jobs: backend ve worker'in
//   zamanlayicisi (ADR-028, ADR-079) acar, backend "tekrar dene" ister,
//   durum/kira/sonuc yalnizca worker (ADR-052, ADR-062)
// - used_names: worker yakar (silmede), backend serbest birakir (ADR-035)
// - attribute_mappings: backend yazar, worker okur ve izinli listeyle dogrular
//   (ADR-029: liste veritabaninda degil worker kodunda)
// - first_passwords: backend ister ve gosterince bosaltir; sifreli parolayi ve
//   red nedenini yalnizca worker yazar (ADR-036/085)
const SERVICE_GRANTS: &str = "\
GRANT SELECT ON _sqlx_migrations TO {backend}, {worker};
GRANT SELECT, INSERT, UPDATE, DELETE ON bootstrap_account, app_settings, \
oidc_auth_requests, operator_sessions TO {backend};
GRANT SELECT (id, ad_host, ad_bind_dn, ad_service_password_enc, zimbra_url, \
zimbra_admin_password_enc) ON app_settings TO {worker};
GRANT SELECT ON audit_log, hourly_counter_usage TO {backend}, {worker};
GRANT INSERT (event_type, detail, actor_subject, actor_username, identity_id, target_system_id) \
ON audit_log TO {backend};
GRANT INSERT (event_type, detail, identity_id, target_system_id, operation_class, emergency, \
intent_id, outcome) ON audit_log TO {worker};
GRANT SELECT, INSERT, UPDATE, DELETE ON departments, roles, identity_additional_roles TO {backend};
GRANT SELECT ON departments, roles TO {worker};
GRANT SELECT, DELETE ON identity_additional_roles TO {worker};
GRANT SELECT ON identities TO {backend}, {worker};
GRANT INSERT ({identity_operator_cols}), UPDATE ({identity_operator_cols}) ON identities TO {backend};
GRANT UPDATE (username, email, upn, given_name, surname, employee_number, mobile_phone, \
national_id_enc, national_id_bidx, national_id_country, deleted_at) ON identities TO {worker};
GRANT SELECT ON target_systems, catalog_items TO {backend}, {worker};
GRANT UPDATE (provision_account_default, default_container_item_id, retention_days, \
delete_requires_approval, password_reset_delay_days) ON target_systems TO {backend};
GRANT INSERT, UPDATE ON catalog_items TO {worker};
GRANT SELECT, INSERT, UPDATE, DELETE ON role_entitlements, department_entitlements, \
role_target_settings, department_target_settings TO {backend};
GRANT SELECT ON role_entitlements, department_entitlements, role_target_settings, \
department_target_settings TO {worker};
GRANT SELECT ON account_links, jobs TO {backend}, {worker};
GRANT INSERT, UPDATE, DELETE ON account_links TO {worker};
GRANT UPDATE (manage_requested_at) ON account_links TO {backend};
GRANT INSERT (identity_id, target_system_id, priority), UPDATE (priority, retry_requested) \
ON jobs TO {backend};
GRANT INSERT (identity_id, target_system_id, priority), UPDATE (status, attempts, \
next_attempt_at, locked_by, locked_until, retry_requested, last_error, result, finished_at) \
ON jobs TO {worker};
GRANT SELECT, INSERT, UPDATE, DELETE ON attribute_mappings TO {backend};
GRANT SELECT ON attribute_mappings TO {worker};
GRANT SELECT ON first_passwords TO {backend}, {worker};
GRANT INSERT (identity_id, target_system_id, requested_by), UPDATE (password_enc, shown_at) \
ON first_passwords TO {backend};
GRANT UPDATE (password_enc, issued_at, error) ON first_passwords TO {worker};
GRANT SELECT ON used_names TO {backend}, {worker};
GRANT INSERT (name, kind, former_identity_id) ON used_names TO {worker};
GRANT UPDATE (released_at, release_reason) ON used_names TO {backend};
GRANT SELECT ON read_jobs TO {backend}, {worker};
GRANT INSERT (kind, target_system_id, requested_by) ON read_jobs TO {backend};
GRANT UPDATE (status, started_at, finished_at, result) ON read_jobs TO {worker};
GRANT INSERT (kind, target_system_id) ON read_jobs TO {worker};
GRANT SELECT ON reconcile_findings TO {backend}, {worker};
GRANT INSERT, DELETE ON reconcile_findings TO {worker};
";

const IDENTITY_OPERATOR_COLUMNS: &str = "given_name, surname, employee_number, mobile_phone, \
existing_ad_account_hint, existing_zimbra_account_hint, department_id, primary_role_id, \
manager_id, handover_manager_id, employment_type, start_date, end_at, suspension_start, \
suspension_end, cancelled, emergency_departure, national_id_enc, national_id_bidx, national_id_country, \
requested_username, name_conflict_override";

async fn grant_service_privileges(
    pool: &PgPool,
    backend: &str,
    worker: &str,
) -> Result<(), RoleError> {
    validate_role_name(backend)?;
    validate_role_name(worker)?;

    // Denetlendi: rol adlari validate_role_name ile sinirlandi; GRANT bind
    // parametresi desteklemez (ensure_role'daki gibi). raw_sql: birden fazla
    // ifade tek gidiste, biri hata verirse tumu geri alinir.
    let stmt = SERVICE_GRANTS
        .replace("{identity_operator_cols}", IDENTITY_OPERATOR_COLUMNS)
        .replace("{backend}", backend)
        .replace("{worker}", worker);
    sqlx::raw_sql(sqlx::AssertSqlSafe(stmt))
        .execute(pool)
        .await
        .map_err(RoleError::Db)?;
    Ok(())
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

// (backend, worker) rol adlarini doner; izinler migration sonrasi bu adlara verilir.
async fn prepare_roles(pool: &PgPool) -> Result<(String, String), String> {
    let backend = prepare_role(pool, "POSTGRES_BACKEND_USER", "POSTGRES_BACKEND_PASSWORD").await?;
    let worker = prepare_role(pool, "POSTGRES_WORKER_USER", "POSTGRES_WORKER_PASSWORD").await?;
    Ok((backend, worker))
}

async fn prepare_role(pool: &PgPool, user_var: &str, pass_var: &str) -> Result<String, String> {
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
    Ok(user)
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
    async fn grant_service_privileges_rejects_invalid_usernames_before_querying() {
        let pool = lazy_unreachable_pool();
        let bad_backend = grant_service_privileges(&pool, "1invalid", "worker").await;
        assert!(matches!(bad_backend, Err(RoleError::InvalidUsername(_))));
        let bad_worker =
            grant_service_privileges(&pool, "backend", "x; DROP TABLE audit_log").await;
        assert!(matches!(bad_worker, Err(RoleError::InvalidUsername(_))));
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
    async fn ensure_role_creates_then_alters() {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
        let admin_pool = crate::db::connect_pool(&database_url)
            .await
            .expect("admin pool kurulamadı");
        let (pool, db_name) = crate::test_support::create_temp_db(&admin_pool, &database_url).await;

        let role = format!("opensicil_test_role_{}", std::process::id());
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE IF EXISTS {role}")))
            .execute(&pool)
            .await
            .expect("eski test rolü temizlenemedi");

        ensure_role(&pool, &role, "ilk-parola")
            .await
            .expect("rol ilk kez oluşturulamadı");
        ensure_role(&pool, &role, "ikinci-parola")
            .await
            .expect("rol ikinci kez (ALTER) güncellenemedi");

        crate::test_support::drop_role(&pool, &role).await;
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-015 "izinler testle dogrulanir": servis rolleri gercek Postgres'te
    // yalnizca SERVICE_GRANTS'in verdigini yapabilmeli.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn service_roles_can_only_do_what_is_granted() {
        let f = ServiceRoles::setup("grant").await;
        grant_service_privileges(&f.pool, &f.backend, &f.worker)
            .await
            .expect("ikinci migrate: GRANT idempotent olmalı");

        sqlx::query("UPDATE app_settings SET ad_host = 'dc1' WHERE id = TRUE")
            .execute(&f.backend_pool)
            .await
            .expect("backend kendi ayar tablosuna yazabilmeli");
        assert!(
            sqlx::query("SELECT username FROM bootstrap_account")
                .execute(&f.worker_pool)
                .await
                .is_err(),
            "worker backend tablolarını okuyamamalı"
        );

        for (service_pool, role) in [(&f.backend_pool, &f.backend), (&f.worker_pool, &f.worker)] {
            assert_audit_is_append_only(service_pool, role).await;
        }
        assert_only_worker_writes_intents(&f.backend_pool, &f.worker_pool).await;
        f.teardown().await;
    }

    // ADR-016/062: niyet satirini (sayac sinifi) yalnizca worker yazar, backend
    // sahte niyetle sayaci dolduramaz; aktoru yalnizca backend yazar.
    async fn assert_only_worker_writes_intents(backend_pool: &PgPool, worker_pool: &PgPool) {
        assert_rejected(
            backend_pool,
            "INSERT INTO audit_log (event_type, operation_class) VALUES ('sahte', 'destructive')",
            &[],
            "backend niyet satırı yazamamalı",
        )
        .await;
        let intent: i64 = sqlx::query_scalar(
            "INSERT INTO audit_log (event_type, operation_class, emergency) \
             VALUES ('ad.account.disable', 'destructive', TRUE) RETURNING id",
        )
        .fetch_one(worker_pool)
        .await
        .expect("worker niyet satırı yazabilmeli");
        sqlx::query(
            "INSERT INTO audit_log (event_type, intent_id, outcome) \
             VALUES ('ad.account.disable', $1, 'succeeded')",
        )
        .bind(intent)
        .execute(worker_pool)
        .await
        .expect("worker sonuç satırını niyetine bağlayabilmeli");
        assert_rejected(
            worker_pool,
            "INSERT INTO audit_log (event_type, actor_username) VALUES ('x', 'sahte')",
            &[],
            "worker aktör uyduramamalı",
        )
        .await;
    }

    // ADR-077: rol turu kisiti veritabaninda, kimlik kolonlari rol bazli.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn identity_model_enforces_role_kinds_and_column_grants() {
        let f = ServiceRoles::setup("model").await;
        let seed = seed_department_and_roles(&f.backend_pool).await;
        let identity_id = assert_backend_writes_operator_fields(&f.backend_pool, &seed).await;
        assert_worker_writes_only_its_columns(&f.worker_pool, identity_id).await;
        assert_only_worker_writes_links(&f.backend_pool, &f.worker_pool, identity_id).await;
        f.teardown().await;
    }

    // ADR-015: hesap baglantisini yalnizca worker yazar (docs/07 kapanis listesi).
    async fn assert_only_worker_writes_links(
        backend_pool: &PgPool,
        worker_pool: &PgPool,
        identity_id: i64,
    ) {
        const NEW_LINK: &str = "INSERT INTO account_links \
            (identity_id, target_system_id, external_id, origin, mode) \
            VALUES ($1, $2, 'guid-1', 'provisioned', 'managed')";
        let ad: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(backend_pool)
            .await
            .unwrap();
        assert_rejected(
            backend_pool,
            NEW_LINK,
            &[identity_id, ad],
            "backend hesap bağlantısı yazamamalı",
        )
        .await;
        bind_all(NEW_LINK, &[identity_id, ad])
            .execute(worker_pool)
            .await
            .expect("worker hesap bağlantısı yazabilmeli");
        assert_rejected(
            backend_pool,
            "UPDATE account_links SET applied_state = 'active' WHERE identity_id = $1",
            &[identity_id],
            "backend uygulanan durumu değiştirememeli",
        )
        .await;
        // ADR-087: backend yalnizca yonetime alma istegini yazar, modu cevirmek worker'in
        assert_rejected(
            backend_pool,
            "UPDATE account_links SET mode = 'managed' WHERE identity_id = $1",
            &[identity_id],
            "backend gözlem modunu kendisi çevirememeli",
        )
        .await;
        bind_all(
            "UPDATE account_links SET manage_requested_at = now() WHERE identity_id = $1",
            &[identity_id],
        )
        .execute(backend_pool)
        .await
        .expect("backend yönetime alma isteğini yazabilmeli");
    }

    struct Seed {
        department: i64,
        base: i64,
        primary: i64,
        additional: i64,
    }

    async fn seed_department_and_roles(pool: &PgPool) -> Seed {
        let department: i64 = sqlx::query_scalar(
            "INSERT INTO departments (name, code) VALUES ('BT', 'BT') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .expect("departman açılamadı");
        let primary: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, title) VALUES ('primary', 'Uzman', 'Uzman') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .expect("birincil rol açılamadı");
        let additional: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name) VALUES ('additional', 'Nöbet') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .expect("ek rol açılamadı");
        let base: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name) VALUES ('base', 'Temel') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .expect("temel rol açılamadı");

        let forbidden = [
            "INSERT INTO roles (kind, name) VALUES ('base', 'İkinci Temel')",
            "INSERT INTO roles (kind, name, title) VALUES ('additional', 'X', 'unvan')",
        ];
        for sql in forbidden {
            assert!(
                sqlx::query(sql).execute(pool).await.is_err(),
                "reddedilmeliydi: {sql}"
            );
        }
        Seed {
            department,
            base,
            primary,
            additional,
        }
    }

    const INSERT_IDENTITY: &str = "INSERT INTO identities \
        (given_name, surname, department_id, primary_role_id, employment_type, start_date) \
        VALUES ('Ayşe', 'Yılmaz', $1, $2, $3, current_date) RETURNING id";

    async fn assert_backend_writes_operator_fields(pool: &PgPool, seed: &Seed) -> i64 {
        let id: i64 = sqlx::query_scalar(INSERT_IDENTITY)
            .bind(seed.department)
            .bind(seed.primary)
            .bind("permanent")
            .fetch_one(pool)
            .await
            .expect("backend kimlik açabilmeli");
        for (role, employment_type, why) in [
            (
                seed.additional,
                "permanent",
                "ek rol birincil rol olarak bağlanamamalı",
            ),
            (seed.primary, "intern", "stajyerde bitiş tarihi zorunlu"),
        ] {
            let result = sqlx::query_scalar::<_, i64>(INSERT_IDENTITY)
                .bind(seed.department)
                .bind(role)
                .bind(employment_type)
                .fetch_one(pool)
                .await;
            assert!(result.is_err(), "{why}");
        }

        sqlx::query(
            "INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) \
             VALUES ($1, $2, current_date + 14)",
        )
        .bind(id)
        .bind(seed.additional)
        .execute(pool)
        .await
        .expect("ek rol atanabilmeli");
        sqlx::query("UPDATE identities SET mobile_phone = '+905321234567' WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("backend operatör alanını güncelleyebilmeli");

        let forbidden = [
            "INSERT INTO identity_additional_roles (identity_id, role_id) VALUES ($1, $2)",
            "UPDATE identities SET username = 'ayse.yilmaz' WHERE id = $1 AND $2 = $2",
            "DELETE FROM departments WHERE id = (SELECT department_id FROM identities WHERE id = $1) AND $2 = $2",
        ];
        for sql in forbidden {
            let result = sqlx::query(sql)
                .bind(id)
                .bind(seed.primary)
                .execute(pool)
                .await;
            assert!(result.is_err(), "backend için reddedilmeliydi: {sql}");
        }
        id
    }

    async fn assert_worker_writes_only_its_columns(pool: &PgPool, id: i64) {
        sqlx::query("UPDATE identities SET username = 'ayse.yilmaz', email = 'ayse@example.com' WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("worker üretilen adı yazabilmeli");
        sqlx::query("DELETE FROM identity_additional_roles WHERE identity_id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("worker süresi dolan ek rolü silebilmeli");

        let forbidden = [
            "UPDATE identities SET start_date = current_date WHERE id = $1",
            "INSERT INTO identities (given_name, surname, department_id, primary_role_id, employment_type, start_date) \
             SELECT 'x', 'y', department_id, primary_role_id, 'permanent', current_date FROM identities WHERE id = $1",
            "DELETE FROM identities WHERE id = $1",
            "INSERT INTO roles (kind, name) SELECT 'additional', 'worker-yazamaz' WHERE $1 IS NOT NULL",
        ];
        for sql in forbidden {
            let result = sqlx::query(sql).bind(id).execute(pool).await;
            assert!(result.is_err(), "worker için reddedilmeliydi: {sql}");
        }
    }

    // docs/03 katalog: yetki ogesi yalnizca grup/liste, konteyner yalnizca ayni
    // hedefin OU/COS'u, tek degerli ayar yalnizca birincil rol; katalogu worker yazar.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn catalog_enforces_item_kinds_and_writer_roles() {
        let f = ServiceRoles::setup("catalog").await;
        let catalog = crate::test_support::seed_example_catalog(&f.pool).await;
        let seed = seed_department_and_roles(&f.backend_pool).await;
        assert_backend_links_roles_to_catalog(&f.backend_pool, &seed, &catalog).await;
        assert_only_worker_writes_catalog(&f.backend_pool, &f.worker_pool, &catalog).await;
        f.teardown().await;
    }

    // sqlx yalnizca 'static SQL'i "denetlenmis" sayar; testteki her sorgu sabittir.
    fn bind_all(
        sql: &'static str,
        binds: &[i64],
    ) -> sqlx::query::Query<'static, sqlx::Postgres, sqlx::postgres::PgArguments> {
        let mut query = sqlx::query(sql);
        for value in binds {
            query = query.bind(*value);
        }
        query
    }

    async fn assert_rejected(pool: &PgPool, sql: &'static str, binds: &[i64], why: &str) {
        assert!(
            bind_all(sql, binds).execute(pool).await.is_err(),
            "{why}: {sql}"
        );
    }

    async fn assert_backend_links_roles_to_catalog(
        pool: &PgPool,
        seed: &Seed,
        catalog: &crate::test_support::ExampleCatalog,
    ) {
        const ROLE_ENT: &str =
            "INSERT INTO role_entitlements (role_id, catalog_item_id) VALUES ($1, $2)";
        const ROLE_SET: &str = "INSERT INTO role_target_settings \
            (role_id, target_system_id, container_item_id) VALUES ($1, $2, $3)";
        const DEPT_ENT: &str =
            "INSERT INTO department_entitlements (department_id, catalog_item_id) VALUES ($1, $2)";
        // docs/03 "Örnek" bölümü: temel rol, departman BT, birincil Uzman, ek Nöbet
        let allowed: [(&'static str, Vec<i64>); 11] = [
            (ROLE_ENT, vec![seed.base, catalog.gg_internet]),
            (ROLE_ENT, vec![seed.base, catalog.list_herkes]),
            (DEPT_ENT, vec![seed.department, catalog.gg_bt_paylasim]),
            (DEPT_ENT, vec![seed.department, catalog.list_bt]),
            (ROLE_ENT, vec![seed.primary, catalog.gg_sistem_uzmanlari]),
            (ROLE_ENT, vec![seed.primary, catalog.gg_vpn]),
            (ROLE_ENT, vec![seed.additional, catalog.gg_nobet]),
            (ROLE_ENT, vec![seed.additional, catalog.list_nobet]),
            (
                ROLE_SET,
                vec![seed.primary, catalog.ad, catalog.sistem_uzmanlari_ou],
            ),
            (
                ROLE_SET,
                vec![seed.primary, catalog.zimbra, catalog.cos_teknik],
            ),
            (
                "INSERT INTO department_target_settings \
                 (department_id, target_system_id, email_domain) VALUES ($1, $2, 'example.com')",
                vec![seed.department, catalog.zimbra],
            ),
        ];
        for (sql, binds) in allowed {
            bind_all(sql, &binds)
                .execute(pool)
                .await
                .unwrap_or_else(|e| panic!("izinli olmalıydı: {sql}: {e}"));
        }

        let rejected: [(&'static str, Vec<i64>, &str); 4] = [
            (
                ROLE_ENT,
                vec![seed.primary, catalog.personel_ou],
                "OU yetki öğesi olamaz",
            ),
            (
                ROLE_SET,
                vec![seed.additional, catalog.ad, catalog.personel_ou],
                "ek rol tek değerli ayar taşıyamaz",
            ),
            (
                ROLE_SET,
                vec![seed.primary, catalog.zimbra, catalog.gg_vpn],
                "grup konteyner olamaz",
            ),
            (
                ROLE_SET,
                vec![seed.primary, catalog.zimbra, catalog.personel_ou],
                "konteyner başka hedefin olamaz",
            ),
        ];
        for (sql, binds, why) in rejected {
            assert_rejected(pool, sql, &binds, why).await;
        }
    }

    async fn assert_only_worker_writes_catalog(
        backend_pool: &PgPool,
        worker_pool: &PgPool,
        catalog: &crate::test_support::ExampleCatalog,
    ) {
        const NEW_ITEM: &str = "INSERT INTO catalog_items \
            (target_system_id, kind, external_id, display_name) VALUES ($1, 'group', 'guid-yeni', 'GG-Yeni')";
        sqlx::query("UPDATE target_systems SET retention_days = 30 WHERE id = $1")
            .bind(catalog.ad)
            .execute(backend_pool)
            .await
            .expect("backend hedef sistem varsayılanını güncelleyebilmeli");
        assert_rejected(
            backend_pool,
            NEW_ITEM,
            &[catalog.ad],
            "backend katalog yazamamalı",
        )
        .await;
        assert_rejected(
            backend_pool,
            "UPDATE target_systems SET kind = 'zimbra' WHERE id = $1",
            &[catalog.ad],
            "backend hedef sistem türünü değiştirememeli",
        )
        .await;

        // Worker hedef baglanti ayarlarini okur, OIDC sirrini okuyamaz (ADR-068).
        sqlx::query("SELECT ad_host, ad_bind_dn, ad_service_password_enc FROM app_settings")
            .execute(worker_pool)
            .await
            .expect("worker AD bağlantı ayarlarını okuyabilmeli");
        assert_rejected(
            worker_pool,
            "SELECT oidc_client_secret_enc FROM app_settings WHERE $1 = $1",
            &[catalog.ad],
            "worker OIDC istemci sırrını okuyamamalı",
        )
        .await;
        sqlx::query(NEW_ITEM)
            .bind(catalog.ad)
            .execute(worker_pool)
            .await
            .expect("worker katalog öğesi ekleyebilmeli");
        sqlx::query("UPDATE catalog_items SET missing_since = now() WHERE id = $1")
            .bind(catalog.gg_nobet)
            .execute(worker_pool)
            .await
            .expect("worker öğeyi kayıp işaretleyebilmeli");
        assert_rejected(
            worker_pool,
            "DELETE FROM catalog_items WHERE id = $1",
            &[catalog.gg_nobet],
            "katalog öğesi silinmez, kayıp işaretlenir",
        )
        .await;
        assert_rejected(
            worker_pool,
            "INSERT INTO role_entitlements (role_id, catalog_item_id) SELECT id, $1 FROM roles LIMIT 1",
            &[catalog.gg_vpn],
            "worker rol tanımı yazamamalı",
        )
        .await;
    }

    // Gecici DB + iki servis rolu + GRANT; izin testleri paylasir.
    struct ServiceRoles {
        admin_pool: PgPool,
        pool: PgPool,
        db_name: String,
        backend: String,
        worker: String,
        backend_pool: PgPool,
        worker_pool: PgPool,
    }

    impl ServiceRoles {
        async fn setup(tag: &str) -> Self {
            let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
            let admin_url = std::env::var("DATABASE_URL").expect("DATABASE_URL ayarlanmalı");
            let pid = std::process::id();
            let backend = format!("opensicil_test_{tag}_backend_{pid}");
            let worker = format!("opensicil_test_{tag}_worker_{pid}");
            ensure_role(&pool, &backend, "pw-b")
                .await
                .expect("backend rolü");
            ensure_role(&pool, &worker, "pw-w")
                .await
                .expect("worker rolü");
            grant_service_privileges(&pool, &backend, &worker)
                .await
                .expect("GRANT başarısız");
            let backend_pool =
                crate::test_support::connect_as(&admin_url, &db_name, &backend, "pw-b").await;
            let worker_pool =
                crate::test_support::connect_as(&admin_url, &db_name, &worker, "pw-w").await;
            Self {
                admin_pool,
                pool,
                db_name,
                backend,
                worker,
                backend_pool,
                worker_pool,
            }
        }

        async fn teardown(self) {
            self.backend_pool.close().await;
            self.worker_pool.close().await;
            for role in [&self.backend, &self.worker] {
                crate::test_support::drop_role(&self.pool, role).await;
            }
            drop(self.pool);
            crate::test_support::drop_temp_db(&self.admin_pool, &self.db_name).await;
        }
    }

    async fn assert_audit_is_append_only(pool: &PgPool, role: &str) {
        let performed_by: String = sqlx::query_scalar(
            "INSERT INTO audit_log (event_type) VALUES ('test') RETURNING performed_by::text",
        )
        .fetch_one(pool)
        .await
        .expect("servis rolü denetim satırı ekleyebilmeli");
        assert_eq!(performed_by, role, "performed_by current_user'dan gelmeli");

        let forbidden = [
            "INSERT INTO audit_log (event_type, performed_by) VALUES ('test', 'sahte')",
            "INSERT INTO audit_log (event_type, occurred_at) VALUES ('test', now() - interval '2 hours')",
            "UPDATE audit_log SET event_type = 'x'",
            "DELETE FROM audit_log",
        ];
        for sql in forbidden {
            assert!(
                sqlx::query(sql).execute(pool).await.is_err(),
                "{role} için reddedilmeliydi: {sql}"
            );
        }
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
            crate::test_support::drop_role(&pool, user).await;
        }
    }
}
