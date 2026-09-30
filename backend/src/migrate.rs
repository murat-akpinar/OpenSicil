use std::path::Path;
use std::process::ExitCode;

use sqlx::PgPool;

pub async fn run() -> ExitCode {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("migrate: DATABASE_URL ortam değişkeni eksik");
            return ExitCode::FAILURE;
        }
    };

    let pool = match crate::db::connect_pool(&database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("migrate: veritabanına bağlanılamadı: {e}");
            return ExitCode::FAILURE;
        }
    };

    for (user_var, pass_var) in [
        ("POSTGRES_BACKEND_USER", "POSTGRES_BACKEND_PASSWORD"),
        ("POSTGRES_WORKER_USER", "POSTGRES_WORKER_PASSWORD"),
    ] {
        if let Err(e) = ensure_role(&pool, user_var, pass_var).await {
            eprintln!("migrate: rol hazırlanamadı ({user_var}): {e}");
            return ExitCode::FAILURE;
        }
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

    // backend ve worker acilista _sqlx_migrations'a bakarak semanin hazir
    // olup olmadigini kontrol eder (ADR-061 madde 3, crate::db::check_schema_ready)
    for user_var in ["POSTGRES_BACKEND_USER", "POSTGRES_WORKER_USER"] {
        if let Err(e) = grant_migrations_read(&pool, user_var).await {
            eprintln!("migrate: migration tablosu izni verilemedi ({user_var}): {e}");
            return ExitCode::FAILURE;
        }
    }

    println!("migrate: tamamlandı");
    ExitCode::SUCCESS
}

#[derive(Debug)]
enum RoleError {
    MissingEnv(String),
    InvalidUsername(String),
    Db(sqlx::Error),
}

impl std::fmt::Display for RoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleError::MissingEnv(name) => write!(f, "ortam değişkeni eksik: {name}"),
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

async fn ensure_role(pool: &PgPool, user_var: &str, pass_var: &str) -> Result<(), RoleError> {
    let user = std::env::var(user_var).map_err(|_| RoleError::MissingEnv(user_var.to_string()))?;
    let pass = std::env::var(pass_var).map_err(|_| RoleError::MissingEnv(pass_var.to_string()))?;
    validate_role_name(&user)?;
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

async fn grant_migrations_read(pool: &PgPool, user_var: &str) -> Result<(), RoleError> {
    let user = std::env::var(user_var).map_err(|_| RoleError::MissingEnv(user_var.to_string()))?;
    validate_role_name(&user)?;

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
}
