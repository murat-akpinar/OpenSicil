mod db;
mod heartbeat;

use std::process::ExitCode;
use std::time::Duration;

use tokio::signal::unix::{signal, SignalKind};

const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("worker-health") {
        return heartbeat::check();
    }

    let database_url = match std::env::var("DATABASE_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("worker: DATABASE_URL ortam değişkeni eksik");
            return ExitCode::FAILURE;
        }
    };

    let pool = match db::connect_pool(&database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("worker: veritabanına bağlanılamadı: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = db::check_schema_ready(&pool).await {
        eprintln!("worker: şema hazır değil: {e}");
        return ExitCode::FAILURE;
    }

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
