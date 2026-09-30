use std::process::ExitCode;

use axum::routing::get;
use axum::Router;
use tokio::signal::unix::{signal, SignalKind};

use crate::health;
use crate::logging;
use crate::web::{self, AppState};

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health::health))
        .with_state(state.pool.clone())
        .merge(web::routes().with_state(state))
        .layer(axum::middleware::from_fn(logging::log_requests))
}

pub async fn run() -> ExitCode {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("backend: DATABASE_URL ortam değişkeni eksik");
            return ExitCode::FAILURE;
        }
    };

    let aead_key = match std::env::var("AEAD_MASTER_KEY") {
        Ok(v) => match crate::crypto::parse_master_key(&v) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("backend: {e}");
                return ExitCode::FAILURE;
            }
        },
        Err(_) => {
            eprintln!("backend: AEAD_MASTER_KEY ortam değişkeni eksik");
            return ExitCode::FAILURE;
        }
    };

    let public_url = match std::env::var("PUBLIC_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("backend: PUBLIC_URL ortam değişkeni eksik");
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

    let app = build_router(AppState {
        pool,
        aead_key,
        public_url,
    });

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    fn test_state() -> AppState {
        AppState {
            pool: lazy_unreachable_pool(),
            aead_key: [0u8; crate::crypto::KEY_LEN],
            public_url: "https://localhost".to_string(),
        }
    }

    #[tokio::test]
    async fn health_route_is_wired_through_router() {
        let app = build_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn unknown_route_returns_404_through_logging_middleware() {
        let app = build_router(test_state());
        let response = app
            .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn login_route_is_wired_through_router() {
        let app = build_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn config_route_redirects_to_login_without_session_cookie() {
        let app = build_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/login");
    }

    #[tokio::test]
    async fn shutdown_signal_resolves_on_sigterm() {
        let handle = tokio::spawn(shutdown_signal());
        // sinyal dinleyicisi kurulana kadar kisa bir bekleme
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let pid = std::process::id();
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .expect("kill çağrılamadı");
        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("shutdown_signal zaman aşımına uğradı")
            .expect("görev panikledi");
    }
}
