use askama::Template;
use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use sqlx::PgPool;

use crate::cookie::{
    clear_cookie_header, get_cookie, set_cookie_header, OPERATOR_SESSION_COOKIE_NAME,
};
use crate::i18n::Lang;
use crate::identity_web::{allowed, forbidden, OperatorSession};
use crate::operator_session::{AuthSource, Operator};
use crate::shell::Shell;

const MIN_PASSWORD_LENGTH: usize = 12;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub aead_key: [u8; crate::crypto::KEY_LEN],
    // kimlik numarasi blind index'i (ADR-010); kimlik kayit formu kullanir
    pub blind_index_key: [u8; crate::crypto::KEY_LEN],
    pub public_url: String,
    // kurulum saat dilimi (ADR-039); operator reddi kimlik durumunu bununla turetir
    pub time_zone: String,
    // degisiklik seti esigi (ADR-031): backend'in kendi ortam degiskeni
    pub change_set_threshold: usize,
    // onay zaman kilidi, saat; 0 = kapali (ADR-026)
    pub approval_timelock_hours: u32,
}

// --- START FEATURE: bootstrap-admin ---
// ADR-068: admin/admin yerel hesabı, ilk girişte zorunlu parola değişimi.
// ADR-095 madde 3: hesap artık kalıcı break-glass yolu ve Sistem yöneticisi
// (`admin`) yetkisiyle gerçek bir operatör oturumu açar; form hiç gizlenmez.

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    lang: Lang,
    error: String,
    oidc_configured: bool,
}

// --- START FEATURE: oidc-login ---
#[derive(Template)]
#[template(path = "operator_home.html")]
struct OperatorHomeTemplate {
    lang: Lang,
    shell: Shell,
    // ADR-096 madde 3: ana sayfa gosterge paneli
    dash: crate::dashboard::Dashboard,
    identities: Vec<crate::identity::Listed>,
}

/// `?days=` panel penceresi (ADR-076); listede olmayan ya da sayi olmayan deger
/// varsayilana duser (metin alinir: `?days=abc` 400 degil varsayilan verir).
#[derive(Deserialize, Default)]
pub(crate) struct HomeQuery {
    days: Option<String>,
}

async fn render_operator_home(
    state: &AppState,
    lang: Lang,
    username: String,
    authorities: Vec<String>,
    query: HomeQuery,
) -> Response {
    let days = crate::dashboard::window(query.days.and_then(|d| d.trim().parse().ok()));
    let loaded = tokio::try_join!(
        crate::dashboard::load(&state.pool, &state.time_zone, days),
        crate::identity::recent(&state.pool, &state.time_zone),
    );
    match loaded {
        Ok((dash, identities)) => render(&OperatorHomeTemplate {
            lang,
            shell: Shell::from_parts(&username, &authorities),
            dash,
            identities,
        }),
        Err(e) => {
            eprintln!("web: gösterge paneli okunamadı: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
// --- END FEATURE: oidc-login ---

#[derive(Template)]
#[template(path = "change_password.html")]
struct ChangePasswordTemplate {
    lang: Lang,
    error: String,
}

#[derive(Template)]
#[template(path = "config.html")]
struct ConfigTemplate {
    lang: Lang,
    shell: Shell,
    ad_host: String,
    ad_bind_dn: String,
    ad_service_password_set: bool,
    ad_national_id_attribute: String,
    zimbra_url: String,
    zimbra_admin_password_set: bool,
    oidc_issuer: String,
    oidc_client_id: String,
    oidc_client_secret_set: bool,
}

pub(crate) fn render<T: Template>(tmpl: &T) -> Response {
    match tmpl.render() {
        Ok(body) => Html(body).into_response(),
        Err(e) => {
            eprintln!("web: şablon render edilemedi: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// Yapılandırma ve parola değiştirme Sistem yöneticisi yetkisi ister (ADR-005'teki
// "hedef sistem ayarları" yetkisi); yerel hesap bu yetkiyle girer (ADR-095 madde 3).
const CONFIG_AUTHORITIES: &[&str] = &[crate::oidc::ADMIN_AUTHORITY];

// Yerel hesabin OIDC `sub`'u yok; denetim ve oturum satirinda kapiyi belli eden
// sabit bir degerle durur (ADR-095 madde 5).
const LOCAL_SUBJECT: &str = "local:admin";

// AD kapisinda "subject" objectGUID'dir: kullanici adi degisse de denetim
// kaydindaki aktor ayni kalir (ADR-095 madde 6).
const AD_SUBJECT_PREFIX: &str = "ad:";

pub fn routes() -> Router<AppState> {
    Router::new()
        // Kabuktaki marka ve "Kimlikler" baglantisi, dil seciciden donus (local_path
        // varsayilani) ve tarayiciya elle yazilan adres koke gider; login_form
        // oturum varsa operator ana sayfasini, yoksa giris formunu verir
        .route("/", get(login_form))
        .route("/login", get(login_form).post(login_submit))
        .route("/logout", post(logout))
        .route(
            "/change-password",
            get(change_password_form).post(change_password_submit),
        )
        .route("/config", get(config_form).post(config_submit))
        // Dil secicisi (ADR-089): tercih operator oturumunda saklanir
        .route("/lang", post(set_lang))
        // --- START FEATURE: oidc-login ---
        .route("/oidc/login", get(oidc_login))
        .route("/oidc/callback", get(oidc_callback))
        // --- END FEATURE: oidc-login ---
        .merge(crate::identity_web::routes())
        .merge(crate::org_web::routes())
        .merge(crate::used_names::routes())
        .merge(crate::mapping_web::routes())
        .merge(crate::upcoming::routes())
        .merge(crate::reconcile::routes())
        .merge(crate::reports::routes())
        .merge(crate::interventions::routes())
        .merge(crate::deletions::routes())
        .merge(crate::search::routes())
        .merge(crate::first_password::routes())
}

// Ayarlar okunamazsa (DB gecici erisilemez) giris sayfasi yine de gosterilir:
// OIDC baglantisi gizlenir, bootstrap formu gorunur kalir (tek giris kapisi
// bir DB hiccup'inda tumden kapanmaz); asil dogrulama zaten girisi deneyince olur.
async fn render_login(pool: &PgPool, lang: Lang, error: String) -> Response {
    let settings = match crate::settings::load(pool).await {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("web: ayarlar okunamadı: {e}");
            None
        }
    };
    render(&LoginTemplate {
        lang,
        error,
        oidc_configured: settings.as_ref().is_some_and(crate::oidc::is_configured),
    })
}

// Operator oturumu zaten gecerliyse (cerez var ve DB'de suresi gecmemis),
// giris formunu degil dogrudan giris sonrasi sayfayi goster.
async fn login_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HomeQuery>,
) -> Response {
    if let Some(token) = get_cookie(&headers, OPERATOR_SESSION_COOKIE_NAME) {
        if let Ok(Some(operator)) =
            crate::operator_session::validate_session(&state.pool, &token).await
        {
            return render_operator_home(
                &state,
                operator.lang,
                operator.username,
                operator.authorities,
                query,
            )
            .await;
        }
    }
    render_login(&state.pool, Lang::from_headers(&headers), String::new()).await
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

// ADR-095 madde 1 ve 3: tek form, kapıyı kullanıcı adı seçer. Yerel break-glass
// hesabının adı tek ve sabittir (`admin`), başka her ad AD kapısına gider.
// "Önce AD, sonra yerel" sırası denenmedi: yanlış yazılan yerel parola AD'ye
// `admin` kullanıcısının parolası olarak gider (AD kilitleme politikasını
// tetikler) ve yanlış kullanıcı adı da yerel sayacı artırdığı için her AD girişi
// break-glass hesabını kilide yaklaştırırdı.
async fn login_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let lang = Lang::from_headers(&headers);
    if form.username == crate::bootstrap_account::BOOTSTRAP_USERNAME {
        local_login(&state, lang, &form).await
    } else {
        ad_login(&state, lang, &form).await
    }
}

async fn local_login(state: &AppState, lang: Lang, form: &LoginForm) -> Response {
    use crate::bootstrap_account::LoginOutcome;
    match crate::bootstrap_account::verify_login(&state.pool, &form.username, &form.password).await
    {
        Ok(LoginOutcome::Ok) => {}
        Ok(LoginOutcome::BadCredentials) => {
            return render_login(&state.pool, lang, lang.t("err.bad_credentials").to_string())
                .await;
        }
        Ok(LoginOutcome::Locked(minutes)) => {
            return render_login(&state.pool, lang, lang.t1("err.account_locked", minutes)).await;
        }
        Err(e) => {
            eprintln!("web: giriş kontrolü başarısız: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // Yerel hesabın oturumu da operatör oturumudur (ADR-095 madde 5); yetkisi
    // yalnızca Sistem yöneticisi — İK/denetçi/PII/helpdesk verilmez (madde 3).
    let operator = Operator {
        subject: LOCAL_SUBJECT.to_string(),
        username: crate::bootstrap_account::BOOTSTRAP_USERNAME.to_string(),
        email: String::new(),
        authorities: vec![crate::oidc::ADMIN_AUTHORITY.to_string()],
        auth_source: AuthSource::Local,
        lang,
    };
    establish_operator_session(state, operator).await
}

// --- START FEATURE: ad-login ---
// Asıl kapı (ADR-095 madde 1): doğrulama AD'de, yetki AD gruplarında, eşleşme
// `sAMAccountName` ile (madde 2 ve 6). Ayrılmış/askıdaki operatörün reddi ve
// oturum satırı üç kapıda ortak (`establish_operator_session`, madde 5).
async fn ad_login(state: &AppState, lang: Lang, form: &LoginForm) -> Response {
    use crate::ad_auth::AuthError;
    match crate::ad_auth::authenticate(&state.pool, &state.aead_key, &form.username, &form.password)
        .await
    {
        Ok(found) => {
            let operator = Operator {
                subject: format!("{AD_SUBJECT_PREFIX}{}", found.guid),
                username: found.username,
                email: found.email,
                authorities: found.authorities,
                auth_source: AuthSource::Ad,
                lang,
            };
            establish_operator_session(state, operator).await
        }
        Err(AuthError::BadCredentials | AuthError::NotConfigured) => {
            render_login(&state.pool, lang, lang.t("err.bad_credentials").to_string()).await
        }
        // Giriş sessizce başarısız olmaz: ekranda AD'ye ulaşılamadığı yazar,
        // ayrıntı yalnızca sunucu log'una gider, yerel kapı çalışmaya devam eder.
        Err(unavailable) => {
            eprintln!("web: AD girişi yapılamadı: {unavailable}");
            render_login(&state.pool, lang, lang.t("err.ad_unreachable").to_string()).await
        }
    }
}
// --- END FEATURE: ad-login ---

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let mut response_headers = HeaderMap::new();
    if let Some(token) = get_cookie(&headers, OPERATOR_SESSION_COOKIE_NAME) {
        let _ = crate::operator_session::delete_session(&state.pool, &token).await;
        response_headers.append(
            header::SET_COOKIE,
            cookie_header_value(&clear_cookie_header(OPERATOR_SESSION_COOKIE_NAME)),
        );
    }
    (response_headers, Redirect::to("/login")).into_response()
}

// --- START FEATURE: oidc-login ---
async fn oidc_login(State(state): State<AppState>) -> Response {
    let redirect_uri = format!("{}/oidc/callback", state.public_url);
    match crate::oidc::login_redirect(&state.pool, &state.aead_key, &redirect_uri).await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => {
            eprintln!("web: oidc girişi başlatılamadı: {e}");
            Redirect::to("/login").into_response()
        }
    }
}

#[derive(Deserialize)]
struct OidcCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn oidc_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OidcCallbackQuery>,
) -> Response {
    let (Some(code), Some(oidc_state)) = (query.code, query.state) else {
        eprintln!(
            "web: oidc geri dönüşünde eksik parametre (error={:?})",
            query.error
        );
        return Redirect::to("/login").into_response();
    };

    let redirect_uri = format!("{}/oidc/callback", state.public_url);
    match crate::oidc::handle_callback(
        &state.pool,
        &state.aead_key,
        &redirect_uri,
        code,
        oidc_state,
    )
    .await
    {
        Ok(result) => establish_oidc_session(&state, Lang::from_headers(&headers), result).await,
        Err(e) => {
            eprintln!("web: oidc girişi başarısız: {e}");
            Redirect::to("/login").into_response()
        }
    }
}

// id_token dogrulandiktan sonraki adim: ilk OpenSicil-Admins girisi isaretlenir
// (denetim bilgisi; ADR-095 madde 3 bunun form gizleme etkisini kaldirdi).
async fn establish_oidc_session(
    state: &AppState,
    lang: Lang,
    result: crate::oidc::LoginResult,
) -> Response {
    if result
        .authorities
        .iter()
        .any(|a| a == crate::oidc::ADMIN_AUTHORITY)
    {
        if let Err(e) = crate::settings::mark_oidc_admin_verified(&state.pool).await {
            eprintln!("web: oidc admin doğrulaması işaretlenemedi: {e}");
        }
    }

    let operator = Operator {
        subject: result.subject,
        username: result.username,
        email: result.email,
        authorities: result.authorities,
        auth_source: AuthSource::Oidc,
        lang,
    };
    establish_operator_session(state, operator).await
}

// Uc kapinin ortak son adimi (ADR-095 madde 5): ayrilmis operator reddi,
// oturum satiri, denetim kaydi, cerez ve giris sonrasi sayfa.
async fn establish_operator_session(state: &AppState, operator: Operator) -> Response {
    let lang = operator.lang;
    // ADR-059 madde 1: ayrilmis/askidaki operator oturum acamaz
    match crate::operator_guard::check_operator(&state.pool, &state.time_zone, &operator.username)
        .await
    {
        Ok(crate::operator_guard::Verdict::Allowed) => {}
        Ok(crate::operator_guard::Verdict::Rejected(reason)) => {
            return crate::operator_guard::rejection_response(state, &operator, reason).await;
        }
        Err(e) => {
            eprintln!("web: operatör kimlik durumu okunamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    let token = match crate::operator_session::create_session(&state.pool, &operator).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("web: operatör oturumu oluşturulamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    audit_operator_login(state, &operator).await;
    // Yerel hesap ilk girişte parolasını değiştirmeden başka ekrana gitmez; ara
    // katman da aynı kuralı her istekte uygular (operator_guard, ADR-095 madde 3).
    if operator.auth_source == AuthSource::Local
        && crate::bootstrap_account::must_change_password(&state.pool)
            .await
            .unwrap_or(false)
    {
        return with_operator_cookie(&token, Redirect::to("/change-password"));
    }
    operator_home_with_cookie(state, &token, lang, operator.username, operator.authorities).await
}

async fn audit_operator_login(state: &AppState, operator: &Operator) {
    let actor = crate::audit::Actor {
        subject: Some(&operator.subject),
        username: &operator.username,
    };
    let detail = serde_json::json!({
        "authorities": operator.authorities,
        "source": operator.auth_source.as_str(),
    });
    if let Err(e) = crate::audit::record(
        &state.pool,
        &actor,
        crate::audit::OPERATOR_LOGIN,
        None,
        detail,
    )
    .await
    {
        eprintln!("web: denetim kaydı yazılamadı (operator.login): {e}");
    }
}

async fn operator_home_with_cookie(
    state: &AppState,
    token: &str,
    lang: Lang,
    username: String,
    authorities: Vec<String>,
) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        cookie_header_value(&set_cookie_header(
            OPERATOR_SESSION_COOKIE_NAME,
            token,
            crate::operator_session::SESSION_LIFETIME_HOURS * 3600,
        )),
    );
    (
        headers,
        render_operator_home(state, lang, username, authorities, HomeQuery::default()).await,
    )
        .into_response()
}
// --- END FEATURE: oidc-login ---

async fn change_password_form(OperatorSession(operator): OperatorSession) -> Response {
    if !allowed(&operator, CONFIG_AUTHORITIES) {
        return forbidden(operator.lang);
    }
    render(&ChangePasswordTemplate {
        lang: operator.lang,
        error: String::new(),
    })
}

#[derive(Deserialize)]
struct ChangePasswordForm {
    new_password: String,
    confirm_password: String,
}

async fn change_password_submit(
    OperatorSession(operator): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<ChangePasswordForm>,
) -> Response {
    if !allowed(&operator, CONFIG_AUTHORITIES) {
        return forbidden(operator.lang);
    }
    let lang = operator.lang;
    if form.new_password != form.confirm_password {
        return render(&ChangePasswordTemplate {
            lang,
            error: lang.t("err.password_mismatch").to_string(),
        });
    }
    if form.new_password.chars().count() < MIN_PASSWORD_LENGTH {
        return render(&ChangePasswordTemplate {
            lang,
            error: lang.t1("err.password_too_short", MIN_PASSWORD_LENGTH),
        });
    }
    if let Err(e) = crate::bootstrap_account::set_password(&state.pool, &form.new_password).await {
        eprintln!("web: parola değiştirilemedi: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    // Denetim satiri yazilamazsa islem geri alinmaz (parola zaten degisti), yalnizca
    // log'a duser; docs/07 sirasi "once yaz" yalnizca worker'in hedef yazmalari icin.
    crate::identity_web::audit_operator(
        &state,
        &operator,
        crate::audit::BOOTSTRAP_PASSWORD_CHANGED,
        None,
        serde_json::json!({}),
    )
    .await;
    Redirect::to("/config").into_response()
}

async fn config_form(
    OperatorSession(operator): OperatorSession,
    State(state): State<AppState>,
) -> Response {
    if !allowed(&operator, CONFIG_AUTHORITIES) {
        return forbidden(operator.lang);
    }
    match crate::settings::load(&state.pool).await {
        Ok(s) => render(&ConfigTemplate {
            lang: operator.lang,
            shell: Shell::of(&operator),
            ad_host: s.ad_host,
            ad_bind_dn: s.ad_bind_dn,
            ad_service_password_set: s.ad_service_password_set,
            ad_national_id_attribute: s.ad_national_id_attribute,
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
    /// ADR-106 madde 5; bos = TC kimlik no okunmaz (varsayilan)
    #[serde(default)]
    ad_national_id_attribute: String,
    zimbra_url: String,
    zimbra_admin_password: String,
    oidc_issuer: String,
    oidc_client_id: String,
    oidc_client_secret: String,
}

impl From<ConfigForm> for crate::settings::AppSettingsInput {
    fn from(form: ConfigForm) -> Self {
        Self {
            ad_host: form.ad_host,
            ad_bind_dn: form.ad_bind_dn,
            ad_service_password: form.ad_service_password,
            ad_national_id_attribute: form.ad_national_id_attribute,
            zimbra_url: form.zimbra_url,
            zimbra_admin_password: form.zimbra_admin_password,
            oidc_issuer: form.oidc_issuer,
            oidc_client_id: form.oidc_client_id,
            oidc_client_secret: form.oidc_client_secret,
        }
    }
}

/// LDAP oznitelik adi (AttributeDescription): harfle baslar, harf/rakam/tire.
/// Bos = okunmaz (varsayilan). Bozuk ad taramayi LDAP hatasiyla dusururdu;
/// burada reddedilir.
fn valid_attribute_name(name: &str) -> bool {
    name.is_empty()
        || (name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Denetim satirina yalnizca hangi sirrin guncellendigi girer, degeri degil.
fn updated_secret_names(form: &ConfigForm) -> Vec<&'static str> {
    [
        ("ad_service_password", &form.ad_service_password),
        ("zimbra_admin_password", &form.zimbra_admin_password),
        ("oidc_client_secret", &form.oidc_client_secret),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(name, _)| name)
    .collect()
}

async fn config_submit(
    OperatorSession(operator): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<ConfigForm>,
) -> Response {
    if !allowed(&operator, CONFIG_AUTHORITIES) {
        return forbidden(operator.lang);
    }
    if !valid_attribute_name(form.ad_national_id_attribute.trim()) {
        let text = operator.lang.t("err.ldap_attribute_shape").to_string();
        return (StatusCode::BAD_REQUEST, text).into_response();
    }
    let secrets_updated = updated_secret_names(&form);
    let before = match crate::settings::load(&state.pool).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("web: ayarlar okunamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let input = crate::settings::AppSettingsInput::from(form);
    if let Err(e) = crate::settings::save(&state.pool, &state.aead_key, &input).await {
        eprintln!("web: ayarlar kaydedilemedi: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Ok(after) = crate::settings::load(&state.pool).await {
        let detail = crate::audit::settings_change_detail(&before, &after, &secrets_updated);
        crate::identity_web::audit_operator(
            &state,
            &operator,
            crate::audit::SETTINGS_CHANGED,
            None,
            detail,
        )
        .await;
    }
    Redirect::to("/config").into_response()
}

// --- END FEATURE: bootstrap-admin ---

// --- START FEATURE: ui-i18n ---
#[derive(Deserialize)]
struct LangForm {
    lang: String,
}

// Secici tercihi oturum satirina yazar ve gelinen sayfaya geri doner (ADR-089).
async fn set_lang(
    OperatorSession(_op): OperatorSession,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LangForm>,
) -> Response {
    let Some(token) = get_cookie(&headers, OPERATOR_SESSION_COOKIE_NAME) else {
        return Redirect::to("/login").into_response();
    };
    if let Err(e) =
        crate::operator_session::set_lang(&state.pool, &token, Lang::from_code(&form.lang)).await
    {
        eprintln!("web: dil tercihi yazılamadı: {e}");
    }
    let back = headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(local_path)
        .unwrap_or("/");
    Redirect::to(back).into_response()
}

// Referer yalnizca kendi sayfamiza donmek icin kullanilir: baska bir host'a ya da
// protokole gidebilecek deger reddedilir (acik yonlendirme, .claude/rules/security.md).
fn local_path(referer: &str) -> Option<&str> {
    let path = match referer.find("://") {
        Some(at) => {
            let host = &referer[at + 3..];
            host.find('/').map(|slash| &host[slash..])?
        }
        None => referer,
    };
    // `//host` ve `/\host` protokol-goreli adrestir: tarayici ters bolu isaretini
    // bolu gibi okur, acik yonlendirme olur
    let relative = path.starts_with("//") || path.starts_with("/\\");
    (path.starts_with('/') && !relative).then_some(path)
}
// --- END FEATURE: ui-i18n ---

fn cookie_header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).expect("cerez basligi gecersiz karakter icermez")
}

fn with_operator_cookie(token: &str, redirect: Redirect) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        cookie_header_value(&set_cookie_header(
            OPERATOR_SESSION_COOKIE_NAME,
            token,
            crate::operator_session::SESSION_LIFETIME_HOURS * 3600,
        )),
    );
    (headers, redirect).into_response()
}

#[cfg(test)]
pub(crate) fn test_state(pool: PgPool, public_url: &str) -> AppState {
    AppState {
        pool,
        aead_key: [3u8; crate::crypto::KEY_LEN],
        blind_index_key: [4u8; crate::crypto::KEY_LEN],
        public_url: public_url.to_string(),
        time_zone: "Europe/Istanbul".to_string(),
        change_set_threshold: crate::change_set::DEFAULT_THRESHOLD,
        approval_timelock_hours: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    // Ayri bir Cargo.toml girdisi gerekmez: openidconnect zaten reqwest'i disa aciyor.
    use openidconnect::reqwest;
    use tower::ServiceExt;

    fn test_app(pool: PgPool) -> Router {
        test_app_with_public_url(pool, "https://localhost")
    }

    fn test_app_with_public_url(pool: PgPool, public_url: &str) -> Router {
        routes().with_state(test_state(pool, public_url))
    }

    // --- START FEATURE: ui-i18n ---
    // ADR-089 kabul kriteri: secici EN'e gecirince ekran Ingilizce, tercih oturumda kalir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn language_switch_is_stored_in_the_session_and_changes_the_screen() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let operator = crate::operator_session::Operator {
            subject: "sub-dil".to_string(),
            username: "dil.operatoru".to_string(),
            email: "dil@example.org".to_string(),
            authorities: vec!["hr".to_string()],
            auth_source: AuthSource::Oidc,
            lang: Lang::Tr,
        };
        let token = crate::operator_session::create_session(&pool, &operator)
            .await
            .unwrap();
        let cookie = format!("{OPERATOR_SESSION_COOKIE_NAME}={token}");
        let home = |app: Router, cookie: String| async move {
            let request = Request::builder()
                .uri("/login")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            String::from_utf8(
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap()
        };

        let body = home(test_app(pool.clone()), cookie.clone()).await;
        assert!(
            body.contains("Ana Sayfa") && body.contains(">EN<"),
            "{body}"
        );

        let request = Request::builder()
            .method("POST")
            .uri("/lang")
            .header("cookie", cookie.clone())
            .header("referer", "/upcoming?days=7")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("lang=en"))
            .unwrap();
        let response = test_app(pool.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get("location").unwrap(),
            "/upcoming?days=7",
            "gelinen sayfaya döner"
        );
        let stored: String =
            sqlx::query_scalar("SELECT lang FROM operator_sessions WHERE username = $1")
                .bind(&operator.username)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, "en", "tercih oturum satırında");

        let body = home(test_app(pool.clone()), cookie).await;
        // Kenar cubugu etiketleri (ADR-096): "Yaklasan bitisler" /reports altina
        // tasindi, menude artik Personel ve Raporlar var
        assert!(
            body.contains("Home") && body.contains("Personnel"),
            "{body}"
        );
        assert!(!body.contains("Personel"), "{body}");
        assert!(
            body.contains("<html lang=\"en\">") && body.contains(">TR<"),
            "{body}"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // Kabuktaki her sayfa "/" baglantisini tasir (marka + "Kimlikler"); rota yoksa
    // gezinmenin ilk linki 404 verir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn root_path_serves_the_operator_home_or_the_login_form() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let operator = crate::operator_session::Operator {
            subject: "sub-kok".to_string(),
            username: "kok.operatoru".to_string(),
            email: "kok@example.org".to_string(),
            authorities: vec!["hr".to_string()],
            auth_source: AuthSource::Oidc,
            lang: Lang::Tr,
        };
        let token = crate::operator_session::create_session(&pool, &operator)
            .await
            .unwrap();

        let get_root = |app: Router, cookie: Option<String>| async move {
            let mut builder = Request::builder().uri("/");
            if let Some(cookie) = cookie {
                builder = builder.header("cookie", cookie);
            }
            let response = app
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let body = String::from_utf8(
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            (status, body)
        };

        let (status, body) = get_root(
            test_app(pool.clone()),
            Some(format!("{OPERATOR_SESSION_COOKIE_NAME}={token}")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("kok.operatoru"), "{body}");

        let (status, body) = get_root(test_app(pool.clone()), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("name=\"password\""), "{body}");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[test]
    fn referer_only_sends_the_operator_back_to_a_local_path() {
        assert_eq!(local_path("/roles"), Some("/roles"));
        assert_eq!(
            local_path("https://opensicil.example/identities/3"),
            Some("/identities/3")
        );
        assert_eq!(local_path("//evil.example/x"), None);
        assert_eq!(local_path("/\\evil.example/x"), None);
        assert_eq!(local_path("https://evil.example"), None);
        assert_eq!(local_path("javascript:alert(1)"), None);
    }
    // --- END FEATURE: ui-i18n ---

    // ADR-059 madde 1: oturum acilisinda da kontrol; ayrilmis operatore oturum acilmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn departed_operator_cannot_establish_session() {
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

        let state = test_state(pool.clone(), "https://localhost");
        let result = crate::oidc::LoginResult {
            subject: "sub-ayse".to_string(),
            username: "ayse.yilmaz".to_string(),
            email: "ayse@example.com".to_string(),
            authorities: vec!["hr".to_string()],
        };
        let response = establish_oidc_session(&state, Lang::En, result).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operator_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0, "oturum açılmamalı");
        let rejected: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE event_type = $1")
                .bind(crate::audit::OPERATOR_REJECTED)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rejected, 1, "red denetim kaydına girmeli");

        drop(state);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
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
        // Gercek router: "parolayi degistirmeden baska ekran yok" kurali ara
        // katmanda (operator_guard), rotalarin kendisinde degil (ADR-095 madde 3).
        let app = crate::server::build_router(test_state(pool.clone(), "https://localhost"));

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
                     ad_national_id_attribute=extensionAttribute5&\
                     zimbra_url=https%3A%2F%2Fzimbra.example.org&zimbra_admin_password=cok-gizli-zimbra&\
                     oidc_issuer=https%3A%2F%2Fidp.example.org&oidc_client_id=opensicil&oidc_client_secret=cok-gizli-oidc";
        let response = app
            .clone()
            .oneshot(form_request("POST", "/config", form, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(location_of(&response), "/config");

        // ADR-106 madde 5: bozuk oznitelik adi kaydedilmez (LDAP filtreye girer)
        let bad = form.replace("extensionAttribute5", "ext%20attr;5");
        let response = app
            .clone()
            .oneshot(form_request("POST", "/config", &bad, Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Yeniden yuklenince degerler gorunur ama sirlar duz metin geri gelmez.
        let response = app
            .clone()
            .oneshot(get_request("/config", Some(&cookie)))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(body.contains("dc1.example.org"));
        assert!(body.contains(r#"value="extensionAttribute5""#));
        assert!(body.contains("kayıtlı"));
        assert!(!body.contains("cok-gizli-ad"));
        assert!(!body.contains("cok-gizli-zimbra"));
        assert!(!body.contains("cok-gizli-oidc"));

        // ADR-095 madde 3/5: yerel hesap gercek bir operator oturumudur, yetkisi
        // yalnizca Sistem yoneticisi ve oturum satiri kapiyi tasir.
        let session: (Vec<String>, String, String) = sqlx::query_as(
            "SELECT authorities, auth_source, subject FROM operator_sessions LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(session.0, vec![crate::oidc::ADMIN_AUTHORITY.to_string()]);
        assert_eq!(session.1, "local");
        assert_eq!(session.2, LOCAL_SUBJECT);
        let login_source: String = sqlx::query_scalar(
            "SELECT detail->>'source' FROM audit_log WHERE event_type = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(crate::audit::OPERATOR_LOGIN)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(login_source, "local", "break-glass kullanımı denetimde");

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

    // --- START FEATURE: oidc-login ---
    // Gercek lab Keycloak'ina karsi uctan uca: /oidc/login'den donen yonlendirme
    // URL'sine gercekten gidip Keycloak'in kendi giris formunu dolduruyor, donen
    // code+state'i kendi /oidc/callback'imize veriyoruz (ADR-027 deseni: ortam
    // degiskeni yoksa bu test calismaz, --ignore varsayilani).
    //
    // `docker compose -f compose.lab.yaml up -d` ile lab Keycloak'i ayakta olmali:
    //   DATABASE_URL=postgres://testuser:testpass@localhost:15432/testdb \
    //   OIDC_LAB_ISSUER=http://localhost:8081/realms/opensicil \
    //   cargo test --include-ignored oidc_login_flow_against_lab_keycloak

    fn cookie_header_from_set_cookies(response: &reqwest::Response) -> String {
        response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|raw| raw.split(';').next())
            .collect::<Vec<_>>()
            .join("; ")
    }

    // Keycloak'in giris sayfasindaki ana formun action URL'sini (session_code,
    // execution, tab_id tasiyan) ham HTML'den cikarir; bagimlilik eklemeden.
    fn extract_login_form_action(html: &str) -> String {
        let form_start = html
            .find(r#"id="kc-form-login""#)
            .expect("kc-form-login formu bulunamadı");
        let action_key = "action=\"";
        let action_start = html[form_start..]
            .find(action_key)
            .map(|i| form_start + i + action_key.len())
            .expect("form action bulunamadı");
        let action_end = html[action_start..]
            .find('"')
            .map(|i| action_start + i)
            .expect("form action kapanışı bulunamadı");
        html[action_start..action_end].replace("&amp;", "&")
    }

    fn query_param(url: &str, name: &str) -> Option<String> {
        let (_, query) = url.split_once('?')?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| percent_decode(value))
        })
    }

    // OIDC query parametreleri icin yeterli, tam bir URL kutuphanesi degil.
    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(byte) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres ve lab Keycloak gerektirir: DATABASE_URL + OIDC_LAB_ISSUER ile çalıştır (--include-ignored)"]
    async fn oidc_login_flow_against_lab_keycloak() {
        let issuer = std::env::var("OIDC_LAB_ISSUER")
            .expect("OIDC_LAB_ISSUER lab Keycloak'a işaret etmeli (bkz. yukarıdaki yorum)");

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        // test_app_with_public_url'in kurdugu AppState.aead_key ile ayni olmali.
        let key = [3u8; crate::crypto::KEY_LEN];
        crate::settings::save(
            &pool,
            &key,
            &crate::settings::AppSettingsInput {
                ad_host: String::new(),
                ad_bind_dn: String::new(),
                ad_service_password: String::new(),
                ad_national_id_attribute: String::new(),
                zimbra_url: String::new(),
                zimbra_admin_password: String::new(),
                oidc_issuer: issuer,
                oidc_client_id: "opensicil-backend".to_string(),
                oidc_client_secret: "lab-only-not-secret".to_string(),
            },
        )
        .await
        .expect("oidc ayarları kaydedilemedi");

        let app = test_app_with_public_url(pool.clone(), "http://localhost:8000");

        // 1) /oidc/login bizim router'imizdan Keycloak'in authorize URL'sine yonlendirir.
        let response = app
            .clone()
            .oneshot(get_request("/oidc/login", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let authorize_url = location_of(&response).to_string();

        // 2) Gercek Keycloak'a git, giris formunu al.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let login_page = http.get(&authorize_url).send().await.unwrap();
        assert_eq!(login_page.status(), reqwest::StatusCode::OK);
        let kc_cookies = cookie_header_from_set_cookies(&login_page);
        let login_html = login_page.text().await.unwrap();
        let form_action = extract_login_form_action(&login_html);

        // 3) test-admin/test-admin-pw ile giris yap (keycloak-lab/realm-opensicil.json).
        let login_response = http
            .post(&form_action)
            .header(reqwest::header::COOKIE, &kc_cookies)
            .form(&[("username", "test-admin"), ("password", "test-admin-pw")])
            .send()
            .await
            .unwrap();
        assert_eq!(
            login_response.status(),
            reqwest::StatusCode::FOUND,
            "Keycloak girişi başarısız görünüyor (beklenen 302 yönlendirme)"
        );
        let redirect_to = login_response
            .headers()
            .get(reqwest::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(redirect_to.starts_with("http://localhost:8000/oidc/callback"));
        let code = query_param(&redirect_to, "code").expect("code parametresi yok");
        let state = query_param(&redirect_to, "state").expect("state parametresi yok");

        // 4) code+state'i kendi /oidc/callback'imize ver.
        let callback_uri = format!("/oidc/callback?code={code}&state={state}");
        let response = app
            .clone()
            .oneshot(get_request(&callback_uri, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let operator_cookie = set_cookie_value(&response);
        assert!(operator_cookie.starts_with("opensicil_operator_session="));
        let body = body_string(response).await;
        assert!(body.contains("test-admin"));
        assert!(body.contains("admin"));

        // 5) test-admin OpenSicil-Admins'te oldugu icin admin dogrulama isareti
        // kuruldu (denetim bilgisi), ama ADR-095: yerel form artik gizlenmiyor.
        let settings = crate::settings::load(&pool).await.unwrap();
        assert!(settings.oidc_admin_verified);

        let response = app
            .clone()
            .oneshot(get_request("/login", None))
            .await
            .unwrap();
        let body = body_string(response).await;
        assert!(
            body.contains(r#"name="username""#),
            "kendi giriş formumuz hiçbir koşulda gizlenmez (ADR-095)"
        );
        assert!(body.contains("/oidc/login"));

        // 6) operatör oturumu cerezle geri geliyor.
        let response = app
            .clone()
            .oneshot(get_request("/login", Some(&operator_cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("test-admin"));

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
    // --- END FEATURE: oidc-login ---

    // --- START FEATURE: ad-login ---
    // Kapiyi kullanici adi secer: AD adiyla yapilan deneme yerel hesabin kilit
    // sayacina dokunmaz. Dokunsaydi (yanlis kullanici adi da deneme sayilir)
    // her AD girisi break-glass hesabini kilide yaklastirirdi.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn an_ad_username_never_touches_the_local_lockout_counter() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::migrate::seed_bootstrap_account(&pool)
            .await
            .expect("bootstrap hesabı seed edilemedi");
        let app = crate::server::build_router(test_state(pool.clone(), "https://localhost"));

        // AD yapilandirilmamis: kapi yok, ekranda ayrimsiz hata
        for _ in 0..=crate::bootstrap_account::MAX_FAILED_ATTEMPTS {
            let response = app
                .clone()
                .oneshot(form_request(
                    "POST",
                    "/login",
                    "username=ayse.yilmaz&password=bir-parola",
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().get(header::SET_COOKIE).is_none());
        }
        let failed: i32 =
            sqlx::query_scalar("SELECT failed_attempts FROM bootstrap_account WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(failed, 0, "yerel sayaç AD denemelerinden etkilenmedi");

        // Yerel kapi hala calisiyor (kilitlenmedi)
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

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // Lab Samba'ya karsi gercek bind (ADR-095 madde 1/2/6 kabul kriterleri):
    // dogru parola girer, yanlis parola reddedilir, yonetim grubunda olmayan
    // kullanici girer ama hicbir ekrani goremez, ayrilmis operator reddedilir.
    //
    // `docker compose -f compose.yaml -f compose.lab.yaml up -d samba-ad` +
    // `sh samba-lab/gen-tls.sh` + `sh samba-lab/seed.sh` sonrasi:
    //   DATABASE_URL=postgres://testuser:testpass@localhost:15432/testdb \
    //   AD_LAB_URL=ldaps://localhost:6360 AD_LAB_BIND_DN=... AD_LAB_PASSWORD=... \
    //   AD_CA_FILE=samba-lab/tls/ca.pem \
    //   cargo test --include-ignored ad_login_flow_against_lab_samba
    #[tokio::test]
    #[ignore = "gerçek Postgres ve lab Samba AD gerektirir: DATABASE_URL + AD_LAB_URL/AD_LAB_BIND_DN/AD_LAB_PASSWORD + AD_CA_FILE ile çalıştır (--include-ignored)"]
    async fn ad_login_flow_against_lab_samba() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        std::env::var("AD_CA_FILE").expect("AD_CA_FILE lab CA'sına işaret etmeli");
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        // test_state'in kurdugu AppState.aead_key ile ayni olmali
        crate::settings::save(
            &pool,
            &[3u8; crate::crypto::KEY_LEN],
            &crate::settings::AppSettingsInput {
                ad_host: var("AD_LAB_URL"),
                ad_bind_dn: var("AD_LAB_BIND_DN"),
                ad_service_password: var("AD_LAB_PASSWORD"),
                ad_national_id_attribute: String::new(),
                zimbra_url: String::new(),
                zimbra_admin_password: String::new(),
                oidc_issuer: String::new(),
                oidc_client_id: String::new(),
                oidc_client_secret: String::new(),
            },
        )
        .await
        .expect("AD ayarları kaydedilemedi");
        let app = crate::server::build_router(test_state(pool.clone(), "https://localhost"));
        let login = |app: Router, body: String| async move {
            app.oneshot(form_request("POST", "/login", &body, None))
                .await
                .unwrap()
        };

        // 1) Yanlis parola: hata gosterilir, oturum acilmaz.
        let response = login(
            app.clone(),
            "username=lab.operator&password=yanlis-parola".to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert!(body_string(response).await.contains("parola yanlış"));

        // 2) Dogru parola: oturum acilir, yetki ic ice grup uyeliginden gelir
        // (lab.operator -> GG-Lab-Operators -> OpenSicil-Admins).
        let response = login(
            app.clone(),
            "username=lab.operator&password=Lab-only-Pass1".to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = set_cookie_value(&response);
        let session: (Vec<String>, String, String) = sqlx::query_as(
            "SELECT authorities, auth_source, subject FROM operator_sessions \
             WHERE username = 'lab.operator'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(session.0, vec![crate::oidc::ADMIN_AUTHORITY.to_string()]);
        assert_eq!(session.1, "ad", "oturum kapıyı taşır (ADR-095 madde 5)");
        assert!(
            session.2.starts_with(AD_SUBJECT_PREFIX) && session.2.contains('-'),
            "aktör objectGUID ile yazılır: {}",
            session.2
        );
        let login_source: String = sqlx::query_scalar(
            "SELECT detail->>'source' FROM audit_log WHERE event_type = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(crate::audit::OPERATOR_LOGIN)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(login_source, "ad");
        let response = app
            .clone()
            .oneshot(get_request("/identities/new", Some(&cookie)))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Sistem yöneticisi kayıt ekranını açar"
        );

        // 3) Yonetim grubunda olmayan AD kullanicisi girer ama hicbir ekrani goremez.
        let response = login(
            app.clone(),
            "username=mevcut.personel&password=Lab-only-Pass1".to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let plain_cookie = set_cookie_value(&response);
        let authorities: Vec<String> = sqlx::query_scalar(
            "SELECT authorities FROM operator_sessions WHERE username = 'mevcut.personel'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(authorities.is_empty(), "yetki yok: {authorities:?}");
        let response = app
            .clone()
            .oneshot(get_request("/identities/new", Some(&plain_cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // 4) ADR-095 madde 5: ayrilmis operator bu kapida da reddedilir.
        let ids = crate::test_support::seed_two_identities(&pool).await;
        sqlx::query(
            "UPDATE identities SET username = 'mevcut.personel', \
             end_at = now() - interval '1 hour' WHERE id = $1",
        )
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
        let response = login(
            app.clone(),
            "username=mevcut.personel&password=Lab-only-Pass1".to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
    // --- END FEATURE: ad-login ---
}
