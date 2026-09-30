#![cfg(test)]

// Postgres gerektiren testler paralel calisir; paylasilan testdb'ye yazan bir
// test digerini bozar. Her cagiran kendi gecici veritabanini acar (ADR-070).

use std::path::Path;

use sqlx::PgPool;

pub async fn create_temp_db(admin_pool: &PgPool, admin_url: &str) -> (PgPool, String) {
    let db_name = format!(
        "opensicil_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db_name}")))
        .execute(admin_pool)
        .await
        .expect("test veritabanı oluşturulamadı");

    let base = admin_url
        .rsplit_once('/')
        .map(|(head, _)| head)
        .unwrap_or(admin_url);
    let pool = crate::db::connect_pool(&format!("{base}/{db_name}"))
        .await
        .expect("test pool kurulamadı");
    (pool, db_name)
}

// Gecici veritabanina verilen servis rolüyle baglanir; host:port DATABASE_URL'den alinir.
pub async fn connect_as(admin_url: &str, db_name: &str, user: &str, pass: &str) -> PgPool {
    let host = admin_url
        .rsplit_once('/')
        .map(|(head, _)| head)
        .unwrap_or(admin_url)
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(admin_url);
    crate::db::connect_pool(&format!("postgres://{user}:{pass}@{host}/{db_name}"))
        .await
        .expect("servis rolüyle bağlanılamadı")
}

// DROP ROLE, role verilmis izin durdukca reddedilir; once bu veritabanindaki
// izinler (DROP OWNED BY) kaldirilir.
pub async fn drop_role(pool: &PgPool, role: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP OWNED BY {role}; DROP ROLE {role}"
    )))
    .execute(pool)
    .await
    .expect("test rolü silinemedi");
}

pub async fn drop_temp_db(admin_pool: &PgPool, db_name: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {db_name}")))
        .execute(admin_pool)
        .await
        .expect("test veritabanı silinemedi");
}

// Bos, gercek migrator ile kurulmus bir veritabani doner; cagiran isini bitirince
// (admin_pool, db_name) ile drop_temp_db cagirmalidir.
pub async fn fresh_migrated_db() -> (PgPool, PgPool, String) {
    let admin_url = std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
    let admin_pool = crate::db::connect_pool(&admin_url)
        .await
        .expect("admin pool kurulamadı");
    let (pool, db_name) = create_temp_db(&admin_pool, &admin_url).await;

    let migrator = sqlx::migrate::Migrator::new(Path::new("./migrations"))
        .await
        .expect("migrator kurulamadı");
    migrator.run(&pool).await.expect("migration çalışmadı");

    (admin_pool, pool, db_name)
}
