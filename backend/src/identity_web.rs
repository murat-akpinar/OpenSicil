// --- START FEATURE: identity-registration ---
// Kimlik kayit formu, kisi sayfasi ve "tekrar dene" rotalari (F-12, ADR-078).
// Yetki: kayit hr/admin, tekrar dene hr/helpdesk/admin, sayfa her operator.

use askama::Template;
use axum::extract::{Form, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::cookie::{get_cookie, OPERATOR_SESSION_COOKIE_NAME};
use crate::identity::{self, FormOptions, IdentityForm, PersonPage};
use crate::operator_session::Operator;
use crate::web::{render, AppState};

const REGISTER_AUTHORITIES: &[&str] = &["hr", "admin"];
const RETRY_AUTHORITIES: &[&str] = &["hr", "helpdesk", "admin"];

pub struct OperatorSession(pub Operator);

pub struct NoOperatorSession;

impl IntoResponse for NoOperatorSession {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

impl FromRequestParts<AppState> for OperatorSession {
    type Rejection = NoOperatorSession;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token =
            get_cookie(&parts.headers, OPERATOR_SESSION_COOKIE_NAME).ok_or(NoOperatorSession)?;
        match crate::operator_session::validate_session(&state.pool, &token).await {
            Ok(Some(operator)) => Ok(OperatorSession(operator)),
            _ => Err(NoOperatorSession),
        }
    }
}

pub(crate) fn allowed(operator: &Operator, any_of: &[&str]) -> bool {
    operator
        .authorities
        .iter()
        .any(|a| any_of.contains(&a.as_str()))
}

pub(crate) fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "Bu işlem için yetkiniz yok.").into_response()
}

pub(crate) fn internal(what: &str, e: impl std::fmt::Display) -> Response {
    eprintln!("web: {what}: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/identities/new", get(new_form))
        .route("/identities", post(create))
        .route("/identities/{id}", get(show))
        .route("/identities/{id}/jobs/{job_id}/retry", post(retry))
}

#[derive(Template)]
#[template(path = "identity_form.html")]
struct IdentityFormTemplate {
    form: IdentityForm,
    options: FormOptions,
    employment_types: &'static [(&'static str, &'static str)],
    error: String,
    duplicate_warning: bool,
}

#[derive(Template)]
#[template(path = "identity.html")]
struct PersonTemplate {
    page: PersonPage,
    can_retry: bool,
}

async fn new_form(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let form = IdentityForm {
        national_id_country: "TR".to_string(),
        ..IdentityForm::default()
    };
    render_form(&state, form, String::new(), false).await
}

async fn render_form(
    state: &AppState,
    form: IdentityForm,
    error: String,
    duplicate_warning: bool,
) -> Response {
    match identity::form_options(&state.pool).await {
        Ok(options) => render(&IdentityFormTemplate {
            form,
            options,
            employment_types: &identity::EMPLOYMENT_TYPES,
            error,
            duplicate_warning,
        }),
        Err(e) => internal("form seçenekleri okunamadı", e),
    }
}

async fn create(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<IdentityForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let new = match identity::validate(&form) {
        Ok(n) => n,
        Err(msg) => return render_form(&state, form, msg, false).await,
    };
    if new.national_id.is_none() && form.confirm_duplicate.is_none() {
        match identity::similar_name_exists(&state.pool, &new.given_name, &new.surname).await {
            Ok(true) => return render_form(&state, form, String::new(), true).await,
            Ok(false) => {}
            Err(e) => return internal("mükerrer kişi kontrolü", e),
        }
    }
    let keys = crate::national_id::Keys {
        aead: &state.aead_key,
        blind_index: &state.blind_index_key,
    };
    let id = match identity::create(&state.pool, &keys, &state.time_zone, &new).await {
        Ok(id) => id,
        Err(identity::CreateError::DuplicateNationalId) => {
            let msg = "Bu kimlik numarası zaten kayıtlı".to_string();
            return render_form(&state, form, msg, false).await;
        }
        Err(identity::CreateError::Db(e)) => return internal("kimlik kaydedilemedi", e),
    };
    let detail = serde_json::json!({
        "employee_number": new.employee_number,
        "department_id": new.department_id,
        "primary_role_id": new.primary_role_id,
        "employment_type": new.employment_type,
        "start_date": new.start_date,
        "end_date": new.end_date,
        "national_id_set": new.national_id.is_some(),
    });
    audit_operator(
        &state,
        &op,
        crate::audit::IDENTITY_CREATED,
        Some(id),
        detail,
    )
    .await;
    // Kayit tamam; is acilamazsa log'a duser, zamanlayici (uctan uca kutucugu) yakalar.
    if let Err(e) =
        identity::enqueue_all_targets(&state.pool, id, crate::jobs::Priority::Single).await
    {
        eprintln!("identity_web: iş açılamadı (kimlik {id}): {e}");
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

async fn show(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match identity::load_page(&state.pool, &state.time_zone, &state.aead_key, id).await {
        Ok(Some(page)) => render(&PersonTemplate {
            page,
            can_retry: allowed(&op, RETRY_AUTHORITIES),
        }),
        Ok(None) => (StatusCode::NOT_FOUND, "Kimlik bulunamadı.").into_response(),
        Err(e) => internal("kişi sayfası okunamadı", e),
    }
}

async fn retry(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((id, job_id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, RETRY_AUTHORITIES) {
        return forbidden();
    }
    match crate::jobs::request_retry(&state.pool, job_id, id).await {
        Ok(true) => {
            let detail = serde_json::json!({ "job_id": job_id });
            audit_operator(
                &state,
                &op,
                crate::audit::JOB_RETRY_REQUESTED,
                Some(id),
                detail,
            )
            .await;
        }
        // Is mudahalede degil ya da bu kimligin degil: sayfa guncel durumu gosterir.
        Ok(false) => {}
        Err(e) => return internal("tekrar dene isteği yazılamadı", e),
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

pub(crate) async fn audit_operator(
    state: &AppState,
    operator: &Operator,
    event_type: &str,
    identity_id: Option<i64>,
    detail: serde_json::Value,
) {
    let actor = crate::audit::Actor {
        subject: Some(&operator.subject),
        username: &operator.username,
    };
    if let Err(e) = crate::audit::record(&state.pool, &actor, event_type, identity_id, detail).await
    {
        eprintln!("web: denetim kaydı yazılamadı ({event_type}): {e}");
    }
}
// --- END FEATURE: identity-registration ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    async fn operator_cookie(pool: &sqlx::PgPool, authorities: &[&str]) -> String {
        let operator = Operator {
            subject: "sub-ik".to_string(),
            username: "ik.operatoru".to_string(),
            email: "ik@example.org".to_string(),
            authorities: authorities.iter().map(|a| a.to_string()).collect(),
        };
        let token = crate::operator_session::create_session(pool, &operator)
            .await
            .unwrap();
        format!("{OPERATOR_SESSION_COOKIE_NAME}={token}")
    }

    fn request(method: &str, uri: &str, body: &str, cookie: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .header(header::COOKIE, cookie)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn register_flow_person_page_and_retry_authority() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        crate::test_support::seed_example_catalog(&pool).await;
        // web::routes() kimlik rotalarini zaten iceriyor (guard katmani ayni yerde sarar).
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let auditor = operator_cookie(&pool, &["auditor"]).await;
        let dept: i64 = sqlx::query_scalar("SELECT id FROM departments LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let role: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE kind = 'primary' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        // Oturumsuz → giris; yetkisiz operator → 403.
        let r = app
            .clone()
            .oneshot(request("GET", "/identities/new", "", ""))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let r = app
            .clone()
            .oneshot(request("GET", "/identities/new", "", &auditor))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Ayni ad-soyad: once uyari, "yine de kaydet" ile kayit → kisi sayfasina yonlendirme.
        let base = format!(
            "given_name=Ay%C5%9Fe&surname=Y%C4%B1lmaz&department_id={dept}&primary_role_id={role}\
             &employment_type=permanent&start_date=2026-10-01&national_id_country=TR"
        );
        let r = app
            .clone()
            .oneshot(request("POST", "/identities", &base, &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_string(r).await.contains("Mükerrer kişi olabilir"));
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                "/identities",
                &format!("{base}&confirm_duplicate=1"),
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let location = r.headers()["location"].to_str().unwrap().to_string();
        assert!(location.starts_with("/identities/"), "{location}");
        let id: i64 = location.rsplit('/').next().unwrap().parse().unwrap();

        // Gecersiz form: operator dilinde hata, kayit yok.
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                "/identities",
                "given_name=&surname=X&employment_type=permanent",
                &hr,
            ))
            .await
            .unwrap();
        assert!(body_string(r).await.contains("Ad boş olamaz"));

        // Kisi sayfasi: auditor da gorur ama tekrar dene dugmesi yok; hr gorur.
        let job_id: i64 = sqlx::query_scalar(
            "UPDATE jobs SET status = 'needs_intervention', \
             last_error = 'müdahale gerekiyor: kullanıcı adı çakışıyor: ayse.yilmaz AD''de bağlı değil' \
             WHERE identity_id = $1 RETURNING id",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let page = format!("/identities/{id}");
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &auditor))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            body.contains("Ayşe Yılmaz") && body.contains("aktif"),
            "{body}"
        );
        assert!(body.contains("kullanıcı adı çakışıyor"));
        assert!(body.contains("AD&#x27;de bağlı değil") || body.contains("AD'de bağlı değil"));
        assert!(!body.contains("Tekrar dene"));
        assert!(
            body.contains("identity.created"),
            "denetim satırı listelenir"
        );
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("Tekrar dene"));

        // Tekrar dene: yalnizca yetkili; is bayragi kalkar, denetim satiri yazilir.
        let retry = format!("/identities/{id}/jobs/{job_id}/retry");
        let r = app
            .clone()
            .oneshot(request("POST", &retry, "", &auditor))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = app
            .clone()
            .oneshot(request("POST", &retry, "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let flagged: bool = sqlx::query_scalar("SELECT retry_requested FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(flagged);
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE identity_id = $1 AND actor_username = 'ik.operatoru'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 2, "identity.created + job.retry_requested");
        let r = app
            .clone()
            .oneshot(request("GET", "/identities/999999", "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
