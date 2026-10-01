use std::process::ExitCode;

use axum::routing::get;
use axum::Router;
use tokio::signal::unix::{signal, SignalKind};

use crate::health;
use crate::logging;
use crate::web::{self, AppState};

// Operator reddi (ADR-059) web rotalarinin tamamini sarar: her istekte kimlik durumu.
pub(crate) fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health::health))
        .with_state(state.pool.clone())
        // Statik varlıklar (ADR-088) operatör oturumu istemez: CSS/font giriş ekranında da gerekir
        .merge(crate::assets::routes())
        .merge(
            web::routes()
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    crate::operator_guard::enforce,
                ))
                .with_state(state),
        )
        .layer(axum::middleware::from_fn(logging::log_requests))
        // En dista: govdesiz 404/405/500 yanitlari kabugun icindeki sayfaya cevrilir
        .layer(axum::middleware::from_fn(crate::errors::error_page))
}

struct Config {
    database_url: String,
    aead_key: [u8; crate::crypto::KEY_LEN],
    blind_index_key: [u8; crate::crypto::KEY_LEN],
    public_url: String,
    time_zone: String,
    change_set_threshold: usize,
    approval_timelock_hours: u32,
}

// Butun ortam degiskenleri acilista dogrulanir: eksik anahtar ya da bozuk ortak
// ayar ilk istekte degil kurulumda goze carpar (ADR-010, ADR-039).
fn load_config() -> Result<Config, String> {
    let database_url = env_required("DATABASE_URL")?;
    let aead_key = key_from_env("AEAD_MASTER_KEY")?;
    let blind_index_key = key_from_env("BLIND_INDEX_KEY")?;
    let public_url = env_required("PUBLIC_URL")?;
    let common = crate::common_settings::CommonSettings::from_env()?;
    println!("backend: ortak ayarlar: {common}");
    Ok(Config {
        database_url,
        aead_key,
        blind_index_key,
        public_url,
        time_zone: common.time_zone,
        change_set_threshold: crate::change_set::threshold_from_env()?,
        approval_timelock_hours: crate::change_set::timelock_from_env()?,
    })
}

fn env_required(var: &str) -> Result<String, String> {
    std::env::var(var).map_err(|_| format!("{var} ortam değişkeni eksik"))
}

// Ayar okuma + havuz; run()'u <= 50 satir tutar (security.md).
async fn startup() -> Result<AppState, String> {
    let c = load_config()?;
    let pool = prepare_pool(&c.database_url, &c.time_zone).await?;
    Ok(AppState {
        pool,
        aead_key: c.aead_key,
        blind_index_key: c.blind_index_key,
        public_url: c.public_url,
        time_zone: c.time_zone,
        change_set_threshold: c.change_set_threshold,
        approval_timelock_hours: c.approval_timelock_hours,
    })
}

pub async fn run() -> ExitCode {
    let app = match startup().await {
        Ok(state) => build_router(state),
        Err(e) => {
            eprintln!("backend: {e}");
            return ExitCode::FAILURE;
        }
    };

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

fn key_from_env(var: &str) -> Result<[u8; crate::crypto::KEY_LEN], String> {
    crate::crypto::parse_key(var, &env_required(var)?)
}

// Baglanti, sema (ADR-061 madde 3) ve saat dilimi kontrolu tek yerde (run() ≤ 50 satir).
async fn prepare_pool(database_url: &str, time_zone: &str) -> Result<sqlx::PgPool, String> {
    let pool = crate::db::connect_pool(database_url)
        .await
        .map_err(|e| format!("veritabanına bağlanılamadı: {e}"))?;
    crate::db::check_schema_ready(&pool)
        .await
        .map_err(|e| format!("şema hazır değil: {e}"))?;
    crate::db::check_time_zone(&pool, time_zone).await?;
    Ok(pool)
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
            blind_index_key: [0u8; crate::crypto::KEY_LEN],
            public_url: "https://localhost".to_string(),
            time_zone: "Europe/Istanbul".to_string(),
            change_set_threshold: crate::change_set::DEFAULT_THRESHOLD,
            approval_timelock_hours: 0,
        }
    }

    // ADR-059 madde 1: her istekte kontrol; ayrilmis operatorun oturumu duser, 403.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn departed_operator_is_rejected_on_any_request_and_session_is_dropped() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        sqlx::query(
            "UPDATE identities SET username = 'ayse.yilmaz', end_at = now() - interval '1 hour' \
             WHERE id = $1",
        )
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
        let operator = |username: &str| crate::operator_session::Operator {
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: format!("{username}@example.com"),
            authorities: vec!["hr".to_string()],
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let departed_token =
            crate::operator_session::create_session(&pool, &operator("ayse.yilmaz"))
                .await
                .unwrap();
        let active_token = crate::operator_session::create_session(&pool, &operator("break.glass"))
            .await
            .unwrap();
        let app = build_router(AppState {
            pool: pool.clone(),
            ..test_state()
        });
        let request = |token: &str| {
            Request::builder()
                .uri("/login")
                .header("cookie", format!("{OPERATOR_COOKIE}={token}"))
                .body(Body::empty())
                .unwrap()
        };

        let response = app.clone().oneshot(request(&departed_token)).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            response
                .headers()
                .get("set-cookie")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("Max-Age=0"),
            "çerez temizlenmeli"
        );
        assert!(
            crate::operator_session::validate_session(&pool, &departed_token)
                .await
                .unwrap()
                .is_none(),
            "oturum silinmeli"
        );
        let response = app.oneshot(request(&active_token)).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "eşleşen kimliği olmayan operatör serbest"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    const OPERATOR_COOKIE: &str = crate::cookie::OPERATOR_SESSION_COOKIE_NAME;

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
        let body = body_text(response).await;
        assert!(body.contains("404"), "durum kodu sayfada yok: {body}");
        assert!(body.contains("Sayfa bulunamadı"), "404 sayfası çizilmedi");
        assert!(body.contains("/static/app.css"), "kabuk yüklenmedi");
    }

    // `/api/*` makine ucudur: hatasi HTML sayfaya cevrilmez
    #[tokio::test]
    async fn the_api_health_route_answers_without_an_html_error_page() {
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
        assert!(!body_text(response).await.contains("<html"));
    }

    #[tokio::test]
    async fn a_wrong_method_gets_the_error_page_not_a_blank_405() {
        let app = build_router(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(body_text(response).await.contains("405"));
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
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
