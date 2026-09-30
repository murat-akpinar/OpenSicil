#![cfg(test)]

// Gercek Postgres gerektiren testler kendi gecici veritabanini acar (ADR-070).
// Sema backend'in migrations dizininden kurulur: worker imajinda o dizin
// yoktur ama testler repo'da kosar (ADR-070 ek).

use std::path::Path;

use sqlx::PgPool;

pub async fn fresh_migrated_db() -> (PgPool, PgPool, String) {
    let admin_url = std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
    let admin_pool = crate::db::connect_pool(&admin_url)
        .await
        .expect("admin pool kurulamadı");
    // Saat çözünürlüğü kaba olabilir; aynı anda başlayan testler aynı damgayı alır.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let db_name = format!(
        "opensicil_worker_test_{}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db_name}")))
        .execute(&admin_pool)
        .await
        .expect("test veritabanı oluşturulamadı");
    let base = admin_url
        .rsplit_once('/')
        .map(|(head, _)| head)
        .unwrap_or(&admin_url);
    let pool = crate::db::connect_pool(&format!("{base}/{db_name}"))
        .await
        .expect("test pool kurulamadı");
    sqlx::migrate::Migrator::new(Path::new("../backend/migrations"))
        .await
        .expect("backend migrations dizini okunamadı")
        .run(&pool)
        .await
        .expect("migration çalışmadı");
    (admin_pool, pool, db_name)
}

pub async fn drop_temp_db(admin_pool: &PgPool, db_name: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {db_name}")))
        .execute(admin_pool)
        .await
        .expect("test veritabanı silinemedi");
}

pub struct ExampleModel {
    pub ad: i64,
    pub identity: i64,
    pub other_identity: i64,
    pub gg_internet: i64,
    pub gg_bt_paylasim: i64,
    pub gg_ankara_yazici: i64,
    pub gg_sistem_uzmanlari: i64,
    pub gg_nobet: i64,
    pub sistem_uzmanlari_ou: i64,
}

// docs/03 ornegi AD icin: Ankara → Bilgi Islem, temel rol, Sistem Uzmani
// (birincil, OU SistemUzmanlari), Nobet Ekibi (ek, 14 gun). Sahip roluyle yazilir.
pub async fn seed_example_model(pool: &PgPool) -> ExampleModel {
    let ad: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
        .fetch_one(pool)
        .await
        .unwrap();
    let mut model = seed_catalog(pool, ad).await;
    let (ankara, bt, primary, additional) = seed_org(pool, &model).await;

    let identity_sql = "INSERT INTO identities \
        (given_name, surname, department_id, primary_role_id, employment_type, start_date) \
        VALUES ($1, $2, $3, $4, 'permanent', current_date - 30) RETURNING id";
    model.identity = sqlx::query_scalar(identity_sql)
        .bind("Ayşe")
        .bind("Yılmaz")
        .bind(bt)
        .bind(primary)
        .fetch_one(pool)
        .await
        .unwrap();
    model.other_identity = sqlx::query_scalar(identity_sql)
        .bind("Ali")
        .bind("Kaya")
        .bind(ankara)
        .bind(primary)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) \
         VALUES ($1, $2, current_date + 14)",
    )
    .bind(model.identity)
    .bind(additional)
    .execute(pool)
    .await
    .unwrap();
    model
}

// Katalog ogeleri ve hedef varsayilani; kimlik alanlari henuz 0.
async fn seed_catalog(pool: &PgPool, ad: i64) -> ExampleModel {
    let item = |kind: &'static str, name: &'static str| {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO catalog_items (target_system_id, kind, external_id, display_name) \
             VALUES ($1, $2, $3, $3) RETURNING id",
        )
        .bind(ad)
        .bind(kind)
        .bind(name)
        .fetch_one(pool)
    };
    let personel_ou = item("ou", "Personel").await.unwrap();
    let model = ExampleModel {
        ad,
        identity: 0,
        other_identity: 0,
        gg_internet: item("group", "GG-Internet").await.unwrap(),
        gg_bt_paylasim: item("group", "GG-BT-Paylasim").await.unwrap(),
        gg_ankara_yazici: item("group", "GG-Ankara-Yazici").await.unwrap(),
        gg_sistem_uzmanlari: item("group", "GG-Sistem-Uzmanlari").await.unwrap(),
        gg_nobet: item("group", "GG-Nobet").await.unwrap(),
        sistem_uzmanlari_ou: item("ou", "SistemUzmanlari").await.unwrap(),
    };
    sqlx::query("UPDATE target_systems SET default_container_item_id = $1 WHERE id = $2")
        .bind(personel_ou)
        .bind(ad)
        .execute(pool)
        .await
        .unwrap();
    model
}

// Departman agaci, roller, yetki ogeleri ve tek degerli ayar; (ankara, bt, primary, additional).
async fn seed_org(pool: &PgPool, m: &ExampleModel) -> (i64, i64, i64, i64) {
    let ankara: i64 = sqlx::query_scalar(
        "INSERT INTO departments (name, code) VALUES ('Ankara', 'ANK') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let bt: i64 = sqlx::query_scalar(
        "INSERT INTO departments (name, code, parent_id) VALUES ('Bilgi İşlem', 'BT', $1) RETURNING id",
    )
    .bind(ankara)
    .fetch_one(pool)
    .await
    .unwrap();
    let role = |kind: &'static str, name: &'static str| {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO roles (kind, name, title) VALUES ($1, $2, CASE WHEN $1 = 'primary' THEN $2 END) RETURNING id",
        )
        .bind(kind)
        .bind(name)
        .fetch_one(pool)
    };
    let base = role("base", "Temel").await.unwrap();
    let primary = role("primary", "Sistem Uzmanı").await.unwrap();
    let additional = role("additional", "Nöbet Ekibi").await.unwrap();

    const ROLE: &str = "INSERT INTO role_entitlements VALUES ($1, $2)";
    const DEPT: &str = "INSERT INTO department_entitlements VALUES ($1, $2)";
    let link = |sql: &'static str, owner: i64, item: i64| {
        sqlx::query(sql).bind(owner).bind(item).execute(pool)
    };
    link(ROLE, base, m.gg_internet).await.unwrap();
    link(DEPT, bt, m.gg_bt_paylasim).await.unwrap();
    link(DEPT, ankara, m.gg_ankara_yazici).await.unwrap();
    link(ROLE, primary, m.gg_sistem_uzmanlari).await.unwrap();
    link(ROLE, additional, m.gg_nobet).await.unwrap();
    sqlx::query(
        "INSERT INTO role_target_settings (role_id, target_system_id, container_item_id, upn_suffix) \
         VALUES ($1, $2, $3, 'example.local')",
    )
    .bind(primary)
    .bind(m.ad)
    .bind(m.sistem_uzmanlari_ou)
    .execute(pool)
    .await
    .unwrap();
    (ankara, bt, primary, additional)
}

// Lab AD baglanti ayarini Yapilandirma sayfasinin yazacagi gibi DB'ye yazar
// (parola AEAD ile sifreli) ve AdConfig doner.
pub async fn configure_lab_ad(
    pool: &PgPool,
    key: &[u8; crate::crypto::KEY_LEN],
    url: &str,
    bind_dn: &str,
    password: &str,
    ca_file: &str,
) -> crate::ad::AdConfig {
    sqlx::query(
        "UPDATE app_settings SET ad_host = $1, ad_bind_dn = $2, ad_service_password_enc = $3 WHERE id = TRUE",
    )
    .bind(url)
    .bind(bind_dn)
    .bind(crate::crypto::encrypt(key, password.as_bytes()))
    .execute(pool)
    .await
    .expect("AD ayarı yazılamadı");
    crate::ad::AdConfig {
        urls: crate::ad::parse_urls(url),
        bind_dn: bind_dn.to_string(),
        password: password.to_string(),
        ca_file: ca_file.to_string(),
    }
}

// Ornek modeli lab AD'nin gercek katalog GUID'lerine baglar: birincil rolun
// OU'su SistemUzmanlari (seed.sh'de yok → Personel altina eklenir), grubu GG-VPN.
pub async fn point_model_at_real_catalog(
    pool: &PgPool,
    seed: &ExampleModel,
    cfg: &crate::ad::AdConfig,
) {
    sync_real_catalog(pool, seed.ad, cfg).await;
    let real = |name: &'static str| {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM catalog_items WHERE display_name = $1 AND external_id NOT LIKE 'GG-%' \
             AND external_id NOT IN ('Personel', 'SistemUzmanlari') LIMIT 1",
        )
        .bind(name)
        .fetch_one(pool)
    };
    let ou = real("SistemUzmanlari")
        .await
        .expect("gerçek OU kataloğa girmeli");
    let vpn = real("GG-VPN").await.expect("gerçek grup kataloğa girmeli");
    sqlx::query("UPDATE role_target_settings SET container_item_id = $1")
        .bind(ou)
        .execute(pool)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "DELETE FROM role_entitlements; DELETE FROM department_entitlements".to_string(),
    ))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO role_entitlements (role_id, catalog_item_id) SELECT id, $1 FROM roles WHERE kind = 'primary'")
        .bind(vpn)
        .execute(pool)
        .await
        .unwrap();
}

// Lab kapsamini (SistemUzmanlari OU'su dahil; seed.sh'de yok, burada eklenir)
// gercek GUID'leriyle kataloga yazar.
async fn sync_real_catalog(pool: &PgPool, ad: i64, cfg: &crate::ad::AdConfig) {
    let scope = crate::ad::ManagedScope {
        user_ous: vec![
            "OU=Personel,DC=opensicil,DC=lab".to_string(),
            "OU=SistemUzmanlari,OU=Personel,DC=opensicil,DC=lab".to_string(),
        ],
        passive_ou: Some("OU=Pasif,OU=Personel,DC=opensicil,DC=lab".to_string()),
        group_ous: vec!["OU=Gruplar,DC=opensicil,DC=lab".to_string()],
    };
    let mut ldap = crate::ad::connect(cfg).await.expect("lab AD");
    let _ = ldap
        .add(
            "OU=SistemUzmanlari,OU=Personel,DC=opensicil,DC=lab",
            vec![(
                "objectClass",
                std::collections::HashSet::from(["organizationalUnit"]),
            )],
        )
        .await;
    let checks = crate::ad::startup_checks(&mut ldap, &scope).await.unwrap();
    let snapshot = crate::ad::read_catalog(&mut ldap, &scope, &checks)
        .await
        .unwrap();
    crate::catalog::sync_snapshot(pool, ad, &snapshot)
        .await
        .unwrap();
    ldap.unbind().await.ok();
}

// Onceki (basarisiz) test kosularindan kalan lab hesaplarini temizler.
pub async fn delete_lab_accounts(cfg: &crate::ad::AdConfig, sam_pattern: &str) {
    let mut ldap = crate::ad::connect(cfg).await.expect("lab AD");
    let base = crate::ad::base_dn(&mut ldap).await.unwrap();
    let filter = format!("(&(objectClass=user)(sAMAccountName={sam_pattern}))");
    let found = crate::ad::search(&mut ldap, &base, ldap3::Scope::Subtree, &filter, &["cn"])
        .await
        .unwrap();
    for entry in found {
        let _ = ldap.delete(&entry.dn).await;
    }
    ldap.unbind().await.ok();
}

// Backend'in enqueue'su ayri crate'te; testte sahip roluyle dogrudan yazilir.
pub async fn enqueue(pool: &PgPool, identity: i64, target: i64, priority: i16) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO jobs (identity_id, target_system_id, priority) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(identity)
    .bind(target)
    .bind(priority)
    .fetch_one(pool)
    .await
    .expect("iş kuyruğa yazılamadı")
}
