// --- START FEATURE: read-lane ---
// Okuma seridi (ADR-051): mutabakat, katalog yenileme ve toplu yonetime almanin
// fark hesabi YAZMA SERIDINDEN AYRI bir gorevde calisir. Bu isler hedefe hicbir
// sey yazmaz ve fren sayaclarina (ADR-050) dokunmaz; veritabanina yalnizca
// katalog, rapor ve is kaydi yazarlar. Yarisin kaynagi yazan islerdir, bu yuzden
// okuma seridi tek siraya ve sayac kilidine ihtiyac duymaz.
//
// Ayni anda en fazla bir okuma isi calisir: gece mutabakati surerken acil ayrilis
// yazma seridinde ilerler (N-02), uzun taramanin arkasinda beklemez.

use sqlx::PgPool;

pub const CATALOG_REFRESH: &str = "catalog_refresh";
pub const RECONCILE: &str = "reconcile";
/// Toplu yonetime almanin fark hesabi (ADR-051 ucuncu tur, ADR-094 sonuclari)
pub const MANAGE_DIFF: &str = "manage_diff";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadJob {
    pub id: i64,
    pub kind: String,
    pub target_system_id: i64,
}

// Ayni anda tek is: SKIP LOCKED yalnizca alma anindaki yarisi cozer, serit zaten
// tek gorevdir. En eski bekleyen is alinir.
const CLAIM_SQL: &str = "WITH candidate AS ( \
    SELECT id FROM read_jobs WHERE status = 'queued' \
    ORDER BY created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED) \
    UPDATE read_jobs j SET status = 'running', started_at = now() \
    FROM candidate WHERE j.id = candidate.id \
    RETURNING j.id, j.kind, j.target_system_id";

pub async fn claim(pool: &PgPool) -> Result<Option<ReadJob>, sqlx::Error> {
    let row: Option<(i64, String, i64)> = sqlx::query_as(CLAIM_SQL).fetch_optional(pool).await?;
    Ok(row.map(|(id, kind, target_system_id)| ReadJob {
        id,
        kind,
        target_system_id,
    }))
}

pub async fn finish(
    pool: &PgPool,
    id: i64,
    outcome: Result<String, String>,
) -> Result<(), sqlx::Error> {
    let (status, result) = match &outcome {
        Ok(text) => ("succeeded", text),
        Err(text) => ("failed", text),
    };
    sqlx::query("UPDATE read_jobs SET status = $2, result = $3, finished_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .bind(result)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Acik is varsa yenisi acilmaz (kismi tekil indeks); cagiran bunu hata saymaz.
pub async fn request(pool: &PgPool, kind: &str, target: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO read_jobs (kind, target_system_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(kind)
    .bind(target)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn ad_target(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
        .fetch_optional(pool)
        .await
}
// --- END FEATURE: read-lane ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    // ADR-051: tek serit, en eski is once; acik is varken ayni tur ve hedef icin
    // yenisi acilmaz; sonuc is kaydina yazilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn claims_one_at_a_time_and_deduplicates_open_requests() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let ad = ad_target(&pool).await.unwrap().expect("AD hedefi seed");

        assert!(claim(&pool).await.unwrap().is_none(), "kuyruk boş");
        assert!(request(&pool, CATALOG_REFRESH, ad).await.unwrap());
        assert!(
            !request(&pool, CATALOG_REFRESH, ad).await.unwrap(),
            "açık iş varken ikincisi açılmaz"
        );

        let job = claim(&pool).await.unwrap().expect("iş alınmalı");
        assert_eq!(
            (job.kind.as_str(), job.target_system_id),
            (CATALOG_REFRESH, ad)
        );
        assert!(
            claim(&pool).await.unwrap().is_none(),
            "çalışan iş ikinci kez alınmaz"
        );
        assert!(
            !request(&pool, CATALOG_REFRESH, ad).await.unwrap(),
            "çalışan iş de açık sayılır"
        );

        finish(&pool, job.id, Ok("3 OU, 5 grup".to_string()))
            .await
            .unwrap();
        let (status, result): (String, Option<String>) =
            sqlx::query_as("SELECT status, result FROM read_jobs WHERE id = $1")
                .bind(job.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (status.as_str(), result.as_deref()),
            ("succeeded", Some("3 OU, 5 grup"))
        );
        assert!(
            request(&pool, CATALOG_REFRESH, ad).await.unwrap(),
            "bitince yeni istek açılır"
        );

        let second = claim(&pool).await.unwrap().expect("ikinci iş");
        finish(&pool, second.id, Err("DC'ye ulaşılamadı".to_string()))
            .await
            .unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM read_jobs WHERE id = $1")
            .bind(second.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
