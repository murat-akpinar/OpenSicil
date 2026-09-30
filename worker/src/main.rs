mod common_settings;
mod db;
// 3a motoru tuketene kadar yalnizca testler cagirir (ADR-038); backend kopyasiyla birebir ayni.
#[allow(dead_code)]
mod desired_state;
mod heartbeat;

use std::process::ExitCode;
use std::time::Duration;

use sqlx::PgPool;
use tokio::signal::unix::{signal, SignalKind};

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

// Ortak ayarlar acilista dogrulanir ve loglanir; backend'in satiriyla yan
// yana konunca iki servisin sapmasi gorulur (ADR-039). Sayac ve sahiplenme
// kararlari Faz 3'te bu degerlerle verilir. DATABASE_URL'i doner.
fn load_env() -> Result<String, String> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "worker: ortam değişkeni eksik: DATABASE_URL".to_string())?;
    let common = common_settings::CommonSettings::from_env().map_err(|e| format!("worker: {e}"))?;
    println!("worker: ortak ayarlar: {common}");
    Ok(database_url)
}

async fn run() -> ExitCode {
    let database_url = match load_env() {
        Ok(url) => url,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let pool = match prepare_pool(&database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("worker: SIGTERM işleyicisi kurulamadı: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("worker: başladı, {POLL_INTERVAL:?} aralıkla yoklanıyor");

    loop {
        if let Err(e) = heartbeat::touch() {
            eprintln!("worker: nabız dosyasına yazılamadı: {e}");
        }

        // Gerçek kuyruk okuma ve hedef sistem işleri Faz 3'te eklenir; burada
        // yalnızca canlılık nabzı ve veritabanı bağlantısı doğrulanır.
        if let Err(e) = sqlx::query("SELECT 1").execute(&pool).await {
            eprintln!("worker: veritabanı kontrolü başarısız: {e}");
        }

        // SIGTERM'de dongu elindeki turu bitirip cikar; yeni tur almaz (ADR-061 madde 1).
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = sigterm.recv() => {
                println!("worker: SIGTERM alındı, kapanıyor");
                break;
            }
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    // Gercek Postgres gerektirir (ADR-070): DATABASE_URL, testler icin
    // ayrilmis bir veritabanina isaret etmeli (proje .env'i degil). POLL_INTERVAL
    // 5 sn oldugundan dongu turunu SIGTERM ile erken kesip beklemeyi kisaltir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn run_completes_one_tick_then_exits_on_sigterm() {
        std::env::var("DATABASE_URL").expect("DATABASE_URL testler için ayarlanmalı");
        // SAFETY: tek is parcacikli, ayni degiskenleri eszamanli degistiren
        // baska test yok (backend migrate testindeki desenle ayni).
        unsafe {
            for (name, value) in common_settings::tests::ENV_EXAMPLE_DEFAULTS {
                std::env::set_var(name, value);
            }
        }

        let handle = tokio::spawn(run());
        tokio::time::sleep(Duration::from_millis(200)).await;
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
    }
}
