// --- START FEATURE: used-names ---
// Kullanilmis ad kaydi (ADR-035/042): listeleme her operator, serbest birakma
// Sistem yoneticisi ve gerekceyle; gerekce denetim kaydina girer (docs/07).
// Worker yakar (silmede), backend yalnizca released_at/release_reason yazar.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::operator_session::Operator;
use crate::shell::Shell;
use crate::web::{render, AppState};

const RELEASE_AUTHORITIES: &[&str] = &["admin"];

pub struct UsedName {
    pub id: i64,
    pub name: String,
    /// Veritabani turu (username/email); ekran karsiligi i18n'de (usedkind.<tur>)
    pub kind: String,
    pub former_identity_id: String,
    pub burned_at: String,
    pub released: String,
}

pub async fn list(pool: &PgPool, time_zone: &str) -> Result<Vec<UsedName>, sqlx::Error> {
    type Row = (
        i64,
        String,
        String,
        Option<i64>,
        String,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, kind, former_identity_id, \
         to_char(burned_at AT TIME ZONE $1, 'YYYY-MM-DD HH24:MI'), \
         to_char(released_at AT TIME ZONE $1, 'YYYY-MM-DD HH24:MI'), release_reason \
         FROM used_names ORDER BY (released_at IS NULL) DESC, burned_at DESC",
    )
    .bind(time_zone)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, kind, former, burned_at, released_at, reason)| UsedName {
                id,
                name,
                kind,
                former_identity_id: former.map(|i| format!("#{i}")).unwrap_or_default(),
                burned_at,
                released: match (released_at, reason) {
                    (Some(at), Some(reason)) => format!("{at} — {reason}"),
                    _ => String::new(),
                },
            },
        )
        .collect())
}

// Doner: serbest birakildi mi (zaten serbestse ya da yoksa false).
pub async fn release(pool: &PgPool, id: i64, reason: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE used_names SET released_at = now(), release_reason = $2 \
         WHERE id = $1 AND released_at IS NULL",
    )
    .bind(id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/used-names", get(page))
        .route("/used-names/{id}/release", post(release_submit))
}

#[derive(Template)]
#[template(path = "used_names.html")]
struct UsedNamesTemplate {
    tabs: crate::shell::Tabs,
    shell: Shell,
    lang: Lang,
    names: Vec<UsedName>,
    error: String,
    can_release: bool,
}

async fn render_page(state: &AppState, op: &Operator, error: String) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    match tokio::try_join!(
        list(&state.pool, &time_zone),
        crate::identity_web::personnel_tabs(&state.pool, op.lang, "/used-names")
    ) {
        Ok((names, tabs)) => render(&UsedNamesTemplate {
            tabs,
            lang: op.lang,
            shell: Shell::of(op),
            names,
            error,
            can_release: allowed(op, RELEASE_AUTHORITIES),
        }),
        Err(e) => internal("kullanılmış adlar okunamadı", e),
    }
}

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    render_page(&state, &op, String::new()).await
}

#[derive(Deserialize)]
struct ReleaseForm {
    #[serde(default)]
    reason: String,
}

async fn release_submit(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<ReleaseForm>,
) -> Response {
    if !allowed(&op, RELEASE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let reason = form.reason.trim();
    if reason.is_empty() {
        return render_page(&state, &op, op.lang.t("err.reason_required").to_string()).await;
    }
    match release(&state.pool, id, reason).await {
        Ok(true) => {
            let detail = serde_json::json!({ "used_name_id": id, "reason": reason });
            audit_operator(&state, &op, crate::audit::USED_NAME_RELEASED, None, detail).await;
            Redirect::to("/used-names").into_response()
        }
        Ok(false) => (StatusCode::CONFLICT, op.lang.t("err.used_name_not_found")).into_response(),
        Err(e) => internal("serbest bırakma yazılamadı", e),
    }
}
// --- END FEATURE: used-names ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn admin_releases_with_reason_others_cannot() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO used_names (name, kind) VALUES ('eski.ad', 'username') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let cookie = |authorities: &'static [&'static str]| {
            let pool = pool.clone();
            async move {
                let operator = Operator {
                    subject: "sub".to_string(),
                    username: "sistem.yoneticisi".to_string(),
                    email: "sy@example.org".to_string(),
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
        let request = |method: &str, uri: &str, body: &str, cookie: &str| {
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie)
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let hr = cookie(&["hr"]).await;
        let admin = cookie(&["admin"]).await;
        let url = format!("/used-names/{id}/release");

        let r = app
            .clone()
            .oneshot(request("POST", &url, "reason=test", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = app
            .clone()
            .oneshot(request("POST", &url, "reason=", &admin))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "gerekçesiz: form yeniden");
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &url,
                "reason=m%C3%BCkerrer+kay%C4%B1t",
                &admin,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let r = app
            .clone()
            .oneshot(request("POST", &url, "reason=tekrar", &admin))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::CONFLICT,
            "ikinci kez serbest bırakılamaz"
        );

        let listed = list(&pool, "Europe/Istanbul").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].released.contains("mükerrer kayıt"));
        let page = app
            .clone()
            .oneshot(request("GET", "/used-names", "", &hr))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE event_type = $1 AND detail->>'reason' = 'mükerrer kayıt'",
        )
        .bind(crate::audit::USED_NAME_RELEASED)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
