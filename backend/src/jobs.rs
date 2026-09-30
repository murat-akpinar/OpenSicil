// --- START FEATURE: job-queue ---
// Backend is olusturur ve "tekrar dene" ister; alma, uygulama ve sonuc worker'da
// (ADR-015). Tekillestirme kismi tekil indeksle DB'de (ADR-016); acik is varken
// yenisi acilmaz, yalnizca onceligi yukseltilir (acil ayrilis toplu setin
// arkasinda beklemez).

use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i16)]
pub enum Priority {
    Emergency = 0,
    Single = 1,
    Bulk = 2,
    Reconciliation = 3,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enqueue {
    Created(i64),
    AlreadyOpen(i64),
}

pub async fn enqueue(
    pool: &PgPool,
    identity_id: i64,
    target_system_id: i64,
    priority: Priority,
) -> Result<Enqueue, sqlx::Error> {
    // xmax = 0: satir bu ifadede eklendi; degilse acik is vardi, onceligi guncellendi
    let (id, inserted): (i64, bool) = sqlx::query_as(
        "INSERT INTO jobs (identity_id, target_system_id, priority) VALUES ($1, $2, $3) \
         ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' \
         DO UPDATE SET priority = LEAST(jobs.priority, EXCLUDED.priority) \
         RETURNING id, (xmax = 0)",
    )
    .bind(identity_id)
    .bind(target_system_id)
    .bind(priority as i16)
    .fetch_one(pool)
    .await?;
    Ok(if inserted {
        Enqueue::Created(id)
    } else {
        Enqueue::AlreadyOpen(id)
    })
}

// "Tekrar dene": yalnizca mudahaledeki is; worker bayragi gorunce kuyruga alir (ADR-052).
pub async fn request_retry(pool: &PgPool, job_id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE jobs SET retry_requested = TRUE WHERE id = $1 AND status = 'needs_intervention'",
    )
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}
// --- END FEATURE: job-queue ---

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn enqueue_dedups_open_jobs_and_raises_priority() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;

        let first = enqueue(&pool, ids[0], catalog.ad, Priority::Bulk)
            .await
            .unwrap();
        let Enqueue::Created(job_id) = first else {
            panic!("ilk iş oluşmalı: {first:?}");
        };
        assert_eq!(
            enqueue(&pool, ids[0], catalog.ad, Priority::Emergency)
                .await
                .unwrap(),
            Enqueue::AlreadyOpen(job_id),
            "açık iş varken yenisi açılmaz"
        );
        let priority: i16 = sqlx::query_scalar("SELECT priority FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(priority, Priority::Emergency as i16, "öncelik yükseltilir");
        assert!(
            matches!(
                enqueue(&pool, ids[0], catalog.zimbra, Priority::Single)
                    .await
                    .unwrap(),
                Enqueue::Created(_)
            ),
            "başka hedef ayrı iş"
        );

        assert!(
            !request_retry(&pool, job_id).await.unwrap(),
            "kuyruktaki işe tekrar dene yok"
        );
        sqlx::query("UPDATE jobs SET status = 'needs_intervention' WHERE id = $1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(request_retry(&pool, job_id).await.unwrap());

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
