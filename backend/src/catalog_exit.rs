// --- START FEATURE: missing-catalog-exit ---
// Kayip katalog ogesinin cikisi (ADR-127). Tazeleme hicbir seyi silmez; kayip
// satirin cikisi operatorun iki eylemidir, kod kendiliginden devretmez ya da silmez.
//
// 1. Canli ikizine devret: ayni hedef + ayni tur + ayni DN'de TEK canli oge varsa.
//    Kayip ogeye bakan butun baglar canliya tasinir (zaten varsa mukerrer eklenmez),
//    sonra kayip satir silinir.
// 2. Katalogdan kaldir: her kayip satirda. O ogeye bakan baglar ve satir silinir.
//
// Hedefe hicbir sey yazilmaz: silinmis grubun hakki AD'de zaten yok.
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub id: i64,
    pub target_id: i64,
    pub display_name: String,
    pub location: String,
    /// Tek canli ikiz (ayni hedef, tur ve DN); yoksa ya da birden fazlaysa None
    pub twin: Option<i64>,
    /// Bu ogeye bakan rol ve departman adlari (ekran kaldirmadan once yazar)
    pub roles: Vec<String>,
    pub departments: Vec<String>,
}

const MISSING_SQL: &str = "SELECT c.id, c.target_system_id, c.display_name, \
    COALESCE(c.location, ''), \
    (SELECT max(t.id) FROM catalog_items t WHERE t.target_system_id = c.target_system_id \
       AND t.kind = c.kind AND t.location IS NOT DISTINCT FROM c.location \
       AND t.missing_since IS NULL HAVING count(*) = 1), \
    ARRAY(SELECT DISTINCT r.name FROM roles r WHERE r.id IN ( \
       SELECT role_id FROM role_entitlements WHERE catalog_item_id = c.id \
       UNION SELECT role_id FROM role_target_settings WHERE container_item_id = c.id) \
       ORDER BY r.name), \
    ARRAY(SELECT DISTINCT d.name FROM departments d WHERE d.id IN ( \
       SELECT department_id FROM department_entitlements WHERE catalog_item_id = c.id \
       UNION SELECT department_id FROM department_target_settings WHERE container_item_id = c.id) \
       ORDER BY d.name) \
    FROM catalog_items c WHERE c.missing_since IS NOT NULL \
    ORDER BY c.target_system_id, c.kind, c.display_name, c.id";

type MissingRow = (
    i64,
    i64,
    String,
    String,
    Option<i64>,
    Vec<String>,
    Vec<String>,
);

pub async fn missing(pool: &PgPool) -> Result<Vec<Missing>, sqlx::Error> {
    let rows: Vec<MissingRow> = sqlx::query_as(MISSING_SQL).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, target_id, display_name, location, twin, roles, departments)| Missing {
                id,
                target_id,
                display_name,
                location,
                twin,
                roles,
                departments,
            },
        )
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Oge yok ya da kayip degil (canli satira eylem yok)
    NotMissing,
    /// Devir icin tek canli ikiz yok
    NoTwin,
    HandedOver {
        twin: i64,
    },
    Removed {
        roles: Vec<String>,
        departments: Vec<String>,
    },
}

// Satir kilitlenir: ayni anda tazeleme ogeyi "geri buldu" diye canliya ceviremez.
async fn lock_missing(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<bool, sqlx::Error> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM catalog_items WHERE id = $1 AND missing_since IS NOT NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(found.is_some())
}

/// Baglar tasinir; ikizde zaten olan bag ikinci kez eklenmez (`ON CONFLICT`).
const HAND_OVER_SQL: [&str; 7] = [
    "INSERT INTO role_entitlements (role_id, catalog_item_id) \
     SELECT role_id, $2 FROM role_entitlements WHERE catalog_item_id = $1 ON CONFLICT DO NOTHING",
    "DELETE FROM role_entitlements WHERE catalog_item_id = $1",
    "INSERT INTO department_entitlements (department_id, catalog_item_id) \
     SELECT department_id, $2 FROM department_entitlements WHERE catalog_item_id = $1 \
     ON CONFLICT DO NOTHING",
    "DELETE FROM department_entitlements WHERE catalog_item_id = $1",
    "UPDATE role_target_settings SET container_item_id = $2 WHERE container_item_id = $1",
    "UPDATE department_target_settings SET container_item_id = $2 WHERE container_item_id = $1",
    "UPDATE target_systems SET default_container_item_id = $2 WHERE default_container_item_id = $1",
];

/// Kaldirmada baglar duser; konteyner ayari "ayarlanmamis"a doner.
const REMOVE_SQL: [&str; 5] = [
    "DELETE FROM role_entitlements WHERE catalog_item_id = $1",
    "DELETE FROM department_entitlements WHERE catalog_item_id = $1",
    "UPDATE role_target_settings SET container_item_id = NULL WHERE container_item_id = $1",
    "UPDATE department_target_settings SET container_item_id = NULL WHERE container_item_id = $1",
    "UPDATE target_systems SET default_container_item_id = NULL WHERE default_container_item_id = $1",
];

pub async fn hand_over(pool: &PgPool, id: i64) -> Result<Outcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !lock_missing(&mut tx, id).await? {
        return Ok(Outcome::NotMissing);
    }
    let twin = missing(pool)
        .await?
        .into_iter()
        .find(|m| m.id == id)
        .and_then(|m| m.twin);
    let Some(twin) = twin else {
        return Ok(Outcome::NoTwin);
    };
    for sql in HAND_OVER_SQL {
        sqlx::query(sql)
            .bind(id)
            .bind(twin)
            .execute(&mut *tx)
            .await?;
    }
    delete_item(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Outcome::HandedOver { twin })
}

pub async fn remove(pool: &PgPool, id: i64) -> Result<Outcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !lock_missing(&mut tx, id).await? {
        return Ok(Outcome::NotMissing);
    }
    let (roles, departments) = missing(pool)
        .await?
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| (m.roles, m.departments))
        .unwrap_or_default();
    for sql in REMOVE_SQL {
        sqlx::query(sql).bind(id).execute(&mut *tx).await?;
    }
    delete_item(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Outcome::Removed { roles, departments })
}

async fn delete_item(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM catalog_items WHERE id = $1 AND missing_since IS NOT NULL")
        .bind(id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
// --- END FEATURE: missing-catalog-exit ---

#[cfg(test)]
mod tests {
    use super::*;

    async fn lost_twin(pool: &PgPool, target: i64, kind: &str, name: &str, location: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO catalog_items (target_system_id, kind, external_id, display_name, \
             location, missing_since) VALUES ($1, $2, $3, $4, $5, now()) RETURNING id",
        )
        .bind(target)
        .bind(kind)
        .bind(format!("old-{name}"))
        .bind(name)
        .bind(location)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn count(pool: &PgPool, sql: &'static str, id: i64) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // ADR-127: devir baglari tasir ve mukerrer eklemez; kaldirma baglari dusurur;
    // canli satirda ve ikizi belirsiz satirda eylem sonuc vermez.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn a_missing_item_is_handed_over_to_its_single_twin_or_removed() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let cat = crate::test_support::seed_example_catalog(&pool).await;
        let (role, dept): (i64, i64) =
            sqlx::query_as("SELECT primary_role_id, department_id FROM identities LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let groups = "OU=Gruplar,DC=example,DC=local";
        let old_vpn = lost_twin(
            &pool,
            cat.ad,
            "group",
            "GG-VPN",
            &format!("CN=GG-VPN,{groups}"),
        )
        .await;
        let old_ou = lost_twin(
            &pool,
            cat.ad,
            "ou",
            "Personel",
            "OU=Personel,DC=example,DC=local",
        )
        .await;
        let gone = lost_twin(&pool, cat.ad, "group", "GG-Eski", "CN=GG-Eski,OU=x").await;
        // Rol hem eski hem canli VPN'e bakiyor (bugunku durum), departman yalnizca eskiye
        for (sql, owner, item) in [
            ("INSERT INTO role_entitlements (role_id, catalog_item_id) VALUES ($1, $2)", role, old_vpn),
            ("INSERT INTO role_entitlements (role_id, catalog_item_id) VALUES ($1, $2)", role, cat.gg_vpn),
            ("INSERT INTO role_entitlements (role_id, catalog_item_id) VALUES ($1, $2)", role, gone),
            ("INSERT INTO department_entitlements (department_id, catalog_item_id) VALUES ($1, $2)", dept, old_vpn),
        ] {
            sqlx::query(sql).bind(owner).bind(item).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO role_target_settings (role_id, target_system_id, container_item_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(role)
        .bind(cat.ad)
        .bind(old_ou)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE target_systems SET default_container_item_id = $1 WHERE id = $2")
            .bind(old_ou)
            .bind(cat.ad)
            .execute(&pool)
            .await
            .unwrap();

        let listed = missing(&pool).await.unwrap();
        let vpn = listed.iter().find(|m| m.id == old_vpn).unwrap();
        assert_eq!(vpn.twin, Some(cat.gg_vpn));
        assert_eq!((vpn.roles.len(), vpn.departments.len()), (1, 1));
        assert_eq!(listed.iter().find(|m| m.id == gone).unwrap().twin, None);

        // Canli satirda eylem yok; ikizi olmayan devredilemez
        assert_eq!(
            hand_over(&pool, cat.gg_vpn).await.unwrap(),
            Outcome::NotMissing
        );
        assert_eq!(
            remove(&pool, cat.gg_vpn).await.unwrap(),
            Outcome::NotMissing
        );
        assert_eq!(hand_over(&pool, gone).await.unwrap(), Outcome::NoTwin);

        assert_eq!(
            hand_over(&pool, old_vpn).await.unwrap(),
            Outcome::HandedOver { twin: cat.gg_vpn }
        );
        let role_vpn = "SELECT count(*) FROM role_entitlements WHERE catalog_item_id = $1";
        assert_eq!(
            count(&pool, role_vpn, cat.gg_vpn).await,
            1,
            "mükerrer eklenmez"
        );
        let dept_vpn = "SELECT count(*) FROM department_entitlements WHERE catalog_item_id = $1";
        assert_eq!(
            count(&pool, dept_vpn, cat.gg_vpn).await,
            1,
            "departman bağı taşındı"
        );
        assert_eq!(
            hand_over(&pool, old_ou).await.unwrap(),
            Outcome::HandedOver {
                twin: cat.personel_ou
            }
        );
        let settings = "SELECT count(*) FROM role_target_settings WHERE container_item_id = $1";
        assert_eq!(count(&pool, settings, cat.personel_ou).await, 1);
        let default = "SELECT count(*) FROM target_systems WHERE default_container_item_id = $1";
        assert_eq!(count(&pool, default, cat.personel_ou).await, 1);

        assert!(matches!(
            remove(&pool, gone).await.unwrap(),
            Outcome::Removed { roles, .. } if roles.len() == 1
        ));
        assert_eq!(count(&pool, role_vpn, gone).await, 0);
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog_items WHERE missing_since IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(left, 0, "üç kayıp satır da çıktı");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
