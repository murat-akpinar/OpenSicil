#![cfg(test)]

// Postgres gerektiren testler paralel calisir; paylasilan testdb'ye yazan bir
// test digerini bozar. Her cagiran kendi gecici veritabanini acar (ADR-070).

use std::path::Path;

use sqlx::PgPool;

pub async fn create_temp_db(admin_pool: &PgPool, admin_url: &str) -> (PgPool, String) {
    // Saat çözünürlüğü kaba olabilir; aynı anda başlayan testler aynı damgayı alır.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let db_name = format!(
        "opensicil_test_{}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

// docs/03'teki ornek katalog: AD OU'lari ve gruplari, Zimbra COS'lari ve
// listeleri. Katalogu gercek AD'den dolduran connector 3a'da; o gune kadar
// (ve saf fonksiyon testlerinde) test verisi budur. Sahip roluyle yazilir.
pub struct ExampleCatalog {
    pub ad: i64,
    pub zimbra: i64,
    pub personel_ou: i64,
    pub sistem_uzmanlari_ou: i64,
    pub gg_internet: i64,
    pub gg_bt_paylasim: i64,
    pub gg_sistem_uzmanlari: i64,
    pub gg_vpn: i64,
    pub gg_nobet: i64,
    pub cos_default: i64,
    pub cos_teknik: i64,
    pub list_herkes: i64,
    pub list_bt: i64,
    pub list_nobet: i64,
}

pub async fn seed_example_catalog(pool: &PgPool) -> ExampleCatalog {
    let ad = target_id(pool, "ad").await;
    let zimbra = target_id(pool, "zimbra").await;
    let ad_item = |kind, name, location| insert_item(pool, ad, kind, name, location);
    let zimbra_item = |kind, name, location| insert_item(pool, zimbra, kind, name, location);
    let groups_ou = "OU=Gruplar,DC=example,DC=local";
    let catalog = ExampleCatalog {
        ad,
        zimbra,
        personel_ou: ad_item("ou", "Personel", "OU=Personel,DC=example,DC=local".into()).await,
        sistem_uzmanlari_ou: ad_item(
            "ou",
            "SistemUzmanlari",
            "OU=SistemUzmanlari,OU=Personel,DC=example,DC=local".into(),
        )
        .await,
        gg_internet: ad_item(
            "group",
            "GG-Internet",
            format!("CN=GG-Internet,{groups_ou}"),
        )
        .await,
        gg_bt_paylasim: ad_item(
            "group",
            "GG-BT-Paylasim",
            format!("CN=GG-BT-Paylasim,{groups_ou}"),
        )
        .await,
        gg_sistem_uzmanlari: ad_item(
            "group",
            "GG-Sistem-Uzmanlari",
            format!("CN=GG-Sistem-Uzmanlari,{groups_ou}"),
        )
        .await,
        gg_vpn: ad_item("group", "GG-VPN", format!("CN=GG-VPN,{groups_ou}")).await,
        gg_nobet: ad_item("group", "GG-Nobet", format!("CN=GG-Nobet,{groups_ou}")).await,
        cos_default: zimbra_item("cos", "default", "default".into()).await,
        cos_teknik: zimbra_item("cos", "teknik", "teknik".into()).await,
        list_herkes: zimbra_item("distribution_list", "herkes", "herkes@example.com".into()).await,
        list_bt: zimbra_item("distribution_list", "bt", "bt@example.com".into()).await,
        list_nobet: zimbra_item("distribution_list", "nobet", "nobet@example.com".into()).await,
    };
    set_default_container(pool, ad, catalog.personel_ou).await;
    set_default_container(pool, zimbra, catalog.cos_default).await;
    catalog
}

async fn insert_item(pool: &PgPool, target: i64, kind: &str, name: &str, location: String) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO catalog_items (target_system_id, kind, external_id, display_name, location) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(target)
    .bind(kind)
    .bind(format!("test-{kind}-{name}"))
    .bind(name)
    .bind(location)
    .fetch_one(pool)
    .await
    .expect("katalog öğesi eklenemedi")
}

async fn set_default_container(pool: &PgPool, target: i64, item: i64) {
    sqlx::query("UPDATE target_systems SET default_container_item_id = $1 WHERE id = $2")
        .bind(item)
        .bind(target)
        .execute(pool)
        .await
        .expect("hedef sistem varsayılan konteyneri yazılamadı");
}

// Bir departman, bir birincil rol ve iki kadrolu kimlik; kimlik gerektiren
// testler icin en kucuk veri. Sahip roluyle yazilir.
pub async fn seed_two_identities(pool: &PgPool) -> [i64; 2] {
    let department: i64 =
        sqlx::query_scalar("INSERT INTO departments (name) VALUES ('Test Birimi') RETURNING id")
            .fetch_one(pool)
            .await
            .expect("departman açılamadı");
    let role: i64 = sqlx::query_scalar(
        "INSERT INTO roles (kind, name) VALUES ('primary', 'Test Rolü') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .expect("rol açılamadı");
    let mut ids = [0_i64; 2];
    for (i, (given, surname)) in [("Ayşe", "Yılmaz"), ("Ali", "Kaya")]
        .into_iter()
        .enumerate()
    {
        ids[i] = sqlx::query_scalar(
            "INSERT INTO identities \
             (given_name, surname, department_id, primary_role_id, employment_type, start_date) \
             VALUES ($1, $2, $3, $4, 'permanent', current_date) RETURNING id",
        )
        .bind(given)
        .bind(surname)
        .bind(department)
        .bind(role)
        .fetch_one(pool)
        .await
        .expect("kimlik açılamadı");
    }
    ids
}

async fn target_id(pool: &PgPool, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = $1")
        .bind(kind)
        .fetch_one(pool)
        .await
        .expect("hedef sistem satırı seed edilmiş olmalı")
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
