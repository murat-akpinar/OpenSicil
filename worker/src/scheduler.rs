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
// ADR-036: gosterilmeyen ilk parola ve cevapsiz istek bu sure sonunda kapanir.
const FIRST_PASSWORD_TTL: &str = "10 minutes";
const FIRST_PASSWORD_UNSHOWN: &str =
    "parola 10 dakika içinde gösterilmedi ve silindi; yeniden isteyin";
const FIRST_PASSWORD_UNANSWERED: &str =
    "worker 10 dakika içinde yanıt vermedi; işler listesine bakın ve yeniden isteyin";

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
    opened += open_password_reset_jobs(&mut tx).await?;
    opened += open_retention_jobs(&mut tx).await?;
    expire_first_passwords(&mut tx).await?;
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

// --- START FEATURE: first-password ---
// ADR-036/085: gosterilmeyen sifreli parola 10 dk sonra silinir; worker'in 10 dk icinde
// cevaplamadigi istek de kapanir ki durum sayfasi sonsuza dek beklemesin.
async fn expire_first_passwords(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE first_passwords SET password_enc = NULL, error = $1 \
         WHERE password_enc IS NOT NULL AND issued_at < now() - $2::interval",
    )
    .bind(FIRST_PASSWORD_UNSHOWN)
    .bind(FIRST_PASSWORD_TTL)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("gösterilmeyen ilk parolalar silinemedi: {e}"))?;
    sqlx::query(
        "UPDATE first_passwords SET error = $1 \
         WHERE issued_at IS NULL AND error IS NULL AND requested_at < now() - $2::interval",
    )
    .bind(FIRST_PASSWORD_UNANSWERED)
    .bind(FIRST_PASSWORD_TTL)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| format!("cevapsız ilk parola istekleri kapatılamadı: {e}"))
}
// --- END FEATURE: first-password ---

// ADR-033: ayrilistan G gun sonra (acilde hemen) parola penceresi acilir; bu bir
// durum gecisi degildir, applied_state esitken de is gerekir. Iptal (dogrulanmis
// ya da henuz bakilmamis) parolaya dokunmaz; reddedilen iptal ayrilis gibidir.
async fn open_password_reset_jobs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<usize, String> {
    let opened = sqlx::query(
        "INSERT INTO jobs (identity_id, target_system_id, priority) \
         SELECT l.identity_id, l.target_system_id, $1 FROM account_links l \
         JOIN identities i ON i.id = l.identity_id \
         JOIN target_systems t ON t.id = l.target_system_id \
         WHERE l.mode = 'managed' AND NOT l.password_reset_at_departure \
           AND l.deleted_by_us_at IS NULL AND i.deleted_at IS NULL \
           AND (NOT i.cancelled OR l.verified_unused = FALSE) \
           AND i.end_at IS NOT NULL AND i.end_at <= now() \
           AND (i.emergency_departure \
                OR i.end_at + make_interval(days => t.password_reset_delay_days) <= now()) \
         ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' DO NOTHING",
    )
    .bind(TRANSITION_PRIORITY)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("parola penceresi işleri açılamadı: {e}"))?;
    Ok(opened.rows_affected() as usize)
}

// ADR-024/028: saklama suresi dolan ayrilmis hesap silinir; durum adi degismediginden
// (ayrildi → ayrildi) ayri sorgu gerekir. Onay isteyen hedefte (Zimbra) onay yoksa is
// acilmaz: "silinmeyi bekliyor" listesi onu gosterir, her dakika bos is uretilmez.
async fn open_retention_jobs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<usize, String> {
    let opened = sqlx::query(
        "INSERT INTO jobs (identity_id, target_system_id, priority) \
         SELECT l.identity_id, l.target_system_id, $1 FROM account_links l \
         JOIN identities i ON i.id = l.identity_id \
         JOIN target_systems t ON t.id = l.target_system_id \
         WHERE l.mode = 'managed' AND l.deleted_by_us_at IS NULL AND i.deleted_at IS NULL \
           AND i.end_at IS NOT NULL \
           AND i.end_at + make_interval(days => t.retention_days) <= now() \
           AND (NOT t.delete_requires_approval OR l.deletion_approved) \
         ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' DO NOTHING",
    )
    .bind(TRANSITION_PRIORITY)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("saklama süresi işleri açılamadı: {e}"))?;
    Ok(opened.rows_affected() as usize)
}

/// ADR-051/099 gece mutabakati: kurulum saat diliminde bu saatten sonra, o gun icin
/// henuz acilmamissa AD hedefine katalog yenileme + mutabakat istegi yazilir.
pub const NIGHTLY_SCAN_AFTER: &str = "02:00";

/// Sorguya dayali (ADR-028): worker gece kapaliysa acildigi ilk tikte acilir; ayni
/// gun ikinci kez acilmaz (o saatten sonra yazilmis bir istek varsa — operatorun
/// "Yeniden tara"si da sayilir). AD yapilandirilmamissa hic yazilmaz; acik is
/// varken kismi tekil indeks ikinciyi engeller. Doner: yazilan istek sayisi.
pub async fn open_nightly_scans(
    pool: &PgPool,
    time_zone: &str,
    after: &str,
) -> Result<usize, String> {
    let opened = sqlx::query(
        "INSERT INTO read_jobs (kind, target_system_id) \
         SELECT k.kind, t.id FROM target_systems t \
         CROSS JOIN (VALUES ('catalog_refresh'), ('reconcile')) AS k(kind) \
         WHERE t.kind = 'ad' \
           AND (SELECT ad_host <> '' FROM app_settings WHERE id = TRUE) \
           AND (now() AT TIME ZONE $1)::time >= $2::time \
           AND NOT EXISTS (SELECT 1 FROM read_jobs r WHERE r.kind = k.kind \
                 AND r.target_system_id = t.id \
                 AND r.created_at >= ((now() AT TIME ZONE $1)::date + $2::time) AT TIME ZONE $1) \
         ON CONFLICT DO NOTHING",
    )
    .bind(time_zone)
    .bind(after)
    .execute(pool)
    .await
    .map_err(|e| format!("gece mutabakatı açılamadı: {e}"))?;
    Ok(opened.rows_affected() as usize)
}

// `silindi` icin is acilmaz: silme zaten uygulanmistir, her dakika bos is uretilirdi.
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

    // ADR-033: pencere dolunca (G gun) parola isi acilir; dolmadan acilmaz; isaret
    // yazildiktan sonra bir daha acilmaz; acil ayrilista hemen.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn tick_opens_password_reset_job_when_window_passes() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        for (identity, days_ago) in [(seed.identity, 8), (seed.other_identity, 2)] {
            sqlx::query(
                "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, applied_state) \
                 VALUES ($1, $2, $3, 'provisioned', 'managed', 'departed')",
            )
            .bind(identity)
            .bind(seed.ad)
            .bind(format!("guid-{identity}"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "UPDATE identities SET end_at = now() - make_interval(days => $2) WHERE id = $1",
            )
            .bind(identity)
            .bind(days_ago)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            1,
            "yalnızca 8 gün önce ayrılan (G = 7)"
        );
        let queued: i64 = sqlx::query_scalar("SELECT identity_id FROM jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(queued, seed.identity);
        sqlx::query("UPDATE jobs SET status = 'succeeded'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE account_links SET password_reset_at_departure = TRUE WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            0,
            "işaret var, diğerinin penceresi dolmadı"
        );
        sqlx::query("UPDATE identities SET emergency_departure = TRUE WHERE id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            1,
            "acil: hemen"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-036/085: 10 dk gosterilmeyen parola silinir, cevapsiz istek kapanir; taze ve
    // gosterilmis satirlara dokunulmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn tick_expires_unshown_and_unanswered_first_passwords() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        let insert = |age: &'static str, issued: bool, enc: bool, shown: bool| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO first_passwords (identity_id, target_system_id, requested_by, \
                     requested_at, issued_at, password_enc, shown_at) \
                     VALUES ($1, $2, 'ik', now() - $3::interval, \
                       CASE WHEN $4 THEN now() - $3::interval END, \
                       CASE WHEN $5 THEN '\\x01aa'::bytea END, \
                       CASE WHEN $6 THEN now() END) RETURNING id",
                )
                .bind(seed.identity)
                .bind(seed.ad)
                .bind(age)
                .bind(issued)
                .bind(enc)
                .bind(shown)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let fresh = insert("1 minute", true, true, false).await;
        let stale = insert("11 minutes", true, true, false).await;
        let unanswered = insert("11 minutes", false, false, false).await;
        let shown = insert("11 minutes", true, false, true).await;
        tick(&pool, "Europe/Istanbul").await.unwrap();
        let state = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (bool, Option<String>)>(
                    "SELECT password_enc IS NOT NULL, error FROM first_passwords WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(state(fresh).await, (true, None));
        assert_eq!(
            state(stale).await,
            (false, Some(FIRST_PASSWORD_UNSHOWN.to_string()))
        );
        assert_eq!(
            state(unanswered).await,
            (false, Some(FIRST_PASSWORD_UNANSWERED.to_string()))
        );
        assert_eq!(state(shown).await, (false, None));

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-024/028: saklama dolunca silme isi; onay isteyen hedefte onay yoksa acilmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn tick_opens_deletion_job_after_retention_unless_approval_pending() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, \
             applied_state, password_reset_at_departure) \
             VALUES ($1, $2, 'guid-1', 'provisioned', 'managed', 'departed', TRUE)",
        )
        .bind(seed.identity)
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE identities SET end_at = now() - interval '100 days' WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            1,
            "90 gün saklama doldu: silme işi"
        );
        sqlx::query("UPDATE jobs SET status = 'succeeded'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE target_systems SET delete_requires_approval = TRUE WHERE id = $1")
            .bind(seed.ad)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            0,
            "onay isteyen hedefte onaysız silme işi açılmaz"
        );
        sqlx::query("UPDATE account_links SET deletion_approved = TRUE")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            tick(&pool, "Europe/Istanbul").await.unwrap(),
            1,
            "onaylandı"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-051/099 (F-13): gece taramasi saat gelince bir kez, AD yapilandirilmissa.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn nightly_scans_open_once_per_day_after_the_hour_only_when_ad_is_configured() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let tz = "Europe/Istanbul";
        let open = |after: &'static str| open_nightly_scans(&pool, tz, after);

        assert_eq!(open("00:00").await.unwrap(), 0, "AD yapılandırılmamış");
        sqlx::query("UPDATE app_settings SET ad_host = 'dc1.example.org'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(open("24:00").await.unwrap(), 0, "saat gelmedi");
        assert_eq!(open("00:00").await.unwrap(), 2, "katalog + mutabakat");
        let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM read_jobs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(kinds, ["catalog_refresh", "reconcile"]);
        assert_eq!(
            open("00:00").await.unwrap(),
            0,
            "aynı gün ikinci kez açılmaz"
        );
        sqlx::query("UPDATE read_jobs SET status = 'succeeded', finished_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            open("00:00").await.unwrap(),
            0,
            "bittiyse de o gün yeniden açılmaz"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
