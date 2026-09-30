// --- START FEATURE: engine ---
// Bir isi calistirir: girdiyi yukle → olmasi gereken durumu hesapla (saf fonksiyon)
// → [hedefi oku → farki uygula]. Koseli parantezli adim AD connector'iyla gelir
// (3a: okuma yolu, tek yazma noktasi ve kuru calistirma); o gune kadar is,
// hesaplanan durumu sonuc olarak yazar ve hedefe dokunmaz.

use sqlx::PgPool;

use crate::desired_state::desired_state;
use crate::queue::ClaimedJob;

// ADR-052: hedefe ulasilamamasi isin degil hedefin arizasidir (deneme tuketmez,
// hedefin isleri bekletilir); nesne duzeyi hata deneme tuketir.
#[derive(Debug)]
pub enum JobError {
    /// Connector'lar uretir (3a okuma yolu ve yazma noktasi); su an yalnizca testler.
    #[allow(dead_code)]
    Unreachable(String),
    Failed(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Unreachable(reason) => write!(f, "hedefe ulaşılamıyor: {reason}"),
            JobError::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

pub async fn run_job(pool: &PgPool, job: &ClaimedJob, time_zone: &str) -> Result<String, JobError> {
    let input = crate::model::load(pool, job.identity_id, job.target_system_id, time_zone)
        .await
        .map_err(JobError::Failed)?;
    let desired = desired_state(
        &input.timeline,
        &input.model,
        input.link.as_ref(),
        &input.clock,
    );
    Ok(format!(
        "olması gereken durum hesaplandı, hedefe yazılmadı (connector yok): {desired:?}"
    ))
}
// --- END FEATURE: engine ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn run_job_computes_desired_state_for_seeded_identity() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let job = ClaimedJob {
            id: 0,
            identity_id: seed.identity,
            target_system_id: seed.ad,
            priority: 1,
            attempts: 0,
        };
        let result = run_job(&pool, &job, "Europe/Istanbul").await.unwrap();
        assert!(result.contains("state: Active"), "{result}");
        assert!(
            result.contains(&format!("Item({})", seed.sistem_uzmanlari_ou)),
            "{result}"
        );
        for item in [
            seed.gg_internet,
            seed.gg_bt_paylasim,
            seed.gg_ankara_yazici,
            seed.gg_sistem_uzmanlari,
            seed.gg_nobet,
        ] {
            assert!(
                result.contains(&format!("{item}")),
                "üyelik eksik: {item}: {result}"
            );
        }
        assert!(run_job(
            &pool,
            &ClaimedJob {
                identity_id: 999_999,
                ..job
            },
            "Europe/Istanbul"
        )
        .await
        .is_err());

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
