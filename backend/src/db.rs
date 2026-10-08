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

    let (failed_migrations, applied): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE NOT success), \
                COALESCE(MAX(version) FILTER (WHERE success), 0) FROM _sqlx_migrations",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("migration durumu okunamadı: {e}"))?;

    schema_readiness(failed_migrations)?;
    schema_current(applied, crate::common_settings::SCHEMA_VERSION)
}

/// ADR-061: eski semaya karsi acilmak sessiz bozulmadir (eksik kolona yazan sorgu ilk
/// istekte duser); daha yeni sema geri donus icin kabul edilir.
fn schema_current(applied: i64, expected: i64) -> Result<(), String> {
    if applied < expected {
        Err(format!(
            "şema eski: veritabanında migration {applied}, binary {expected} bekliyor — önce migrate"
        ))
    } else {
        Ok(())
    }
}

// Kurulum saat dilimi Postgres'in tzdata'siyla cevrilir (Rust'ta tz kutuphanesi
// yok); bilinmeyen ad ilk istekte degil acilista yakalanir (ADR-039 TZ).
pub async fn check_time_zone(pool: &PgPool, time_zone: &str) -> Result<(), String> {
    sqlx::query("SELECT now() AT TIME ZONE $1")
        .bind(time_zone)
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|e| format!("TZ '{time_zone}' Postgres tarafından tanınmıyor: {e}"))
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
    fn older_schema_is_refused_newer_is_accepted() {
        assert!(schema_current(33, 34).is_err());
        assert!(schema_current(34, 34).is_ok());
        assert!(
            schema_current(35, 34).is_ok(),
            "geri dönüşte yeni şema kabul"
        );
    }

    #[test]
    fn ready_when_no_failed_migrations() {
        assert!(schema_readiness(0).is_ok());
    }

    #[test]
    fn not_ready_when_failed_migrations_exist() {
        assert!(schema_readiness(2).is_err());
    }

    #[tokio::test]
    async fn connect_pool_fails_for_unreachable_host() {
        // 127.0.0.1:1 hicbir servis dinlemez ama bind reddi yerine sqlx'in
        // varsayilan 30 sn'lik acquire_timeout'una takilir; kisaltilir.
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            connect_pool("postgres://x:x@127.0.0.1:1/x"),
        )
        .await;
        match result {
            Ok(connect_result) => assert!(connect_result.is_err()),
            Err(_elapsed) => {}
        }
    }

    // Gercek Postgres gerektirir (ADR-070): DATABASE_URL, testler icin
    // ayrilmis bir veritabanina isaret etmeli (proje .env'i degil).
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn check_schema_ready_reports_each_state() {
        let admin_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
        let admin_pool = connect_pool(&admin_url)
            .await
            .expect("admin pool kurulamadı");

        let db_name = format!("opensicil_test_{}", unique_suffix());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db_name}")))
            .execute(&admin_pool)
            .await
            .expect("test veritabanı oluşturulamadı");

        let test_url = replace_db_name(&admin_url, &db_name);
        let pool = connect_pool(&test_url).await.expect("test pool kurulamadı");

        let before = check_schema_ready(&pool).await;
        assert!(
            before.is_err(),
            "migrations tablosu yokken hazır sayılmamalı"
        );

        sqlx::query("CREATE TABLE _sqlx_migrations (version BIGINT, success BOOLEAN)")
            .execute(&pool)
            .await
            .expect("migrations tablosu oluşturulamadı");

        let old = check_schema_ready(&pool).await;
        assert!(
            old.as_ref().is_err_and(|e| e.contains("şema eski")),
            "boş migrations tablosu eski şemadır: {old:?}"
        );
        sqlx::query("INSERT INTO _sqlx_migrations (version, success) VALUES ($1, true)")
            .bind(crate::common_settings::SCHEMA_VERSION)
            .execute(&pool)
            .await
            .expect("güncel migration satırı eklenemedi");
        let ready = check_schema_ready(&pool).await;
        assert!(
            ready.is_ok(),
            "başarısız migration yokken hazır sayılmalı: {ready:?}"
        );

        sqlx::query("INSERT INTO _sqlx_migrations (version, success) VALUES (1, false)")
            .execute(&pool)
            .await
            .expect("başarısız migration satırı eklenemedi");

        let after_failure = check_schema_ready(&pool).await;
        assert!(
            after_failure.is_err(),
            "başarısız migration varken hazır sayılmamalı"
        );

        drop(pool);
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {db_name}")))
            .execute(&admin_pool)
            .await
            .expect("test veritabanı silinemedi");
    }

    // Postgres CREATE DATABASE tirnaksiz tanimlayicilari kucuk harfe cevirir;
    // baglanti URL'i ise ismi oldugu gibi (case-sensitive) kullanir. Karisikligi
    // onlemek icin ad yalnizca rakamdan olusur.
    fn unique_suffix() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{nanos}{count}")
    }

    fn replace_db_name(url: &str, db_name: &str) -> String {
        let base = url.rsplit_once('/').map(|(head, _)| head).unwrap_or(url);
        format!("{base}/{db_name}")
    }
}
