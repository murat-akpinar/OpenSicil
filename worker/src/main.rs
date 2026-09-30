mod common_settings;
mod db;
// effective_manager'i AD eslemesi (3a connector) cagirir; backend kopyasiyla birebir ayni.
#[allow(dead_code)]
mod desired_state;
mod engine;
mod heartbeat;
mod model;
mod queue;
#[cfg(test)]
mod test_support;

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Notify;

// ADR-028: kuyruk 5 sn'de bir yoklanir; ayri bildirim mekanizmasi yok.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("worker-health") {
        return heartbeat::check();
    }
    run().await
}

// Baglanti ve sema kontrolunu tek yerde toplar (run()'u ≤50 satir tutar, security.md).
async fn prepare_pool(database_url: &str) -> Result<PgPool, String> {
    let pool = db::connect_pool(database_url)
        .await
        .map_err(|e| format!("worker: veritabanına bağlanılamadı: {e}"))?;
    db::check_schema_ready(&pool)
        .await
        .map_err(|e| format!("worker: şema hazır değil: {e}"))?;
    Ok(pool)
}

struct Env {
    database_url: String,
    time_zone: String,
}

// Ortak ayarlar acilista dogrulanir ve loglanir; backend'in satiriyla yan
// yana konunca iki servisin sapmasi gorulur (ADR-039).
fn load_env() -> Result<Env, String> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "worker: ortam değişkeni eksik: DATABASE_URL".to_string())?;
    let common = common_settings::CommonSettings::from_env().map_err(|e| format!("worker: {e}"))?;
    println!("worker: ortak ayarlar: {common}");
    Ok(Env {
        database_url,
        time_zone: common.time_zone,
    })
}

// SIGTERM'de eldeki is bitirilir, yeni is alinmaz (ADR-061 madde 1): bayrak +
// uyandirma; dongu her isten sonra ve beklerken bakar.
fn spawn_sigterm_watcher() -> Result<(Arc<AtomicBool>, Arc<Notify>), String> {
    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| format!("worker: SIGTERM işleyicisi kurulamadı: {e}"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(Notify::new());
    let (stop_flag, wake_signal) = (Arc::clone(&stop), Arc::clone(&wake));
    tokio::spawn(async move {
        sigterm.recv().await;
        println!("worker: SIGTERM alındı, eldeki iş bitirilip çıkılıyor");
        stop_flag.store(true, Ordering::SeqCst);
        wake_signal.notify_one();
    });
    Ok((stop, wake))
}

// Kira sahibinin adi (ADR-062 locked_by); ayni host'ta iki kopya pid ile ayrilir.
fn worker_id() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "worker".to_string());
    format!("{host}-{}", std::process::id())
}

async fn run() -> ExitCode {
    let env = match load_env() {
        Ok(env) => env,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let pool = match prepare_pool(&env.database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let (stop, wake) = match spawn_sigterm_watcher() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let worker_id = worker_id();
    println!("worker: {worker_id} başladı, {POLL_INTERVAL:?} aralıkla yoklanıyor");

    // Yazma seridi tek sirada (ADR-047): bir is bitmeden digeri alinmaz;
    // kuyruk bosalinca 5 sn beklenir.
    while !stop.load(Ordering::SeqCst) {
        if let Err(e) = heartbeat::touch() {
            eprintln!("worker: nabız dosyasına yazılamadı: {e}");
        }
        match queue::claim(&pool, &worker_id).await {
            Ok(Some(job)) => {
                process_job(&pool, &job, &worker_id, &env.time_zone).await;
                continue;
            }
            Ok(None) => {}
            Err(e) => eprintln!("worker: kuyruk okunamadı: {e}"),
        }
        let _ = tokio::time::timeout(POLL_INTERVAL, wake.notified()).await;
    }
    ExitCode::SUCCESS
}

async fn process_job(pool: &PgPool, job: &queue::ClaimedJob, worker_id: &str, time_zone: &str) {
    let outcome = match engine::run_job(pool, job, time_zone).await {
        Ok(result) => queue::complete(pool, job, worker_id, &result)
            .await
            .map(|_| ()),
        Err(error) => {
            eprintln!("worker: iş {} başarısız: {error}", job.id);
            queue::fail(pool, job, worker_id, &error).await
        }
    };
    if let Err(e) = outcome {
        eprintln!("worker: iş {} sonucu yazılamadı: {e}", job.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Gercek Postgres gerektirir (ADR-070): DATABASE_URL, testler icin
    // ayrilmis bir veritabanina isaret etmeli (proje .env'i degil). POLL_INTERVAL
    // 5 sn oldugundan dongu turunu SIGTERM ile erken kesip beklemeyi kisaltir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn run_processes_queue_then_exits_on_sigterm() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let job_id = test_support::enqueue(&pool, seed.identity, seed.ad, 1).await;
        let test_url = format!(
            "{}/{db_name}",
            std::env::var("DATABASE_URL")
                .unwrap()
                .rsplit_once('/')
                .unwrap()
                .0
        );
        // SAFETY: tek is parcacikli, ayni degiskenleri eszamanli degistiren
        // baska test yok (backend migrate testindeki desenle ayni).
        unsafe {
            std::env::set_var("DATABASE_URL", &test_url);
            for (name, value) in common_settings::tests::ENV_EXAMPLE_DEFAULTS {
                std::env::set_var(name, value);
            }
        }

        let handle = tokio::spawn(run());
        tokio::time::sleep(Duration::from_millis(800)).await;
        let pid = std::process::id();
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .expect("kill çağrılamadı");

        let exit_code = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("run() zaman aşımına uğradı")
            .expect("görev panikledi");
        assert_eq!(format!("{exit_code:?}"), format!("{:?}", ExitCode::SUCCESS));

        let (status, result): (String, Option<String>) =
            sqlx::query_as("SELECT status, result FROM jobs WHERE id = $1")
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "succeeded", "kuyruktaki iş tek sırada işlenmeli");
        assert!(result.unwrap_or_default().contains("state: Active"));

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
