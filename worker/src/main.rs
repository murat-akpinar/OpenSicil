#[macro_use]
mod log;
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
mod normalize;
mod queue;
mod read_lane;
mod reconcile;
mod scheduler;
mod scope;
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

// Acilis kablolamasi (ADR-131 madde 1); is ayarlari `Operational`'da.
struct Env {
    database_url: String,
    aead_key: [u8; crypto::KEY_LEN],
    ad_ca_file: Option<String>,
}

// ADR-131 madde 6: isletme ayarlari is/tik basina tablodan okunur; ekrandaki
// degisiklik worker yeniden baslatilmadan etkili olur, bayat deger penceresi yok.
struct Operational {
    map: HashMap<String, String>,
    write_mode: writes::Mode,
    // ADR-019: ilk paroladan sonra pwdLastSet 0; varsayilan acik
    first_login_change_required: bool,
    // Ortak yedi ayar (ADR-039 kurallari, yeri ADR-131): backend ayni satiri okur
    common: common_settings::CommonSettings,
}

fn parse_bool_setting(map: &HashMap<String, String>, name: &str) -> Result<Option<bool>, String> {
    match map.get(name).map(|v| v.trim()) {
        Some("true") | Some("1") => Ok(Some(true)),
        Some("false") | Some("0") => Ok(Some(false)),
        Some(other) => Err(format!("{name} true ya da false olmalı, '{other}' geldi")),
        None => Ok(None),
    }
}

// Bozuk deger isi `failed` yapar ve nedeni yazar; sessiz varsayilana dusulmez (ADR-131 madde 5).
fn operational_of(map: HashMap<String, String>) -> Result<Operational, String> {
    let dry_run = parse_bool_setting(&map, "DRY_RUN")?
        .ok_or_else(|| "DRY_RUN Ayarlar ekranında tanımlı değil".to_string())?;
    let first_login_change_required =
        parse_bool_setting(&map, "FIRST_LOGIN_CHANGE_REQUIRED")?.unwrap_or(true);
    let common = common_settings::CommonSettings::from_lookup(|name| map.get(name).cloned())?;
    Ok(Operational {
        common,
        map,
        write_mode: writes::Mode { dry_run },
        first_login_change_required,
    })
}

async fn load_operational(pool: &PgPool) -> Result<Operational, String> {
    let map = common_settings::load_operational(pool)
        .await
        .map_err(|e| format!("işletme ayarları okunamadı: {e}"))?;
    operational_of(map)
}

// DRY_RUN (ADR-054): acikken hedefe hicbir sey yazilmaz; unutulmasin diye her acilista loglanir.
async fn log_operational_at_startup(pool: &PgPool) {
    match load_operational(pool).await {
        Ok(ops) => {
            log_info!(
                "worker: ortak ayarlar: {}; FIRST_LOGIN_CHANGE_REQUIRED={}",
                ops.common,
                ops.first_login_change_required
            );
            if ops.write_mode.dry_run {
                log_info!("worker: KURU ÇALIŞTIRMA açık — hedefe hiçbir şey yazılmaz (ADR-054)");
            }
        }
        Err(e) => log_error!("worker: {e}"),
    }
}

fn load_env() -> Result<Env, String> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "worker: ortam değişkeni eksik: DATABASE_URL".to_string())?;
    let aead_key = std::env::var("AEAD_MASTER_KEY")
        .map_err(|_| "worker: ortam değişkeni eksik: AEAD_MASTER_KEY".to_string())
        .and_then(|v| {
            crypto::parse_key("AEAD_MASTER_KEY", &v).map_err(|e| format!("worker: {e}"))
        })?;
    Ok(Env {
        database_url,
        aead_key,
        ad_ca_file: std::env::var("AD_CA_FILE").ok(),
    })
}

// Katalog yenileme artik OKUMA SERIDINDE calisir (ADR-051): acilista yalnizca
// istek yazilir, taramayi ayri gorev yapar; yazma seridi uzun bir LDAP okumasini
// beklemez. AD yapilandirilmamissa istek de yazilmaz.
async fn request_catalog_refresh_at_startup(pool: &PgPool) {
    match read_lane::ad_target(pool).await {
        Ok(Some(target)) => {
            match read_lane::request(pool, read_lane::CATALOG_REFRESH, target).await {
                Ok(true) => {
                    log_info!("worker: açılışta katalog yenileme isteği okuma şeridine yazıldı")
                }
                Ok(false) => log_info!("worker: katalog yenileme isteği zaten açık"),
                Err(e) => log_error!("worker: katalog yenileme isteği yazılamadı: {e}"),
            }
        }
        Ok(None) => log_error!("worker: AD hedef sistemi bulunamadı, katalog yenileme atlandı"),
        Err(e) => log_error!("worker: hedef sistem okunamadı: {e}"),
    }
}

// Okuma seridi gorevi (ADR-051): kendi dongusu, kendi LDAP baglantisi; hedefe
// hicbir sey yazmaz, sayac okumaz. Ayni anda tek is.
async fn run_read_lane(pool: PgPool, env: Arc<Env>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        let job = match read_lane::claim(&pool).await {
            Ok(Some(job)) => job,
            Ok(None) => {
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
            Err(e) => {
                log_error!("worker: okuma şeridi kuyruğu okunamadı: {e}");
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
        };
        let work = async {
            match job.kind.as_str() {
                read_lane::CATALOG_REFRESH => {
                    refresh_catalog(&pool, &env, job.target_system_id).await
                }
                read_lane::RECONCILE => {
                    run_reconcile(&pool, &env, job.target_system_id, job.id).await
                }
                read_lane::MANAGE_DIFF => run_manage_diff(&pool, &env, job.target_system_id).await,
                other => Err(format!("bilinmeyen okuma işi türü: {other}")),
            }
        };
        // Tarama surerken kira uzar (ADR-062): 5 dakikadan uzun okuma kendi
        // kirasina dusup geri alinmaz.
        let outcome = read_lane::with_lease(&pool, job.id, work).await;
        match &outcome {
            Ok(text) => log_info!("worker: okuma şeridi {} bitti: {text}", job.kind),
            Err(e) => log_error!("worker: okuma şeridi {} başarısız: {e}", job.kind),
        }
        if let Err(e) = read_lane::finish(&pool, job.id, outcome).await {
            log_error!("worker: okuma işi sonucu yazılamadı: {e}");
        }
    }
}

// Okuma islerinin ortak acilisi: ayarlar → kapsam → baglanti → acilis kontrolleri
// (kapsam DN → GUID, ADR-060). ADR-061: DC'ye ulasilamiyorsa surec cikmaz, is
// basarisiz olur ve bir sonraki istekte yeniden denenir.
type OpenAd = (
    ldap3::Ldap,
    ad::ManagedScope,
    ad::StartupChecks,
    Operational,
);

async fn open_ad(pool: &PgPool, env: &Env) -> Result<OpenAd, String> {
    let ops = load_operational(pool).await?;
    let cfg = ad::load_config(pool, &env.aead_key, env.ad_ca_file.as_deref())
        .await
        .map_err(|e| format!("AD ayarları okunamadı: {e}"))?
        .ok_or_else(|| "AD yapılandırılmamış".to_string())?;
    let scope = ad::parse_scope(|name| ops.map.get(name).cloned())
        .map_err(|e| format!("yönetilen kapsam geçersiz: {e}"))?;
    let mut ldap = ad::connect(&cfg).await.map_err(|e| e.to_string())?;
    let checks = ad::startup_checks(&mut ldap, &scope)
        .await
        .map_err(|e| e.to_string())?;
    Ok((ldap, scope, checks, ops))
}

// Mutabakat (ADR-099): yonetilen kullanici OU'larindaki hesaplar okunur ve
// `account_links` ile karsilastirilir. Hedefe yazilmaz, denetim kaydina
// dokunulmaz, fren sayaclari harcanmaz (ADR-051).
async fn run_reconcile(
    pool: &PgPool,
    env: &Env,
    target: i64,
    read_job_id: i64,
) -> Result<String, String> {
    let (mut ldap, scope, _checks, _ops) = open_ad(pool, env).await?;
    // ADR-106 madde 5: TC kimlik no yalnizca Yapilandirma'da oznitelik verildiyse okunur
    let national_id_attr = ad::national_id_attribute(pool).await?;
    let accounts = ad::read_accounts(&mut ldap, &scope, national_id_attr.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    ldap.unbind().await.ok();

    let links = reconcile::load_links(pool, target)
        .await
        .map_err(|e| format!("hesap bağlantıları okunamadı: {e}"))?;
    let findings = reconcile::compare(&accounts, &links);
    let counts = reconcile::store(pool, target, read_job_id, &findings, &env.aead_key)
        .await
        .map_err(|e| format!("mutabakat bulguları yazılamadı: {e}"))?;
    // ADR-112 madde 1: bulgular yazildiktan sonra bagli kimliklerin bos alanlari dolar
    let filled = reconcile::fill_linked_identities(pool, target).await?;
    // ADR-120 madde 5: yer tutucu rol de "bos" sayilir, AD'deki unvandan dolar
    let roles = reconcile::fill_placeholder_roles(pool, target).await?;
    Ok(format!(
        "{} hesap tarandı: {} yönetiliyor, {} gözlemde, {} yönetilmeyen, {} kayıp; \
         {filled} kimlikte boş alan AD'den doldu, {roles} kimlikte rol unvandan doldu",
        accounts.len(),
        counts.managed,
        counts.observed,
        counts.unmanaged,
        counts.missing
    ))
}

// Toplu yonetime almanin fark hesabi (ADR-051 ucuncu tur; ADR-087 kuru yol): hedefin
// gozlem baglantilari icin motorun kuru yolu tek LDAP baglantisiyla calisir, metin ve
// "esige giren fark var mi" baglantiya yazilir. Hedefe yazilmaz, sayaclar degismez;
// hesaplanamayan baglanti nedeniyle isaretlenir, is dusmez.
async fn run_manage_diff(pool: &PgPool, env: &Env, target: i64) -> Result<String, String> {
    let (mut ldap, _scope, _checks, ops) = open_ad(pool, env).await?;
    let engine_env = engine_env_of(env, "read-lane", &ops);
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT identity_id FROM account_links WHERE target_system_id = $1 \
         AND mode = 'observed' AND deleted_by_us_at IS NULL ORDER BY identity_id",
    )
    .bind(target)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("gözlem bağlantıları okunamadı: {e}"))?;
    let (mut applies, mut failed) = (0, 0);
    for id in &ids {
        let diff = engine::observed_diff(pool, &engine_env, &mut ldap, *id, target).await;
        let (text, flag) = match diff {
            Ok(d) => {
                applies += usize::from(d.applies);
                (d.text, Some(d.applies))
            }
            Err(e) => {
                failed += 1;
                (format!("fark hesaplanamadı: {e}"), None)
            }
        };
        sqlx::query(
            "UPDATE account_links SET observed_diff = $3, observed_diff_applies = $4, \
             observed_diff_at = now() WHERE identity_id = $1 AND target_system_id = $2",
        )
        .bind(id)
        .bind(target)
        .bind(text)
        .bind(flag)
        .execute(pool)
        .await
        .map_err(|e| format!("gözlem farkı yazılamadı: {e}"))?;
    }
    ldap.unbind().await.ok();
    Ok(format!(
        "{} gözlem bağlantısı: {applies} uygulanacak fark, {failed} hesaplanamadı",
        ids.len()
    ))
}

// docs/03 katalog.
async fn refresh_catalog(pool: &PgPool, env: &Env, target: i64) -> Result<String, String> {
    let (mut ldap, scope, checks, _ops) = open_ad(pool, env).await?;
    let snapshot = ad::read_catalog(&mut ldap, &scope, &checks)
        .await
        .map_err(|e| e.to_string())?;
    ldap.unbind().await.ok();
    let counts = catalog::sync_snapshot(pool, target, &snapshot)
        .await
        .map_err(|e| format!("katalog yazılamadı: {e}"))?;
    Ok(format!(
        "{} OU, {} grup, {} kayıp; {} yasaklı grup kataloğa alınmadı",
        counts.ous,
        counts.groups,
        counts.marked_missing,
        snapshot.forbidden.len()
    ))
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
        log_info!("worker: SIGTERM alındı, eldeki iş bitirilip çıkılıyor");
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
            log_error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let worker_id = worker_id();
    log_info!("worker: {worker_id} başladı, {POLL_INTERVAL:?} aralıkla yoklanıyor");
    let env = Arc::new(env);
    log_operational_at_startup(&pool).await;
    request_catalog_refresh_at_startup(&pool).await;
    tokio::spawn(run_read_lane(
        pool.clone(),
        Arc::clone(&env),
        Arc::clone(&stop),
    ));

    // Yazma seridi tek sirada (ADR-047): bir is bitmeden digeri alinmaz;
    // kuyruk bosalinca 5 sn beklenir. Erisilemeyen hedefin isleri bir yoklama
    // suresi boyunca alinmaz, sonra tek isle yeniden yoklanir (ADR-052 madde 3).
    let mut unreachable: HashMap<i64, Instant> = HashMap::new();
    let mut next_tick = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if let Err(e) = heartbeat::touch() {
            log_error!("worker: nabız dosyasına yazılamadı: {e}");
        }
        tick_if_due(&pool, &worker_id, &mut next_tick).await;
        let skip = unreachable_targets(&unreachable, Instant::now());
        match queue::claim(&pool, &worker_id, &skip).await {
            Ok(Some(job)) => {
                if process_job(&pool, &job, &worker_id, &env).await {
                    unreachable.insert(job.target_system_id, Instant::now() + POLL_INTERVAL);
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => log_error!("worker: kuyruk okunamadı: {e}"),
        }
        let _ = tokio::time::timeout(POLL_INTERVAL, wake.notified()).await;
    }
    ExitCode::SUCCESS
}

// Zamanlayici tiki (ADR-028): hata surec durdurmaz, sonraki tikte yeniden denenir.
async fn tick_if_due(pool: &PgPool, worker_id: &str, next_tick: &mut Instant) {
    if Instant::now() < *next_tick {
        return;
    }
    *next_tick = Instant::now() + TICK_INTERVAL;
    let ops = match load_operational(pool).await {
        Ok(ops) => ops,
        Err(e) => return log_error!("worker: zamanlayıcı tiki atlandı: {e}"),
    };
    let time_zone = ops.common.time_zone.as_str();
    // F-19 / ADR-054: mod ve son gorulme veritabanina, dakikada bir (metrik ucu + panel)
    if let Err(e) = heartbeat::record(pool, worker_id, ops.write_mode.dry_run).await {
        log_error!("worker: durum satırı yazılamadı: {e}");
    }
    match scheduler::tick(pool, time_zone).await {
        Ok(0) => {}
        Ok(opened) => log_info!("worker: zamanlayıcı {opened} geçiş işi açtı"),
        Err(e) => log_error!("worker: zamanlayıcı tiki başarısız: {e}"),
    }
    // ADR-051/099: gece mutabakati okuma seridine istek olarak yazilir (F-13)
    let slots = ops.map.get(common_settings::SCAN_AT).map(String::as_str);
    let slots = match common_settings::parse_scan_times(slots.unwrap_or_default()) {
        Ok(slots) => slots,
        Err(e) => return log_error!("worker: gece taraması açılmadı: {e}"),
    };
    match scheduler::open_nightly_scans(pool, time_zone, &slots).await {
        Ok(0) => {}
        Ok(opened) => log_info!("worker: gece mutabakatı için {opened} okuma işi açıldı"),
        Err(e) => log_error!("worker: {e}"),
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
fn engine_env_of<'a>(
    env: &'a Env,
    worker_id: &'a str,
    ops: &'a Operational,
) -> engine::EngineEnv<'a> {
    engine::EngineEnv {
        time_zone: &ops.common.time_zone,
        mode: ops.write_mode,
        settings: &ops.map,
        aead_key: &env.aead_key,
        ad_ca_file: env.ad_ca_file.as_deref(),
        worker_id,
        sensitive_mapping_enabled: ops.common.sensitive_mapping_enabled,
        first_login_change_required: ops.first_login_change_required,
        ownership_mode_enabled: ops.common.ownership_mode_enabled,
        limits: counters::Limits {
            destructive: ops.common.hourly_destructive_limit,
            grant: ops.common.hourly_grant_limit,
            first_password: ops.common.hourly_first_password_limit,
            emergency_quota: ops.common.emergency_quota,
        },
    }
}

async fn process_job(pool: &PgPool, job: &queue::ClaimedJob, worker_id: &str, env: &Env) -> bool {
    let run = match load_operational(pool).await {
        Ok(ops) => engine::run_job(pool, job, &engine_env_of(env, worker_id, &ops)).await,
        Err(e) => Err(engine::JobError::Failed(e)),
    };
    let (outcome, unreachable) = match run {
        Ok(result) => (
            queue::complete(pool, job, worker_id, &result)
                .await
                .map(|_| ()),
            false,
        ),
        Err(engine::JobError::Unreachable(reason)) => {
            log_error!(
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
            log_info!("worker: iş {} fren nedeniyle bekliyor: {blocked}", job.id);
            let retry = blocked.retry_after_seconds;
            (
                queue::defer(pool, job, worker_id, &blocked.job_error(), retry).await,
                false,
            )
        }
        Err(engine::JobError::NeedsIntervention(reason)) => {
            log_error!("worker: iş {} müdahale gerekiyor: {reason}", job.id);
            (queue::intervene(pool, job, worker_id, &reason).await, false)
        }
        Err(error) => {
            log_error!("worker: iş {} başarısız: {error}", job.id);
            (
                queue::fail(pool, job, worker_id, &error.to_string()).await,
                false,
            )
        }
    };
    if let Err(e) = outcome {
        log_error!("worker: iş {} sonucu yazılamadı: {e}", job.id);
    }
    unreachable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_settings_parse_or_fail_loudly() {
        // Ortak yedi ayar seed degerleriyle; test yalnizca worker'in kendi anahtarlarini oynatir
        let map = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            common_settings::tests::SEED_DEFAULTS
                .iter()
                .chain(pairs)
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let ops = operational_of(map(&[("DRY_RUN", "0")])).unwrap();
        assert!(!ops.write_mode.dry_run);
        assert!(ops.first_login_change_required, "verilmezse açık (ADR-019)");
        let err = operational_of(map(&[("DRY_RUN", "belki")])).err().unwrap();
        assert!(err.contains("DRY_RUN"), "{err}");
        assert!(
            operational_of(map(&[])).is_err(),
            "DRY_RUN yoksa iş yürümez"
        );
        assert!(operational_of(map(&[
            ("DRY_RUN", "true"),
            ("FIRST_LOGIN_CHANGE_REQUIRED", "")
        ]))
        .is_err());
    }

    // ADR-131 madde 6: ekrandaki degisiklik bir sonraki okumada etkili, yeniden baslatma yok.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn dry_run_is_read_per_job_from_the_table() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        assert!(
            load_operational(&pool).await.unwrap().write_mode.dry_run,
            "seed: açık"
        );
        test_support::set_setting(&pool, "DRY_RUN", "false").await;
        test_support::set_setting(&pool, "FIRST_LOGIN_CHANGE_REQUIRED", "false").await;
        let ops = load_operational(&pool).await.unwrap();
        assert!(!ops.write_mode.dry_run);
        assert!(!ops.first_login_change_required);
        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

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
            std::env::set_var(
                "AEAD_MASTER_KEY",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            );
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
        assert!(result.contains("connector'ı yok"), "{result}");
        // Sonuc satirini operator okur: `DesiredState` Debug dokumu basilmaz
        assert!(!result.contains("DesiredState"), "{result}");

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
