// --- START FEATURE: job-queue ---
// Kuyruk mekanigi (docs/02 kuyruk; ADR-016, 047, 052, 062): is 5 dk kirayla
// alinir (SKIP LOCKED yalnizca alma anindaki yarisi cozer, is boyunca
// transaction acik tutulmaz); kirasi dolan is deneme tuketmeden yeniden alinir;
// her hedef yazmasindan once NIYET satiri + kira uzatmasi tek transaction'da,
// kira baskasina gecmisse is birakilir. Basarisiz is artan aralikla yeniden
// denenir, deneme hakki bitince mudahaleye duser (acik sayilir, ADR-052).

use sqlx::PgPool;

/// Is kirasi (ADR-062 madde 1); okuma seridi de ayni sureyi kullanir.
pub const LEASE_MINUTES: i32 = 5;
const MAX_ATTEMPTS: i32 = 5;
const BACKOFF_BASE_SECONDS: i64 = 60;
const BACKOFF_MAX_EXPONENT: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub id: i64,
    pub identity_id: i64,
    pub target_system_id: i64,
    pub priority: i16,
    pub attempts: i32,
}

// Oncelik sirasi, ayni oncelikte ilk acilan. Alinabilir: kuyrukta ve zamani gelmis;
// kirasi dolmus (olen worker'in isi); mudahalede ve "tekrar dene" istenmis.
// Erisilemeyen hedeflerin isleri alinmaz (ADR-052 madde 3).
const CLAIM_SQL: &str = "WITH candidate AS ( \
    SELECT id FROM jobs \
    WHERE ((status = 'queued' AND next_attempt_at <= now()) \
       OR (status = 'running' AND locked_until < now()) \
       OR (status = 'needs_intervention' AND retry_requested)) \
      AND target_system_id <> ALL($3) \
    ORDER BY priority, created_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
    UPDATE jobs j SET status = 'running', locked_by = $1, \
        locked_until = now() + make_interval(mins => $2), retry_requested = FALSE \
    FROM candidate WHERE j.id = candidate.id \
    RETURNING j.id, j.identity_id, j.target_system_id, j.priority, j.attempts";

pub async fn claim(
    pool: &PgPool,
    worker_id: &str,
    unreachable_targets: &[i64],
) -> Result<Option<ClaimedJob>, sqlx::Error> {
    let row: Option<(i64, i64, i64, i16, i32)> = sqlx::query_as(CLAIM_SQL)
        .bind(worker_id)
        .bind(LEASE_MINUTES)
        .bind(unreachable_targets)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(
        |(id, identity_id, target_system_id, priority, attempts)| ClaimedJob {
            id,
            identity_id,
            target_system_id,
            priority,
            attempts,
        },
    ))
}

// Niyet/sonuc satirlarini tek yazma noktasi (3a ucuncu kutucuk) cagiracak; bu
// kutucukta yalnizca testler kullanir.
#[allow(dead_code)]
#[derive(Debug)]
pub enum IntentError {
    /// Kira baskasina gecmis (uyuyup uyanan eski worker): is birakilir, hedefe yazilmaz.
    LeaseLost,
    Db(sqlx::Error),
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentError::LeaseLost => write!(f, "iş kirası başka worker'a geçti"),
            IntentError::Db(e) => write!(f, "veritabanı hatası: {e}"),
        }
    }
}

#[allow(dead_code)]
pub struct Intent<'a> {
    pub event_type: &'a str,
    /// ADR-050 sayac sinifi: destructive | grant | first_password | attribute
    pub operation_class: &'a str,
    pub emergency: bool,
    /// JSON metni; worker'da serde yok, kucuk nesneler elle kurulur
    pub detail_json: &'a str,
}

// ADR-062 madde 2: once yaz, sonra uygula. Niyet satiri ve kira uzatmasi tek
// transaction'da; satir yazilamazsa ya da kira gitmisse hedefe dokunulmaz.
#[allow(dead_code)]
pub async fn record_intent(
    pool: &PgPool,
    job: &ClaimedJob,
    worker_id: &str,
    intent: &Intent<'_>,
) -> Result<i64, IntentError> {
    let mut tx = pool.begin().await.map_err(IntentError::Db)?;
    let intent_id: i64 = sqlx::query_scalar(
        "INSERT INTO audit_log \
         (event_type, identity_id, target_system_id, operation_class, emergency, detail) \
         VALUES ($1, $2, $3, $4, $5, $6::jsonb) RETURNING id",
    )
    .bind(intent.event_type)
    .bind(job.identity_id)
    .bind(job.target_system_id)
    .bind(intent.operation_class)
    .bind(intent.emergency)
    .bind(intent.detail_json)
    .fetch_one(&mut *tx)
    .await
    .map_err(IntentError::Db)?;
    let extended = sqlx::query(
        "UPDATE jobs SET locked_until = now() + make_interval(mins => $3) \
         WHERE id = $1 AND locked_by = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(LEASE_MINUTES)
    .execute(&mut *tx)
    .await
    .map_err(IntentError::Db)?;
    if extended.rows_affected() != 1 {
        return Err(IntentError::LeaseLost);
    }
    tx.commit().await.map_err(IntentError::Db)?;
    Ok(intent_id)
}

// Sonuc satiri niyetine baglanir (ADR-062); sonucu olmayan niyet "bilinmiyor"dur.
#[allow(dead_code)]
pub async fn record_outcome(
    pool: &PgPool,
    job: &ClaimedJob,
    intent_id: i64,
    succeeded: bool,
    event_type: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (event_type, identity_id, target_system_id, intent_id, outcome) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(event_type)
    .bind(job.identity_id)
    .bind(job.target_system_id)
    .bind(intent_id)
    .bind(if succeeded { "succeeded" } else { "failed" })
    .execute(pool)
    .await?;
    Ok(())
}

// Yalnizca kirayi hala tutan worker bitirebilir; baskasina gecmis is dokunulmaz.
pub async fn complete(
    pool: &PgPool,
    job: &ClaimedJob,
    worker_id: &str,
    result: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE jobs SET status = 'succeeded', result = $3, last_error = NULL, \
         finished_at = now(), locked_by = NULL, locked_until = NULL \
         WHERE id = $1 AND locked_by = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(crate::db::pg_text(result))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

// Nesne duzeyi hata deneme tuketir; artan aralikla yeniden denenir, hak bitince
// mudahale. Baglanti duzeyi hata icin ayri yol (3a ikinci kutucuk, ADR-052).
pub async fn fail(
    pool: &PgPool,
    job: &ClaimedJob,
    worker_id: &str,
    error: &str,
) -> Result<(), sqlx::Error> {
    let attempts = job.attempts + 1;
    let (status, delay) = if attempts >= MAX_ATTEMPTS {
        ("needs_intervention", 0)
    } else {
        ("queued", backoff_seconds(attempts))
    };
    sqlx::query(
        "UPDATE jobs SET status = $3, attempts = $4, last_error = $5, \
         next_attempt_at = now() + make_interval(secs => $6), locked_by = NULL, locked_until = NULL \
         WHERE id = $1 AND locked_by = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(status)
    .bind(attempts)
    .bind(crate::db::pg_text(error))
    .bind(delay as f64)
    .execute(pool)
    .await?;
    Ok(())
}

// Insan karari gereken is (ad cakismasi, kapsam disi hesap) beklemeden
// mudahaleye duser; deneme tuketmez, "tekrar dene" ile kuyruga doner (ADR-022, 052).
pub async fn intervene(
    pool: &PgPool,
    job: &ClaimedJob,
    worker_id: &str,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE jobs SET status = 'needs_intervention', last_error = $3, \
         locked_by = NULL, locked_until = NULL \
         WHERE id = $1 AND locked_by = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(crate::db::pg_text(reason))
    .execute(pool)
    .await?;
    Ok(())
}

// Isin kendi hatasi olmayan bekleme: deneme sayisi degismez, is verilen sure
// sonra yeniden alinabilir. Iki cagiran var — ADR-052 madde 3 baglanti duzeyi
// hata (TCP/TLS, bind, oturum, zaman asimi, 5xx; hedefin diger isleri de o sure
// alinmaz, main.rs) ve ADR-050 dolu fren sayaci (pencerenin acilisi beklenir).
pub async fn defer(
    pool: &PgPool,
    job: &ClaimedJob,
    worker_id: &str,
    reason: &str,
    retry_after_seconds: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE jobs SET status = 'queued', last_error = $3, \
         next_attempt_at = now() + make_interval(secs => $4), locked_by = NULL, locked_until = NULL \
         WHERE id = $1 AND locked_by = $2 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker_id)
    .bind(crate::db::pg_text(reason))
    .bind(retry_after_seconds as f64)
    .execute(pool)
    .await?;
    Ok(())
}

// 1, 2, 4, 8, 16 dk; ustel ama sinirli.
fn backoff_seconds(attempts: i32) -> i64 {
    let exponent = (attempts.max(1) - 1).min(BACKOFF_MAX_EXPONENT as i32) as u32;
    BACKOFF_BASE_SECONDS << exponent
}
// --- END FEATURE: job-queue ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_seconds(1), 60);
        assert_eq!(backoff_seconds(2), 120);
        assert_eq!(backoff_seconds(4), 480);
        assert_eq!(backoff_seconds(9), 960, "16 dk tavan");
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn claims_by_priority_reclaims_expired_lease_and_backs_off() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let bulk = test_support::enqueue(&pool, seed.identity, seed.ad, 2).await;
        let emergency = test_support::enqueue(&pool, seed.other_identity, seed.ad, 0).await;

        assert!(
            claim(&pool, "w1", &[seed.ad]).await.unwrap().is_none(),
            "erişilemeyen hedefin işi alınmaz"
        );
        let first = claim(&pool, "w1", &[]).await.unwrap().expect("iş alınmalı");
        assert_eq!(first.id, emergency, "acil ayrılış önce");

        // baglanti hatasi: deneme sayisi degismez, kisa sure sonra yeniden alinir
        defer(&pool, &first, "w1", "LDAP erişilemiyor", 0)
            .await
            .unwrap();
        let again = claim(&pool, "w1", &[])
            .await
            .unwrap()
            .expect("ertelenen iş yeniden alınmalı");
        assert_eq!(
            (again.id, again.attempts),
            (first.id, 0),
            "bağlantı hatası deneme tüketmez"
        );
        let first = again;

        let second = claim(&pool, "w1", &[]).await.unwrap().expect("ikinci iş");
        assert_eq!(second.id, bulk);
        assert!(
            claim(&pool, "w1", &[]).await.unwrap().is_none(),
            "kuyruk boş"
        );

        // olen worker: kira doldu, deneme sayisi degismeden yeniden alinir (ADR-062)
        sqlx::query("UPDATE jobs SET locked_until = now() - interval '1 second' WHERE id = $1")
            .bind(first.id)
            .execute(&pool)
            .await
            .unwrap();
        let reclaimed = claim(&pool, "w2", &[])
            .await
            .unwrap()
            .expect("kirası dolan iş yeniden alınmalı");
        assert_eq!((reclaimed.id, reclaimed.attempts), (first.id, 0));

        // niyet: kira w2'de; w1 artik uzatamaz ve hedefe yazmamali
        let intent = Intent {
            event_type: "ad.account.disable",
            operation_class: "destructive",
            emergency: true,
            detail_json: "{}",
        };
        assert!(matches!(
            record_intent(&pool, &first, "w1", &intent).await,
            Err(IntentError::LeaseLost)
        ));
        let intent_id = record_intent(&pool, &reclaimed, "w2", &intent)
            .await
            .expect("w2 niyet yazabilmeli");
        record_outcome(&pool, &reclaimed, intent_id, true, "ad.account.disable")
            .await
            .unwrap();
        assert!(
            !complete(&pool, &first, "w1", "x").await.unwrap(),
            "w1 bitiremez"
        );
        assert!(complete(&pool, &reclaimed, "w2", "tamam").await.unwrap());

        // basarisizlik: geri cekilme, sonra mudahale
        fail(&pool, &second, "w1", "nesne hatası").await.unwrap();
        assert!(
            claim(&pool, "w1", &[]).await.unwrap().is_none(),
            "geri çekilme süresi dolmadan alınmaz"
        );
        sqlx::query("UPDATE jobs SET next_attempt_at = now(), attempts = $2 WHERE id = $1")
            .bind(second.id)
            .bind(MAX_ATTEMPTS - 1)
            .execute(&pool)
            .await
            .unwrap();
        let last_try = claim(&pool, "w1", &[])
            .await
            .unwrap()
            .expect("süresi gelen iş alınır");
        fail(&pool, &last_try, "w1", "yine hata").await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
            .bind(second.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "needs_intervention");
        assert!(
            claim(&pool, "w1", &[]).await.unwrap().is_none(),
            "müdahaledeki iş kendiliğinden alınmaz"
        );
        sqlx::query("UPDATE jobs SET retry_requested = TRUE WHERE id = $1")
            .bind(second.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            claim(&pool, "w1", &[]).await.unwrap().is_some(),
            "tekrar dene ile alınır"
        );

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
