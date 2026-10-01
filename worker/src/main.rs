mod ad;
mod ad_account;
mod adoption;
mod catalog;
mod common_settings;
mod counters;
// backend kopyasiyla birebir ayni; worker su an yalnizca cozer, ilk parola (3d) sifreler.
#[allow(dead_code)]
mod crypto;
mod db;
// effective_manager'i AD eslemesi (3a connector) cagirir; backend kopyasiyla birebir ayni.
#[allow(dead_code)]
mod desired_state;
mod engine;
mod first_password;
mod heartbeat;
mod mapping;
// Ikiz dosya (backend ile birebir ayni); ekran etiketleri yalnizca backend'de kullanilir.
#[allow(dead_code)]
mod mapping_rules;
mod model;
mod queue;
mod scheduler;
#[cfg(test)]
mod test_support;
mod username;
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
// ADR-028: zamanlayici cozunurlugu 1 dakika; ilk tik acilista (kacirilan gecisler).
const TICK_INTERVAL: Duration = Duration::from_secs(60);

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
    // ADR-029: hassas kaynak (kimlik no, cep) eslemesi; kapaliyken satir reddedilir
    sensitive_mapping_enabled: bool,
    // ADR-019: ilk paroladan sonra pwdLastSet 0; varsayilan acik
    first_login_change_required: bool,
    // ADR-018: sahiplenme (ortak ayar, varsayilan kapali)
    ownership_mode_enabled: bool,
    // ADR-016/050: saatlik fren sayaclari ve acil kota
    limits: counters::Limits,
}

fn parse_bool_env(name: &str) -> Result<Option<bool>, String> {
    match std::env::var(name).as_deref().map(str::trim) {
        Ok("true") | Ok("1") => Ok(Some(true)),
        Ok("false") | Ok("0") => Ok(Some(false)),
        Ok(other) => Err(format!(
            "worker: {name} true ya da false olmalı, '{other}' geldi"
        )),
        Err(_) => Ok(None),
    }
}

// Ortak ayarlar acilista dogrulanir ve loglanir; backend'in satiriyla yan
// yana konunca iki servisin sapmasi gorulur (ADR-039). DRY_RUN (ADR-054):
// acikken hedefe hicbir sey yazilmaz; unutulmasin diye her acilista loglanir.
fn load_env() -> Result<Env, String> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "worker: ortam değişkeni eksik: DATABASE_URL".to_string())?;
    let common = common_settings::CommonSettings::from_env().map_err(|e| format!("worker: {e}"))?;
    let dry_run = parse_bool_env("DRY_RUN")?
        .ok_or_else(|| "worker: ortam değişkeni eksik: DRY_RUN".to_string())?;
    let first_login_change_required =
        parse_bool_env("FIRST_LOGIN_CHANGE_REQUIRED")?.unwrap_or(true);
    let aead_key = std::env::var("AEAD_MASTER_KEY")
        .map_err(|_| "worker: ortam değişkeni eksik: AEAD_MASTER_KEY".to_string())
        .and_then(|v| {
            crypto::parse_key("AEAD_MASTER_KEY", &v).map_err(|e| format!("worker: {e}"))
        })?;
    println!(
        "worker: ortak ayarlar: {common}; FIRST_LOGIN_CHANGE_REQUIRED={first_login_change_required}"
    );
    if dry_run {
        println!("worker: KURU ÇALIŞTIRMA açık — hedefe hiçbir şey yazılmaz (ADR-054)");
    }
    Ok(Env {
        database_url,
        write_mode: writes::Mode { dry_run },
        aead_key,
        ad_ca_file: std::env::var("AD_CA_FILE").ok(),
        sensitive_mapping_enabled: common.sensitive_mapping_enabled,
        first_login_change_required,
        ownership_mode_enabled: common.ownership_mode_enabled,
        limits: counters::Limits {
            destructive: common.hourly_destructive_limit,
            grant: common.hourly_grant_limit,
            first_password: common.hourly_first_password_limit,
            emergency_quota: common.emergency_quota,
        },
        time_zone: common.time_zone,
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
        // docs/05 acilis kontrolleri (kapsam DN → GUID, ADR-060); gecmezse AD connector'i baslamaz
        let checks = ad::startup_checks(&mut ldap, &scope)
            .await
            .map_err(|e| e.to_string())?;
        let snapshot = ad::read_catalog(&mut ldap, &scope, &checks)
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

// Acilistaki uc fallible adim (run()'u ≤50 satir tutar, security.md).
async fn startup() -> Result<(Env, PgPool, Arc<AtomicBool>, Arc<Notify>), String> {
    let env = load_env()?;
    let pool = prepare_pool(&env.database_url).await?;
    let (stop, wake) = spawn_sigterm_watcher()?;
    Ok((env, pool, stop, wake))
}

async fn run() -> ExitCode {
    let (env, pool, stop, wake) = match startup().await {
        Ok(parts) => parts,
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
    let mut next_tick = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if let Err(e) = heartbeat::touch() {
            eprintln!("worker: nabız dosyasına yazılamadı: {e}");
        }
        tick_if_due(&pool, &env.time_zone, &mut next_tick).await;
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

// Zamanlayici tiki (ADR-028): hata surec durdurmaz, sonraki tikte yeniden denenir.
async fn tick_if_due(pool: &PgPool, time_zone: &str, next_tick: &mut Instant) {
    if Instant::now() < *next_tick {
        return;
    }
    *next_tick = Instant::now() + TICK_INTERVAL;
    match scheduler::tick(pool, time_zone).await {
        Ok(0) => {}
        Ok(opened) => println!("worker: zamanlayıcı {opened} geçiş işi açtı"),
        Err(e) => eprintln!("worker: zamanlayıcı tiki başarısız: {e}"),
    }
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
    let engine_env = engine::EngineEnv {
        time_zone: &env.time_zone,
        mode: env.write_mode,
        aead_key: &env.aead_key,
        ad_ca_file: env.ad_ca_file.as_deref(),
        worker_id,
        sensitive_mapping_enabled: env.sensitive_mapping_enabled,
        first_login_change_required: env.first_login_change_required,
        ownership_mode_enabled: env.ownership_mode_enabled,
        limits: env.limits,
    };
    let run = engine::run_job(pool, job, &engine_env).await;
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
                queue::defer(pool, job, worker_id, &reason, retry).await,
                true,
            )
        }
        // ADR-050: fren; hedefe hicbir sey yazilmadi, is pencerenin acilisini bekler
        Err(engine::JobError::Throttled(blocked)) => {
            println!("worker: iş {} fren nedeniyle bekliyor: {blocked}", job.id);
            let retry = blocked.retry_after_seconds;
            (
                queue::defer(pool, job, worker_id, &blocked.job_error(), retry).await,
                false,
            )
        }
        Err(engine::JobError::NeedsIntervention(reason)) => {
            eprintln!("worker: iş {} müdahale gerekiyor: {reason}", job.id);
            (queue::intervene(pool, job, worker_id, &reason).await, false)
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
        // Zimbra hedefi: connector yok, is "hedefe yazilmadi" sonucuyla biter; AD hedefi
        // yapilandirma ister ve bu testte AD yoktur.
        let zimbra: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'zimbra'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let job_id = test_support::enqueue(&pool, seed.identity, zimbra, 1).await;
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
        assert!(result.contains("connector'ı yok"), "{result}");

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
