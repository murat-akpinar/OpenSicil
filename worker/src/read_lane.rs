// --- START FEATURE: read-lane ---
// Okuma seridi (ADR-051): mutabakat, katalog yenileme ve toplu yonetime almanin
// fark hesabi YAZMA SERIDINDEN AYRI bir gorevde calisir. Bu isler hedefe hicbir
// sey yazmaz ve fren sayaclarina (ADR-050) dokunmaz; veritabanina yalnizca
// katalog, rapor ve is kaydi yazarlar. Yarisin kaynagi yazan islerdir, bu yuzden
// okuma seridi tek siraya ve sayac kilidine ihtiyac duymaz.
//
// Ayni anda en fazla bir okuma isi calisir: gece mutabakati surerken acil ayrilis
// yazma seridinde ilerler (N-02), uzun taramanin arkasinda beklemez.

use std::time::Duration;

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
// tek gorevdir. En eski bekleyen is alinir ve satira kira yazilir.
//
// `expired`: kirasi dolmus `running` is geri alinir (ADR-062 madde 1'in okuma
// seridi karsiligi). Worker is ortasinda olurse satir sonsuza dek `running`
// kalir, `read_jobs_open_idx` o tur + hedef icin yeni is actirmaz ve gece
// mutabakati sessizce duser. Kirasi dolmayana dokunulmaz: baska bir worker
// kopyasinin suren taramasi calinmaz (ADR-061). Kira kolonu yokken alinmis
// satirin (yukseltme ani) suresi `started_at`ten sayilir.
/// Geri alinan isin sonuc metni; gece taramasi bu satiri "dilim tuketti" saymaz.
pub const RECLAIMED: &str = "yarıda kaldı: iş kirası doldu, geri alındı";

const CLAIM_SQL: &str = "WITH expired AS ( \
    UPDATE read_jobs SET status = 'failed', finished_at = now(), result = $2 \
    WHERE status = 'running' \
      AND COALESCE(locked_until, started_at + make_interval(mins => $1)) < now() \
    RETURNING id), \
    candidate AS ( \
    SELECT id FROM read_jobs WHERE status = 'queued' \
    ORDER BY created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED) \
    UPDATE read_jobs j SET status = 'running', started_at = now(), \
        locked_until = now() + make_interval(mins => $1) \
    FROM candidate WHERE j.id = candidate.id \
    RETURNING j.id, j.kind, j.target_system_id";

pub async fn claim(pool: &PgPool) -> Result<Option<ReadJob>, sqlx::Error> {
    let row: Option<(i64, String, i64)> = sqlx::query_as(CLAIM_SQL)
        .bind(crate::queue::LEASE_MINUTES)
        .bind(RECLAIMED)
        .fetch_optional(pool)
        .await?;
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
    // Kirasi dolup geri alinmis is sonucu yazamaz: "yarıda kaldı" kaydinin
    // uzerine binmez, yerine gececek yeni is zaten acilabilir durumdadir.
    sqlx::query(
        "UPDATE read_jobs SET status = $2, result = $3, finished_at = now() \
         WHERE id = $1 AND status = 'running'",
    )
    .bind(id)
    .bind(status)
    .bind(crate::db::pg_text(result))
    .execute(pool)
    .await
    .map(|_| ())
}

/// Kira uzatmasi (ADR-062 madde 2'nin okuma seridi karsiligi): uzun tarama
/// kendi kirasina dusup geri alinmasin. Sifir satir = is elden gitti.
pub async fn renew(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE read_jobs SET locked_until = now() + make_interval(mins => $2) \
         WHERE id = $1 AND status = 'running'",
    )
    .bind(id)
    .bind(crate::queue::LEASE_MINUTES)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Isi kirayi yarilayan araliklarla uzatarak yurutur. Kira elden gittiyse is
/// birakilir (ADR-062 madde 2): geri alinmis satirin uzerine yazan bir hayalet
/// tarama katalogu ve bulgulari tazelemeye devam etmez.
pub async fn with_lease<F>(pool: &PgPool, id: i64, work: F) -> Result<String, String>
where
    F: std::future::Future<Output = Result<String, String>>,
{
    let every = Duration::from_secs(crate::queue::LEASE_MINUTES as u64 * 30);
    tokio::pin!(work);
    loop {
        tokio::select! {
            outcome = &mut work => return outcome,
            _ = tokio::time::sleep(every) => match renew(pool, id).await {
                Ok(true) => {}
                Ok(false) => return Err("iş kirası elden gitti, okuma bırakıldı".to_string()),
                Err(e) => eprintln!("worker: okuma işi {id} kirası uzatılamadı: {e}"),
            },
        }
    }
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
        // Hata metni hedeften gelir ve NUL tasiyabilir (Windows AD'nin LDAP
        // tani metni); satir yine yazilmali, yoksa is sonsuza dek `running`
        // kalir ve hata ekranda hic gorunmez.
        finish(
            &pool,
            second.id,
            Err("DC'ye ulaşılamadı: best match of:\u{0}\n".to_string()),
        )
        .await
        .unwrap();
        let (status, result): (String, Option<String>) =
            sqlx::query_as("SELECT status, result FROM read_jobs WHERE id = $1")
                .bind(second.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(
            result.as_deref(),
            Some("DC'ye ulaşılamadı: best match of:\n"),
            "hata metni iş kaydına yazılmadı"
        );

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-062'nin okuma seridi karsiligi: worker is ortasinda olurse kirasi
    // dolan satir geri alinir ("yarıda kaldı") ve ayni tur + hedef icin yeni is
    // acilabilir; kirasi dolmayan satira dokunulmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn expired_lease_is_reclaimed_and_a_live_one_is_left_alone() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let ad = ad_target(&pool).await.unwrap().expect("AD hedefi seed");
        let row = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (String, Option<String>)>(
                    "SELECT status, result FROM read_jobs WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };

        assert!(request(&pool, RECONCILE, ad).await.unwrap());
        let job = claim(&pool).await.unwrap().expect("iş alınmalı");

        // Kira sürüyor: başka bir worker kopyası taramayı çalmaz (ADR-061).
        assert!(claim(&pool).await.unwrap().is_none(), "süren iş alınmaz");
        assert_eq!(row(job.id).await.0, "running", "süren iş geri alınmaz");
        assert!(
            renew(&pool, job.id).await.unwrap(),
            "süren işin kirası uzar"
        );

        // Worker iş ortasında ölür: satır `running` kalır, kira dolar.
        sqlx::query(
            "UPDATE read_jobs SET locked_until = now() - make_interval(mins => 1) WHERE id = $1",
        )
        .bind(job.id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(claim(&pool).await.unwrap().is_none(), "kuyrukta iş yok");
        let (status, result) = row(job.id).await;
        assert_eq!(status, "failed");
        assert!(
            result
                .as_deref()
                .unwrap_or_default()
                .contains("yarıda kaldı"),
            "{result:?}"
        );
        assert!(
            !renew(&pool, job.id).await.unwrap(),
            "geri alınan işin kirası uzamaz"
        );
        // Hayalet tarama "yarıda kaldı" kaydının üzerine binmez.
        finish(&pool, job.id, Ok("3 OU, 5 grup".to_string()))
            .await
            .unwrap();
        assert!(row(job.id).await.1.unwrap().contains("yarıda kaldı"));

        // Tikanma kalkar: ayni tur + hedef icin yeni is acilir ve alinir.
        assert!(
            request(&pool, RECONCILE, ad).await.unwrap(),
            "geri alındıktan sonra yeni iş açılır"
        );
        let next = claim(&pool).await.unwrap().expect("yeni iş alınmalı");
        assert_ne!(next.id, job.id);

        // Kira kolonu yokken alinmis satir (yukseltme ani): suresi `started_at`ten sayilir.
        sqlx::query(
            "UPDATE read_jobs SET locked_until = NULL, \
             started_at = now() - make_interval(mins => 10) WHERE id = $1",
        )
        .bind(next.id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(claim(&pool).await.unwrap().is_none());
        assert_eq!(row(next.id).await.0, "failed", "kirasız satır geri alınır");

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
