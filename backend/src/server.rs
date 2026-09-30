use std::process::ExitCode;

use axum::routing::get;
use axum::Router;
use tokio::signal::unix::{signal, SignalKind};

use crate::health;
use crate::logging;

pub async fn run() -> ExitCode {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("backend: DATABASE_URL ortam değişkeni eksik");
            return ExitCode::FAILURE;
        }
    };

    let pool = match crate::db::connect_pool(&database_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("backend: veritabanına bağlanılamadı: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = crate::db::check_schema_ready(&pool).await {
        eprintln!("backend: şema hazır değil: {e}");
        return ExitCode::FAILURE;
    }

    let app = Router::new()
        .route("/api/health", get(health::health))
        .with_state(pool)
        .layer(axum::middleware::from_fn(logging::log_requests));

    let listener = match tokio::net::TcpListener::bind("0.0.0.0:8000").await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("backend: 8000 portu dinlenemedi: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("backend: 0.0.0.0:8000 dinleniyor");
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        eprintln!("backend: sunucu hatası: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

// SIGTERM'de yeni baglanti almayi durdurur, acik istekler biter (ADR-061 madde 1).
async fn shutdown_signal() {
    match signal(SignalKind::terminate()) {
        Ok(mut sigterm) => {
            sigterm.recv().await;
            println!("backend: SIGTERM alındı, açık istekler bitiriliyor");
        }
        Err(e) => eprintln!("backend: SIGTERM işleyicisi kurulamadı: {e}"),
    }
}
