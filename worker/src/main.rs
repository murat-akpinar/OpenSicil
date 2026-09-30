mod ad;
mod catalog;
mod common_settings;
// backend kopyasiyla birebir ayni; worker su an yalnizca cozer, ilk parola (3d) sifreler.
#[allow(dead_code)]
mod crypto;
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
// Motor farki islemlere cevirince (3a) her hedef yazmasi buradan gecer; su an testler.
#[allow(dead_code)]
mod writes;

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    write_mode: writes::Mode,
    aead_key: [u8; crypto::KEY_LEN],
    ad_ca_file: Option<String>,
}

// Ortak ayarlar acilista dogrulanir ve loglanir; backend'in satiriyla yan
// yana konunca iki servisin sapmasi gorulur (ADR-039). DRY_RUN (ADR-054):
// acikken hedefe hicbir sey yazilmaz; unutulmasin diye her acilista loglanir.
fn load_env() -> Result<Env, String> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "worker: ortam değişkeni eksik: DATABASE_URL".to_string())?;
    let common = common_settings::CommonSettings::from_env().map_err(|e| format!("worker: {e}"))?;
    let dry_run = match std::env::var("DRY_RUN").as_deref().map(str::trim) {
        Ok("true") | Ok("1") => true,
        Ok("false") | Ok("0") => false,
        Ok(other) => {
            return Err(format!(
                "worker: DRY_RUN true ya da false olmalı, '{other}' geldi"
            ))
        }
        Err(_) => return Err("worker: ortam değişkeni eksik: DRY_RUN".to_string()),
    };
    let aead_key = std::env::var("AEAD_MASTER_KEY")
        .map_err(|_| "worker: ortam değişkeni eksik: AEAD_MASTER_KEY".to_string())
        .and_then(|v| {
            crypto::parse_key("AEAD_MASTER_KEY", &v).map_err(|e| format!("worker: {e}"))
        })?;
    println!("worker: ortak ayarlar: {common}");
    if dry_run {
        println!("worker: KURU ÇALIŞTIRMA açık — hedefe hiçbir şey yazılmaz (ADR-054)");
    }
    Ok(Env {
        database_url,
        time_zone: common.time_zone,
        write_mode: writes::Mode { dry_run },
        aead_key,
        ad_ca_file: std::env::var("AD_CA_FILE").ok(),
    })
}

// Acilista katalog yenileme (docs/03 katalog; ADR-051: okuma seridi Faz 5'e kadar
// burada). AD yapilandirilmamissa atlanir; DC'ye ulasilamiyorsa surec cikmaz,
// loglar ve devam eder (ADR-061); kapsam hatasi da AD connector'ini baslatmaz.
async fn refresh_catalog_at_startup(pool: &PgPool, env: &Env) {
    let cfg = match ad::load_config(pool, &env.aead_key, env.ad_ca_file.as_deref()).await {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            println!("worker: AD yapılandırılmamış, katalog yenileme atlandı");
            return;
        }
        Err(e) => {
            eprintln!("worker: AD ayarları okunamadı, katalog yenileme atlandı: {e}");
            return;
        }
    };
    let scope = match ad::parse_scope(|name| std::env::var(name).ok()) {
        Ok(scope) => scope,
        Err(e) => {
            eprintln!("worker: yönetilen kapsam geçersiz, AD connector'ı başlamadı: {e}");
            return;
        }
    };
    let target: Result<i64, sqlx::Error> =
        sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(pool)
            .await;
    let outcome = async {
        let target = target.map_err(|e| format!("hedef sistem okunamadı: {e}"))?;
        let mut ldap = ad::connect(&cfg).await.map_err(|e| e.to_string())?;
        let snapshot = ad::read_catalog(&mut ldap, &scope)
            .await
            .map_err(|e| e.to_string())?;
        ldap.unbind().await.ok();
        let counts = catalog::sync_snapshot(pool, target, &snapshot)
            .await
            .map_err(|e| format!("katalog yazılamadı: {e}"))?;
        Ok::<_, String>((counts, snapshot.forbidden.len()))
    }
    .await;
    match outcome {
        Ok((counts, forbidden)) => println!(
            "worker: AD kataloğu yenilendi: {} OU, {} grup, {} kayıp; {forbidden} yasaklı grup kataloğa alınmadı",
            counts.ous, counts.groups, counts.marked_missing
        ),
        Err(e) => eprintln!("worker: AD kataloğu yenilenemedi: {e}"),
    }
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
    refresh_catalog_at_startup(&pool, &env).await;

    // Yazma seridi tek sirada (ADR-047): bir is bitmeden digeri alinmaz;
    // kuyruk bosalinca 5 sn beklenir. Erisilemeyen hedefin isleri bir yoklama
    // suresi boyunca alinmaz, sonra tek isle yeniden yoklanir (ADR-052 madde 3).
    let mut unreachable: HashMap<i64, Instant> = HashMap::new();
    while !stop.load(Ordering::SeqCst) {
        if let Err(e) = heartbeat::touch() {
            eprintln!("worker: nabız dosyasına yazılamadı: {e}");
        }
        let skip = unreachable_targets(&unreachable, Instant::now());
        match queue::claim(&pool, &worker_id, &skip).await {
            Ok(Some(job)) => {
                if process_job(&pool, &job, &worker_id, &env).await {
                    unreachable.insert(job.target_system_id, Instant::now() + POLL_INTERVAL);
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => eprintln!("worker: kuyruk okunamadı: {e}"),
        }
        let _ = tokio::time::timeout(POLL_INTERVAL, wake.notified()).await;
    }
    ExitCode::SUCCESS
}

fn unreachable_targets(marks: &HashMap<i64, Instant>, now: Instant) -> Vec<i64> {
    marks
        .iter()
        .filter(|(_, until)| **until > now)
        .map(|(target, _)| *target)
        .collect()
}

// Doner: hedef erisilemez isaretlenmeli mi.
async fn process_job(pool: &PgPool, job: &queue::ClaimedJob, worker_id: &str, env: &Env) -> bool {
    let run = engine::run_job(pool, job, &env.time_zone, env.write_mode).await;
    let (outcome, unreachable) = match run {
        Ok(result) => (
            queue::complete(pool, job, worker_id, &result)
                .await
                .map(|_| ()),
            false,
        ),
        Err(engine::JobError::Unreachable(reason)) => {
            eprintln!(
                "worker: iş {} ertelendi, hedefe ulaşılamıyor: {reason}",
                job.id
            );
            let retry = POLL_INTERVAL.as_secs() as i64;
            (
                queue::defer_unreachable(pool, job, worker_id, &reason, retry).await,
                true,
            )
        }
        Err(error) => {
            eprintln!("worker: iş {} başarısız: {error}", job.id);
            (
                queue::fail(pool, job, worker_id, &error.to_string()).await,
                false,
            )
        }
    };
    if let Err(e) = outcome {
        eprintln!("worker: iş {} sonucu yazılamadı: {e}", job.id);
    }
    unreachable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_marks_expire() {
        let now = Instant::now();
        let marks = HashMap::from([
            (1_i64, now + POLL_INTERVAL),
            (2_i64, now - Duration::from_secs(1)),
        ]);
        assert_eq!(unreachable_targets(&marks, now), vec![1]);
    }

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
            std::env::set_var("DRY_RUN", "true");
            std::env::set_var(
                "AEAD_MASTER_KEY",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            );
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
        let result = result.unwrap_or_default();
        assert!(result.contains("state: Active"), "{result}");
        assert!(
            result.starts_with("kuru çalıştırma"),
            "DRY_RUN=true: {result}"
        );

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
