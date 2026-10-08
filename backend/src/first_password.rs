// --- START FEATURE: first-password ---
// Ilk parola teslimi, backend tarafi (ADR-009/019/036/046/085): istek satiri + is;
// durum sayfasi worker'i bekler; parola bir kez cozulur, gosterilir, bosaltilir.
// Yetki: hr, helpdesk, admin (ADR-019 altinci yetki).

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use sqlx::PgPool;

use crate::audit::{FIRST_PASSWORD_REQUESTED, FIRST_PASSWORD_SHOWN};
use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::operator_session::Operator;
use crate::shell::Shell;
use crate::web::{render, AppState};

pub const AUTHORITIES: &[&str] = &["hr", "helpdesk", "admin"];

pub async fn request(
    pool: &PgPool,
    identity_id: i64,
    target_system_id: i64,
    requested_by: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO first_passwords (identity_id, target_system_id, requested_by) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(identity_id)
    .bind(target_system_id)
    .bind(requested_by)
    .fetch_one(pool)
    .await
}

pub enum Status {
    Pending,
    /// Parola cozuldu ve ayni anda bosaltildi: yalnizca bu yanitta var
    Ready(String),
    AlreadyShown,
    Rejected(String),
}

/// (eski password_enc, issued_at epoch, error)
type TakeRow = (Option<Vec<u8>>, Option<i64>, Option<String>);

// Tek sorguda "al ve bosalt" (Postgres 18 `RETURNING old`): iki operator ayni anda
// yenilese bile satir kilidi sayesinde parola bir kez cikar.
pub async fn take(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
    identity_id: i64,
) -> Result<Option<Status>, sqlx::Error> {
    let row: Option<TakeRow> = sqlx::query_as(
        "UPDATE first_passwords SET password_enc = NULL, \
           shown_at = CASE WHEN password_enc IS NOT NULL THEN now() ELSE shown_at END \
         WHERE id = $1 AND identity_id = $2 \
         RETURNING old.password_enc, EXTRACT(EPOCH FROM issued_at)::bigint, error",
    )
    .bind(id)
    .bind(identity_id)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(enc, issued_at, error)| match (enc, issued_at, error) {
            (_, _, Some(reason)) => Status::Rejected(reason),
            (Some(enc), _, None) => match crate::crypto::decrypt_versioned(aead_key, &enc) {
                Ok(bytes) => Status::Ready(String::from_utf8_lossy(&bytes).into_owned()),
                Err(e) => Status::Rejected(format!("parola çözülemedi: {e}")),
            },
            (None, Some(_), None) => Status::AlreadyShown,
            (None, None, None) => Status::Pending,
        }),
    )
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/identities/{id}/first-password", post(create))
        .route("/identities/{id}/first-password/{fp}", get(show))
}

async fn create(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, AUTHORITIES) {
        return forbidden(op.lang);
    }
    // v1: parola AD'nindir (ADR-009); Zimbra girişi AD parolasıyla
    let ad: Option<i64> =
        match sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_optional(&state.pool)
            .await
        {
            Ok(t) => t,
            Err(e) => return internal("hedef sistem okunamadı", e),
        };
    let Some(ad) = ad else {
        return (StatusCode::NOT_FOUND, op.lang.t("err.ad_target_missing")).into_response();
    };
    let fp = match request(&state.pool, id, ad, &op.username).await {
        Ok(fp) => fp,
        Err(e) => return internal("ilk parola isteği yazılamadı", e),
    };
    let detail = serde_json::json!({ "first_password_id": fp, "target_id": ad });
    audit_operator(&state, &op, FIRST_PASSWORD_REQUESTED, Some(id), detail).await;
    if let Err(e) = crate::jobs::enqueue(&state.pool, id, ad, crate::jobs::Priority::Single).await {
        log_error!("web: ilk parola işi açılamadı (kimlik {id}): {e}");
    }
    Redirect::to(&format!("/identities/{id}/first-password/{fp}")).into_response()
}

#[derive(Template)]
#[template(path = "first_password.html")]
struct FirstPasswordTemplate {
    shell: Shell,
    lang: Lang,
    identity_id: i64,
    name: String,
    username: String,
    email: String,
    pending: bool,
    /// ADR-056: beklerken hedef islerinin ilerlemesi (AD hesabi, mailbox)
    jobs: Vec<crate::identity::Job>,
    password: String,
    error: String,
}

impl FirstPasswordTemplate {
    /// `header`: (ad soyad, kullanici adi, e-posta) — `person_header` cikti sirasi
    fn new(op: &Operator, identity_id: i64, header: (String, String, String)) -> Self {
        Self {
            shell: Shell::of(op),
            lang: op.lang,
            identity_id,
            name: header.0,
            username: header.1,
            email: header.2,
            pending: false,
            jobs: Vec::new(),
            password: String::new(),
            error: String::new(),
        }
    }
}

async fn show(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((id, fp)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, AUTHORITIES) {
        return forbidden(op.lang);
    }
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let header = match person_header(&state.pool, id).await {
        Ok(Some(h)) => h,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.identity_not_found")).into_response()
        }
        Err(e) => return internal("kimlik okunamadı", e),
    };
    let status = match take(&state.pool, &state.aead_key, fp, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.request_not_found")).into_response()
        }
        Err(e) => return internal("ilk parola okunamadı", e),
    };
    let mut page = FirstPasswordTemplate::new(&op, id, header);
    match status {
        Status::Pending => {
            page.pending = true;
            page.jobs = match crate::identity::load_jobs(&state.pool, op.lang, &time_zone, id).await
            {
                Ok(jobs) => jobs,
                Err(e) => return internal("işler okunamadı", e),
            };
        }
        Status::Ready(password) => {
            page.password = password;
            let detail = serde_json::json!({ "first_password_id": fp });
            audit_operator(&state, &op, FIRST_PASSWORD_SHOWN, Some(id), detail).await;
        }
        Status::AlreadyShown => page.error = op.lang.t("err.first_password_shown").to_string(),
        Status::Rejected(reason) => page.error = reason,
    }
    render(&page)
}

/// (ad soyad, kullanici adi, e-posta); kullanici adi henuz uretilmemisse bos
async fn person_header(
    pool: &PgPool,
    id: i64,
) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let row: Option<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT given_name, surname, username, email FROM identities WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(given, surname, username, email)| {
        (
            format!("{given} {surname}"),
            username.unwrap_or_default(),
            email.unwrap_or_default(),
        )
    }))
}
// --- END FEATURE: first-password ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn helpdesk_requests_then_password_shows_exactly_once() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let cookie = |authorities: &'static [&'static str]| {
            let pool = pool.clone();
            async move {
                let operator = crate::operator_session::Operator {
                    subject: "sub".to_string(),
                    username: "yardim.masasi".to_string(),
                    email: "ym@example.org".to_string(),
                    authorities: authorities.iter().map(|a| a.to_string()).collect(),
                    auth_source: crate::operator_session::AuthSource::Oidc,
                    lang: crate::i18n::DEFAULT,
                };
                let token = crate::operator_session::create_session(&pool, &operator)
                    .await
                    .unwrap();
                format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME)
            }
        };
        let request_with = |method: &str, uri: &str, cookie: &str| {
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap()
        };
        let body_string = |r: Response| async move {
            String::from_utf8(
                axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap()
        };
        let helpdesk = cookie(&["helpdesk"]).await;
        let auditor = cookie(&["auditor"]).await;
        let id = ids[0];

        // Yardim masasi ister; denetci isteyemez; tekrar dene de yapamaz (ADR-019).
        let r = app
            .clone()
            .oneshot(request_with(
                "POST",
                &format!("/identities/{id}/first-password"),
                &auditor,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = app
            .clone()
            .oneshot(request_with(
                "POST",
                &format!("/identities/{id}/first-password"),
                &helpdesk,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let location = r.headers()["location"].to_str().unwrap().to_string();
        let fp: i64 = location.rsplit('/').next().unwrap().parse().unwrap();
        // ADR-019: yardim masasi kayit, ayrilis ve rol degistiremez
        for uri in [
            "/identities",
            "/identities/1/departure",
            "/identities/1/roles",
        ] {
            let r = app
                .clone()
                .oneshot(request_with("POST", uri, &helpdesk))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{uri}");
        }
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE identity_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 1, "AD hedefine iş açılır");
        let r = app
            .clone()
            .oneshot(request_with(
                "POST",
                &format!("/identities/{id}/jobs/1/retry"),
                &helpdesk,
            ))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::FORBIDDEN,
            "yardım masası başka şey yazamaz"
        );

        // Bekliyor → worker yazdi → bir kez gosterilir → ikinci acilista yok.
        let page = body_string(
            app.clone()
                .oneshot(request_with("GET", &location, &helpdesk))
                .await
                .unwrap(),
        )
        .await;
        assert!(page.contains("bekliyor"), "{page}");
        let key = [3u8; crate::crypto::KEY_LEN];
        let enc = crate::crypto::encrypt_versioned(&key, b"Kf7m-Rq2x-Wn8d-Tz4p");
        sqlx::query(
            "UPDATE first_passwords SET password_enc = $2, issued_at = now() WHERE id = $1",
        )
        .bind(fp)
        .bind(enc)
        .execute(&pool)
        .await
        .unwrap();
        let page = body_string(
            app.clone()
                .oneshot(request_with("GET", &location, &helpdesk))
                .await
                .unwrap(),
        )
        .await;
        assert!(page.contains("Kf7m-Rq2x-Wn8d-Tz4p"), "{page}");
        let page = body_string(
            app.clone()
                .oneshot(request_with("GET", &location, &helpdesk))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !page.contains("Kf7m-Rq2x") && page.contains("bir kez gösterildi"),
            "{page}"
        );
        let (enc_left, shown): (Option<Vec<u8>>, Option<i64>) = sqlx::query_as(
            "SELECT password_enc, EXTRACT(EPOCH FROM shown_at)::bigint FROM first_passwords WHERE id = $1",
        )
        .bind(fp)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(enc_left.is_none() && shown.is_some());
        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE event_type IN ($1, $2) AND identity_id = $3",
        )
        .bind(crate::audit::FIRST_PASSWORD_REQUESTED)
        .bind(crate::audit::FIRST_PASSWORD_SHOWN)
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 2);
        // Red nedeni operatore gosterilir.
        sqlx::query("UPDATE first_passwords SET error = 'hesap kullanılmış' WHERE id = $1")
            .bind(fp)
            .execute(&pool)
            .await
            .unwrap();
        let page = body_string(
            app.clone()
                .oneshot(request_with("GET", &location, &helpdesk))
                .await
                .unwrap(),
        )
        .await;
        assert!(page.contains("hesap kullanılmış"), "{page}");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
