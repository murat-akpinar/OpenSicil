// --- START FEATURE: ad-connector ---
// Katalog eslemesi (docs/03 Katalog; ADR-015 yalnizca worker yazar): hedeften
// okunan OU ve gruplar GUID'iyle upsert edilir, ad ve DN yalnizca gosterim;
// bu turda gorulmeyen oge silinmez, "kayip" isaretlenir (roller bozulmaz),
// yeniden gorulunce isaret kalkar.

use sqlx::PgPool;

use crate::ad::Snapshot;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncCounts {
    pub ous: usize,
    pub groups: usize,
    pub marked_missing: u64,
}

const UPSERT_SQL: &str = "INSERT INTO catalog_items \
    (target_system_id, kind, external_id, display_name, location, sid) \
    VALUES ($1, $2, $3, $4, $5, $6) \
    ON CONFLICT (target_system_id, external_id) DO UPDATE SET \
    display_name = EXCLUDED.display_name, location = EXCLUDED.location, sid = EXCLUDED.sid, \
    last_seen_at = now(), missing_since = NULL";

pub async fn sync_snapshot(
    pool: &PgPool,
    target_system_id: i64,
    snapshot: &Snapshot,
) -> Result<SyncCounts, sqlx::Error> {
    let mut tx = pool.begin().await?;
    for ou in &snapshot.ous {
        sqlx::query(UPSERT_SQL)
            .bind(target_system_id)
            .bind("ou")
            .bind(&ou.guid)
            .bind(&ou.name)
            .bind(&ou.dn)
            .bind(None::<String>)
            .execute(&mut *tx)
            .await?;
    }
    for group in &snapshot.groups {
        sqlx::query(UPSERT_SQL)
            .bind(target_system_id)
            .bind("group")
            .bind(&group.guid)
            .bind(&group.name)
            .bind(&group.dn)
            .bind(Some(&group.sid))
            .execute(&mut *tx)
            .await?;
    }
    // now() transaction boyunca sabittir: bu turda upsert edilen her satirin
    // last_seen_at'i tam now(); daha eski olanlar bu turda gorulmemistir.
    let marked = sqlx::query(
        "UPDATE catalog_items SET missing_since = now() \
         WHERE target_system_id = $1 AND missing_since IS NULL AND last_seen_at < now()",
    )
    .bind(target_system_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(SyncCounts {
        ous: snapshot.ous.len(),
        groups: snapshot.groups.len(),
        marked_missing: marked,
    })
}
// --- END FEATURE: ad-connector ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ad::{DirectoryGroup, DirectoryOu};
    use crate::test_support;

    fn group(name: &str, guid: &str) -> DirectoryGroup {
        DirectoryGroup {
            dn: format!("CN={name},OU=Gruplar,DC=x"),
            name: name.to_string(),
            guid: guid.to_string(),
            sid: "S-1-5-21-1-2-3-1105".to_string(),
            admin_count: false,
        }
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn sync_upserts_marks_missing_and_revives() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let ad: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let first = Snapshot {
            ous: vec![DirectoryOu {
                dn: "OU=Personel,DC=x".into(),
                name: "Personel".into(),
                guid: "ou-1".into(),
            }],
            groups: vec![group("GG-Internet", "g-1"), group("GG-VPN", "g-2")],
            forbidden: vec![],
        };
        assert_eq!(
            sync_snapshot(&pool, ad, &first).await.unwrap(),
            SyncCounts {
                ous: 1,
                groups: 2,
                marked_missing: 0
            }
        );

        let second = Snapshot {
            ous: first.ous.clone(),
            groups: vec![group("GG-Internet-Yeni-Ad", "g-1")],
            forbidden: vec![],
        };
        assert_eq!(
            sync_snapshot(&pool, ad, &second)
                .await
                .unwrap()
                .marked_missing,
            1,
            "GG-VPN kayıp"
        );
        let (name, missing): (String, bool) = sqlx::query_as(
            "SELECT display_name, missing_since IS NOT NULL FROM catalog_items WHERE external_id = 'g-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (name.as_str(), missing),
            ("GG-Internet-Yeni-Ad", false),
            "ad güncellenir, GUID sabit"
        );

        let counts = sync_snapshot(&pool, ad, &first).await.unwrap();
        assert_eq!(counts.marked_missing, 0);
        let revived: bool = sqlx::query_scalar(
            "SELECT missing_since IS NULL FROM catalog_items WHERE external_id = 'g-2'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(revived, "yeniden görülen öğe kayıp olmaktan çıkar");
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM catalog_items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total, 3, "öğe silinmez");

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
