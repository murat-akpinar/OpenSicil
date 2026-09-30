use sqlx::postgres::{PgPool, PgPoolOptions};

pub async fn connect_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
}

// ADR-061 madde 3: sema hazir degilse surec baslamadan cikar. Backend ve worker
// ayri build context'te oldugundan (migrations dosyalarini paylasamazlar),
// kontrol migrate alt komutunun yazdigi _sqlx_migrations defterine bakar.
pub async fn check_schema_ready(pool: &PgPool) -> Result<(), String> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_name = '_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("migration tablosu sorgulanamadı: {e}"))?;

    if !table_exists {
        return Err("migrations hiç çalıştırılmamış".to_string());
    }

    let failed_migrations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = false")
            .fetch_one(pool)
            .await
            .map_err(|e| format!("migration durumu okunamadı: {e}"))?;

    schema_readiness(failed_migrations)
}

fn schema_readiness(failed_migrations: i64) -> Result<(), String> {
    if failed_migrations > 0 {
        Err(format!("{failed_migrations} başarısız migration var"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_when_no_failed_migrations() {
        assert!(schema_readiness(0).is_ok());
    }

    #[test]
    fn not_ready_when_failed_migrations_exist() {
        assert!(schema_readiness(2).is_err());
    }
}
