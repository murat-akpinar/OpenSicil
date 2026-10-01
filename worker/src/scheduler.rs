// --- START FEATURE: scheduler ---
// Sorguya dayali zamanlayici (ADR-028, ADR-038): her tikte "turetilen durum ≠
// hesap baglantisinda uygulanan durum" olan yonetilen baglantilar icin is acar.
// Kacirilan gecisler (worker kapaliyken gelen baslangic/bitis) bir sonraki tikte
// birden yakalanir. Operator islemleri isi zaten hemen acar; zamanlayici yalnizca
// zamanin degistirdigi farki yakalar. Advisory lock: iki worker kopyasi ayni
// tikte ayni taramayi yapmaz (tekil indeks zaten mukerrer isi engeller).

use sqlx::PgPool;

use crate::desired_state::{lifecycle_state, Clock, Date, LifecycleState, Timeline};
use crate::engine::state_name;

// pg_advisory_lock anahtari ("opensici" baytlari); sabit, projeye ozel.
const LOCK_KEY: i64 = 0x6f70_656e_7369_6369;
// ADR-016: tarihli gecis tek kimlik islemiyle ayni oncelikte.
const TRANSITION_PRIORITY: i16 = 1;

type LinkRow = (
    i64,
    i64,
    Option<String>,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    bool,
    bool,
    Option<i64>,
    i64,
    String,
);

// Doner: bu tikte acilan is sayisi. Kilit alinamadiysa (baska kopya tariyor) 0.
pub async fn tick(pool: &PgPool, time_zone: &str) -> Result<usize, String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(LOCK_KEY)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    if !locked {
        return Ok(0);
    }
    let rows: Vec<LinkRow> = sqlx::query_as(
        "SELECT l.identity_id, l.target_system_id, l.applied_state, \
         to_char(i.start_date, 'YYYY-MM-DD'), EXTRACT(EPOCH FROM i.end_at)::bigint, \
         to_char(i.suspension_start, 'YYYY-MM-DD'), to_char(i.suspension_end, 'YYYY-MM-DD'), \
         i.cancelled, i.emergency_departure, EXTRACT(EPOCH FROM i.deleted_at)::bigint, \
         EXTRACT(EPOCH FROM now())::bigint, to_char((now() AT TIME ZONE $1)::date, 'YYYY-MM-DD') \
         FROM account_links l JOIN identities i ON i.id = l.identity_id \
         WHERE l.mode = 'managed' ORDER BY l.identity_id, l.target_system_id",
    )
    .bind(time_zone)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| format!("bağlantılar okunamadı: {e}"))?;
    let mut opened = expire_additional_roles(&mut tx, time_zone).await?;
    for row in rows {
        let (identity_id, target_system_id, applied) = (row.0, row.1, row.2.as_deref());
        let state = derived_state(&row)?;
        if !needs_job(state, applied) {
            continue;
        }
        let inserted = sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority) VALUES ($1, $2, $3) \
             ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' DO NOTHING",
        )
        .bind(identity_id)
        .bind(target_system_id)
        .bind(TRANSITION_PRIORITY)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("iş açılamadı: {e}"))?;
        opened += inserted.rows_affected() as usize;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(opened)
}

// ADR-020/038: bitisi gecmis ek rol atamasi kaldirilir, denetim kaydina "suresi doldu"
// yazilir ve kimlik icin her hedefe is acilir (gruplar sonraki iste duser). Tek
// ifade: veri degistiren CTE'ler bir kez calisir, sonuc acilan is sayisidir.
async fn expire_additional_roles(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    time_zone: &str,
) -> Result<usize, String> {
    let opened = sqlx::query(
        "WITH expired AS ( \
           DELETE FROM identity_additional_roles \
           WHERE ends_on < (now() AT TIME ZONE $1)::date RETURNING identity_id, role_id), \
         logged AS ( \
           INSERT INTO audit_log (event_type, identity_id, detail) \
           SELECT 'identity.role_expired', identity_id, \
                  jsonb_build_object('role_id', role_id, 'reason', 'süresi doldu') \
           FROM expired RETURNING identity_id) \
         INSERT INTO jobs (identity_id, target_system_id, priority) \
         SELECT DISTINCT e.identity_id, t.id, $2 FROM expired e CROSS JOIN target_systems t \
         ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' DO NOTHING",
    )
    .bind(time_zone)
    .bind(TRANSITION_PRIORITY)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("süresi dolan ek roller işlenemedi: {e}"))?;
    Ok(opened.rows_affected() as usize)
}

// Silme (saklama suresi, ADR-024) 3c'de gelir; o gune kadar `silindi` icin is acilmaz,
// yoksa her dakika bos is uretilirdi.
pub fn needs_job(state: LifecycleState, applied: Option<&str>) -> bool {
    state != LifecycleState::Deleted && applied != Some(state_name(state))
}

fn derived_state(row: &LinkRow) -> Result<LifecycleState, String> {
    let date = |s: &str| Date::from_iso(s).ok_or_else(|| format!("tarih çözümlenemedi: {s}"));
    let timeline = Timeline {
        start_date: date(&row.3)?,
        end_at: row.4,
        suspension_start: row.5.as_deref().map(date).transpose()?,
        suspension_end: row.6.as_deref().map(date).transpose()?,
        cancelled: row.7,
        emergency_departure: row.8,
        deleted_at: row.9,
    };
    let clock = Clock {
        now: row.10,
        today: date(&row.11)?,
    };
    Ok(lifecycle_state(&timeline, &clock))
}
// --- END FEATURE: scheduler ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_needed_only_on_mismatch_and_never_for_deleted() {
        use LifecycleState::*;
        assert!(needs_job(Active, Some("pending")));
        assert!(needs_job(Active, None));
        assert!(!needs_job(Active, Some("active")));
        assert!(needs_job(Departed, Some("active")));
        assert!(!needs_job(Deleted, Some("departed")));
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn tick_opens_jobs_for_mismatched_managed_links_once() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        let link = |identity: i64, mode: &'static str, applied: Option<&'static str>| {
            sqlx::query(
                "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, applied_state) \
                 VALUES ($1, $2, $3, 'provisioned', $4, $5)",
            )
            .bind(identity)
            .bind(seed.ad)
            .bind(format!("guid-{identity}"))
            .bind(mode)
            .bind(applied)
            .execute(&pool)
        };
        // Ayşe: aktif ama hedefte "bekliyor" → iş; Ali: gözlem modu → zamanlayıcı dokunmaz.
        link(seed.identity, "managed", Some("pending"))
            .await
            .unwrap();
        link(seed.other_identity, "observed", Some("pending"))
            .await
            .unwrap();

        assert_eq!(tick(&pool, "Europe/Istanbul").await.unwrap(), 1);
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            0,
            "açık iş varken yenisi açılmaz"
        );
        let (identity, priority): (i64, i16) =
            sqlx::query_as("SELECT identity_id, priority FROM jobs")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((identity, priority), (seed.identity, TRANSITION_PRIORITY));

        // Is bitti ve uygulanan durum eslesti → tik bos; ayrilis girilince yeniden is.
        sqlx::query("UPDATE jobs SET status = 'succeeded'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE account_links SET applied_state = 'active' WHERE identity_id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(tick(&pool, "Europe/Istanbul").await.unwrap(), 0);
        sqlx::query("UPDATE identities SET end_at = now() - interval '1 minute' WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(tick(&pool, "Europe/Istanbul").await.unwrap(), 1);

        // ADR-038: `bekliyor` iken bitisi gecmis kimlik tek tikte tek is uretir (ayrilis).
        for sql in [
            "UPDATE account_links SET mode = 'managed' WHERE identity_id = $1",
            "UPDATE identities SET start_date = current_date + 1, end_at = now() - interval '1 minute' \
             WHERE id = $1",
        ] {
            sqlx::query(sql)
                .bind(seed.other_identity)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert_eq!(tick(&pool, "Europe/Istanbul").await.unwrap(), 1);
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND status <> 'succeeded'",
        )
        .bind(seed.other_identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-020: bitisi gecmis ek rol tek tikte kalkar, denetime yazilir, her hedefe is acilir;
    // bitisi gelmemis atama durur.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn tick_expires_additional_roles_once_with_audit_and_jobs() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        let nobet: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE kind = 'additional'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) \
             VALUES ($1, $2, current_date - 1)",
        )
        .bind(seed.other_identity)
        .bind(nobet)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            2,
            "iki hedefe iş"
        );
        let remaining: Vec<i64> =
            sqlx::query_scalar("SELECT identity_id FROM identity_additional_roles")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            remaining,
            vec![seed.identity],
            "14 gün sonrası duran atama kalır"
        );
        let (events, jobs): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM audit_log WHERE event_type = 'identity.role_expired' \
                     AND identity_id = $1 AND detail->>'reason' = 'süresi doldu'), \
                    (SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND status = 'queued')",
        )
        .bind(seed.other_identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((events, jobs), (1, 2));
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            0,
            "ikinci tik boş"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
