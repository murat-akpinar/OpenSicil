use askama::Template;
use axum::extract::{Form, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use sqlx::PgPool;

use crate::cookie::{clear_cookie_header, get_cookie, set_cookie_header, SESSION_COOKIE_NAME};
use crate::session::SESSION_LIFETIME_HOURS;

const MIN_PASSWORD_LENGTH: usize = 12;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub aead_key: [u8; crate::crypto::KEY_LEN],
}

// --- START FEATURE: bootstrap-admin ---
// ADR-068: admin/admin bootstrap girisi, ilk girişte zorunlu parola değişimi,
// yalnızca Yapılandırma sayfasına (AD/Zimbra/OIDC bağlantı ayarları) erişim.

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: String,
}

#[derive(Template)]
#[template(path = "change_password.html")]
struct ChangePasswordTemplate {
    error: String,
}

#[derive(Template)]
#[template(path = "config.html")]
struct ConfigTemplate {
    ad_host: String,
    ad_bind_dn: String,
    ad_service_password_set: bool,
    zimbra_url: String,
    zimbra_admin_password_set: bool,
    oidc_issuer: String,
    oidc_client_id: String,
    oidc_client_secret_set: bool,
}

fn render<T: Template>(tmpl: &T) -> Response {
    match tmpl.render() {
        Ok(body) => Html(body).into_response(),
        Err(e) => {
            eprintln!("web: şablon render edilemedi: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// Gecersiz/eksik oturumda Yapilandirma ve parola degistirme ekranlarina hic girilmez.
struct BootstrapSession;

impl IntoResponse for BootstrapSessionRejection {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

struct BootstrapSessionRejection;

impl FromRequestParts<AppState> for BootstrapSession {
    type Rejection = BootstrapSessionRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token =
            get_cookie(&parts.headers, SESSION_COOKIE_NAME).ok_or(BootstrapSessionRejection)?;
        let valid = crate::session::validate_session(&state.pool, &token)
            .await
            .unwrap_or(false);
        if valid {
            Ok(BootstrapSession)
        } else {
            Err(BootstrapSessionRejection)
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", post(logout))
        .route(
            "/change-password",
            get(change_password_form).post(change_password_submit),
        )
        .route("/config", get(config_form).post(config_submit))
}

async fn login_form() -> Response {
    render(&LoginTemplate {
        error: String::new(),
    })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_submit(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    match crate::bootstrap_account::verify_login(&state.pool, &form.username, &form.password).await
    {
        Ok(true) => {}
        Ok(false) => {
            return render(&LoginTemplate {
                error: "Kullanıcı adı ya da parola yanlış".to_string(),
            });
        }
        Err(e) => {
            eprintln!("web: giriş kontrolü başarısız: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    let token = match crate::session::create_session(&state.pool).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("web: oturum oluşturulamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    with_session_cookie(&token, Redirect::to("/change-password"))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = get_cookie(&headers, SESSION_COOKIE_NAME) {
        let _ = crate::session::delete_session(&state.pool, &token).await;
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        cookie_header_value(&clear_cookie_header(SESSION_COOKIE_NAME)),
    );
    (response_headers, Redirect::to("/login")).into_response()
}

async fn change_password_form(_session: BootstrapSession) -> Response {
    render(&ChangePasswordTemplate {
        error: String::new(),
    })
}

#[derive(Deserialize)]
struct ChangePasswordForm {
    new_password: String,
    confirm_password: String,
}

async fn change_password_submit(
    _session: BootstrapSession,
    State(state): State<AppState>,
    Form(form): Form<ChangePasswordForm>,
) -> Response {
    if form.new_password != form.confirm_password {
        return render(&ChangePasswordTemplate {
            error: "Parolalar eşleşmiyor".to_string(),
        });
    }
    if form.new_password.chars().count() < MIN_PASSWORD_LENGTH {
        return render(&ChangePasswordTemplate {
            error: format!("Parola en az {MIN_PASSWORD_LENGTH} karakter olmalı"),
        });
    }
    if let Err(e) = crate::bootstrap_account::set_password(&state.pool, &form.new_password).await {
        eprintln!("web: parola değiştirilemedi: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Redirect::to("/config").into_response()
}

async fn config_form(_session: BootstrapSession, State(state): State<AppState>) -> Response {
    match crate::bootstrap_account::must_change_password(&state.pool).await {
        Ok(true) => return Redirect::to("/change-password").into_response(),
        Ok(false) => {}
        Err(e) => {
            eprintln!("web: hesap durumu okunamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    match crate::settings::load(&state.pool).await {
        Ok(s) => render(&ConfigTemplate {
            ad_host: s.ad_host,
            ad_bind_dn: s.ad_bind_dn,
            ad_service_password_set: s.ad_service_password_set,
            zimbra_url: s.zimbra_url,
            zimbra_admin_password_set: s.zimbra_admin_password_set,
            oidc_issuer: s.oidc_issuer,
            oidc_client_id: s.oidc_client_id,
            oidc_client_secret_set: s.oidc_client_secret_set,
        }),
        Err(e) => {
            eprintln!("web: ayarlar okunamadı: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
struct ConfigForm {
    ad_host: String,
    ad_bind_dn: String,
    ad_service_password: String,
    zimbra_url: String,
    zimbra_admin_password: String,
    oidc_issuer: String,
    oidc_client_id: String,
    oidc_client_secret: String,
}

async fn config_submit(
    _session: BootstrapSession,
    State(state): State<AppState>,
    Form(form): Form<ConfigForm>,
) -> Response {
    let input = crate::settings::AppSettingsInput {
        ad_host: form.ad_host,
        ad_bind_dn: form.ad_bind_dn,
        ad_service_password: form.ad_service_password,
        zimbra_url: form.zimbra_url,
        zimbra_admin_password: form.zimbra_admin_password,
        oidc_issuer: form.oidc_issuer,
        oidc_client_id: form.oidc_client_id,
        oidc_client_secret: form.oidc_client_secret,
    };
    if let Err(e) = crate::settings::save(&state.pool, &state.aead_key, &input).await {
        eprintln!("web: ayarlar kaydedilemedi: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Redirect::to("/config").into_response()
}

// --- END FEATURE: bootstrap-admin ---

fn cookie_header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).expect("cerez basligi gecersiz karakter icermez")
}

fn with_session_cookie(token: &str, redirect: Redirect) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        cookie_header_value(&set_cookie_header(
            SESSION_COOKIE_NAME,
            token,
            SESSION_LIFETIME_HOURS * 3600,
        )),
    );
    (headers, redirect).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_app(pool: PgPool) -> Router {
        routes().with_state(AppState {
            pool,
            aead_key: [3u8; crate::crypto::KEY_LEN],
        })
    }

    fn form_request(method: &str, uri: &str, body: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, c);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    fn get_request(uri: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("GET").uri(uri);
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, c);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn location_of(response: &Response) -> &str {
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
    }

    // Set-Cookie header'inin "name=value" kismi; bir sonraki isteğin Cookie
    // basligina aynen konur (gercek tarayici davranisinin taklidi).
    fn set_cookie_value(response: &Response) -> String {
        response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    async fn body_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn full_bootstrap_login_and_config_flow() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::migrate::seed_bootstrap_account(&pool)
            .await
            .expect("bootstrap hesabı seed edilemedi");
        let app = test_app(pool.clone());

        // Yanlis parola: hata gosterilir, cerez kurulmaz.
        let response = app
            .clone()
            .oneshot(form_request(
                "POST",
                "/login",
                "username=admin&password=yanlis",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::SET_COOKIE).is_none());

        // Dogru parola: /change-password'e yonlendirir ve oturum cerezi kurar.
        let response = app
            .clone()
            .oneshot(form_request(
                "POST",
                "/login",
                "username=admin&password=admin",
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location_of(&response), "/change-password");
        let cookie = set_cookie_value(&response);

        // Parola degistirilmeden Yapilandirma'ya girilemez.
        let response = app
            .clone()
            .oneshot(get_request("/config", Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location_of(&response), "/change-password");

        // Eslesmeyen parolalar reddedilir.
        let response = app
            .clone()
            .oneshot(form_request(
                "POST",
                "/change-password",
                "new_password=guclu-yeni-parola&confirm_password=baska-bir-parola",
                Some(&cookie),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_string(response).await.contains("eşleşmiyor"));

        // Basarili parola degisimi /config'e yonlendirir.
        let response = app
            .clone()
            .oneshot(form_request(
                "POST",
                "/change-password",
                "new_password=guclu-yeni-parola&confirm_password=guclu-yeni-parola",
                Some(&cookie),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location_of(&response), "/config");

        // Artik Yapilandirma sayfasina girilebiliyor, henuz sir kayitli degil.
        let response = app
            .clone()
            .oneshot(get_request("/config", Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(!body.contains("kayıtlı"));

        // Ayarlari sirlarla kaydet.
        let form = "ad_host=dc1.example.org&ad_bind_dn=CN%3Dsvc&ad_service_password=cok-gizli-ad&\
                     zimbra_url=https%3A%2F%2Fzimbra.example.org&zimbra_admin_password=cok-gizli-zimbra&\
                     oidc_issuer=https%3A%2F%2Fidp.example.org&oidc_client_id=opensicil&oidc_client_secret=cok-gizli-oidc";
        let response = app
            .clone()
            .oneshot(form_request("POST", "/config", form, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location_of(&response), "/config");

        // Yeniden yuklenince degerler gorunur ama sirlar duz metin geri gelmez.
        let response = app
            .clone()
            .oneshot(get_request("/config", Some(&cookie)))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("dc1.example.org"));
        assert!(body.contains("kayıtlı"));
        assert!(!body.contains("cok-gizli-ad"));
        assert!(!body.contains("cok-gizli-zimbra"));
        assert!(!body.contains("cok-gizli-oidc"));

        // Cikis cerezi gecersiz kilar; sonraki istek yeniden girise yonlendirir.
        let response = app
            .clone()
            .oneshot(form_request("POST", "/logout", "", Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=0"));

        let response = app
            .clone()
            .oneshot(get_request("/config", Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location_of(&response), "/login");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
