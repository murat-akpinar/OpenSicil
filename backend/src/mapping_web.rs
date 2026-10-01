// --- START FEATURE: attribute-mapping ---
// Hedef sistem basina oznitelik eslemesi ekrani (ADR-012/029/034/082): satir
// ekle/sil. Hedef oznitelik ve kaynak ikiz mapping_rules listesinden; hassas
// kaynak yalnizca admin + acik onay. Yetkili worker'dir, ekran kopyayi sunar.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::mapping_rules;
use crate::operator_session::Operator;
use crate::shell::Shell;
use crate::web::{render, AppState};

const WRITE_AUTHORITIES: &[&str] = &["role_admin", "admin"];
const SENSITIVE_AUTHORITIES: &[&str] = &["admin"];

pub struct MappingRow {
    pub id: i64,
    pub attribute: String,
    /// Kaynak ve donusumun i18n etiket anahtarlari (mapping_rules, ADR-089)
    pub source: &'static str,
    pub source_text: String,
    pub transform: &'static str,
    pub write_if_empty: bool,
    pub sensitive: bool,
}

pub async fn list(pool: &PgPool, target: i64) -> Result<Vec<MappingRow>, sqlx::Error> {
    let rows: Vec<(i64, String, String, Option<String>, String, bool)> = sqlx::query_as(
        "SELECT id, target_attribute, source_kind, source_text, transform, write_if_empty \
         FROM attribute_mappings WHERE target_system_id = $1 ORDER BY target_attribute",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, attribute, source_kind, text, transform, write_if_empty)| MappingRow {
                id,
                attribute,
                source: mapping_rules::source_label_key(&source_kind),
                source_text: text.unwrap_or_default(),
                transform: mapping_rules::transform_label_key(&transform),
                write_if_empty,
                sensitive: mapping_rules::source_is_sensitive(&source_kind) == Some(true),
            },
        )
        .collect())
}

pub struct NewMapping {
    pub attribute: String,
    pub source_kind: String,
    pub source_text: Option<String>,
    pub transform: String,
    pub write_if_empty: bool,
    pub sensitive_ack: bool,
}

// Ekran dogrulamasi worker'in kopyasidir (ADR-029); worker yine de reddeder.
// Doner: i18n anahtari (ADR-089); ceviri web katmaninda.
pub fn validate(target_kind: &str, m: &NewMapping, is_admin: bool) -> Result<(), &'static str> {
    if !mapping_rules::attribute_allowed(target_kind, &m.attribute) {
        return Err("err.attribute_not_allowed");
    }
    let sensitive = match mapping_rules::source_is_sensitive(&m.source_kind) {
        Some(s) => s,
        None => return Err("err.source_required"),
    };
    if sensitive && !(is_admin && m.sensitive_ack) {
        return Err("err.sensitive_not_allowed");
    }
    if !mapping_rules::transform_known(&m.transform) {
        return Err("err.transform_required");
    }
    let needs_text = matches!(m.source_kind.as_str(), "constant" | "template");
    if needs_text && m.source_text.as_deref().unwrap_or("").trim().is_empty() {
        return Err("err.constant_text_required");
    }
    Ok(())
}

pub async fn add(pool: &PgPool, target: i64, m: &NewMapping) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO attribute_mappings (target_system_id, target_attribute, source_kind, \
         source_text, transform, write_if_empty, sensitive_acknowledged) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(target)
    .bind(&m.attribute)
    .bind(&m.source_kind)
    .bind(m.source_text.as_deref().map(str::trim))
    .bind(&m.transform)
    .bind(m.write_if_empty)
    .bind(m.sensitive_ack)
    .fetch_one(pool)
    .await
}

pub async fn delete(pool: &PgPool, target: i64, id: i64) -> Result<bool, sqlx::Error> {
    let done =
        sqlx::query("DELETE FROM attribute_mappings WHERE id = $1 AND target_system_id = $2")
            .bind(id)
            .bind(target)
            .execute(pool)
            .await?;
    Ok(done.rows_affected() == 1)
}

// Esleme degisince o hedefte yonetilen hesabi olan herkes icin toplu is (sahneleme 3f'te).
pub async fn enqueue_linked(pool: &PgPool, target: i64) -> Result<usize, sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT l.identity_id FROM account_links l JOIN identities i ON i.id = l.identity_id \
         WHERE l.target_system_id = $1 AND l.mode = 'managed' AND i.deleted_at IS NULL",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    for identity in &ids {
        crate::jobs::enqueue(pool, *identity, target, crate::jobs::Priority::Bulk).await?;
    }
    Ok(ids.len())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/targets/{id}/mappings", get(page).post(create))
        .route("/targets/{id}/mappings/{mid}/delete", post(remove))
}

#[derive(Template)]
#[template(path = "mappings.html")]
struct MappingsTemplate {
    shell: Shell,
    lang: Lang,
    target_id: i64,
    target_name: String,
    rows: Vec<MappingRow>,
    attributes: &'static [&'static str],
    sources: &'static [(&'static str, &'static str, bool)],
    transforms: &'static [(&'static str, &'static str)],
    error: String,
    can_edit: bool,
    can_sensitive: bool,
}

async fn target_of(pool: &PgPool, id: i64) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT kind, name FROM target_systems WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

async fn render_page(state: &AppState, op: &Operator, id: i64, error: String) -> Response {
    let (kind, name) = match target_of(&state.pool, id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.target_not_found")).into_response()
        }
        Err(e) => return internal("hedef sistem okunamadı", e),
    };
    match list(&state.pool, id).await {
        Ok(rows) => render(&MappingsTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            target_id: id,
            target_name: name,
            rows,
            attributes: mapping_rules::allowed_attributes(&kind),
            sources: mapping_rules::SOURCES,
            transforms: mapping_rules::TRANSFORMS,
            error,
            can_edit: allowed(op, WRITE_AUTHORITIES),
            can_sensitive: allowed(op, SENSITIVE_AUTHORITIES),
        }),
        Err(e) => internal("eşleme satırları okunamadı", e),
    }
}

async fn page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    render_page(&state, &op, id, String::new()).await
}

fn field<'a>(form: &'a [(String, String)], key: &str) -> &'a str {
    form.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

async fn create(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let kind = match target_of(&state.pool, id).await {
        Ok(Some((kind, _))) => kind,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.target_not_found")).into_response()
        }
        Err(e) => return internal("hedef sistem okunamadı", e),
    };
    let text = field(&form, "source_text").trim();
    let new = NewMapping {
        attribute: field(&form, "target_attribute").to_string(),
        source_kind: field(&form, "source_kind").to_string(),
        source_text: (!text.is_empty()).then(|| text.to_string()),
        transform: field(&form, "transform").to_string(),
        write_if_empty: field(&form, "write_if_empty") == "1",
        sensitive_ack: field(&form, "sensitive_ack") == "1",
    };
    if let Err(msg) = validate(&kind, &new, allowed(&op, SENSITIVE_AUTHORITIES)) {
        return render_page(&state, &op, id, msg.to_string()).await;
    }
    match add(&state.pool, id, &new).await {
        Ok(mapping_id) => {
            let detail = serde_json::json!({
                "action": "added", "target_id": id, "mapping_id": mapping_id,
                "attribute": new.attribute, "source_kind": new.source_kind,
                "transform": new.transform, "write_if_empty": new.write_if_empty,
                "sensitive_acknowledged": new.sensitive_ack,
            });
            audit_operator(&state, &op, crate::audit::MAPPING_CHANGED, None, detail).await;
            enqueue_after_change(&state, id).await;
            Redirect::to(&format!("/targets/{id}/mappings")).into_response()
        }
        Err(e)
            if e.as_database_error()
                .is_some_and(|d| d.is_unique_violation()) =>
        {
            let msg = op.lang.t("err.attribute_already_mapped").to_string();
            render_page(&state, &op, id, msg).await
        }
        Err(e) => internal("eşleme satırı yazılamadı", e),
    }
}

async fn enqueue_after_change(state: &AppState, target: i64) {
    match enqueue_linked(&state.pool, target).await {
        Ok(n) if n > 0 => println!("web: eşleme değişti, {n} kimlik için iş açıldı"),
        Ok(_) => {}
        Err(e) => eprintln!("web: eşleme sonrası iş açılamadı: {e}"),
    }
}

async fn remove(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((id, mapping_id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    match delete(&state.pool, id, mapping_id).await {
        Ok(true) => {
            let detail = serde_json::json!({ "action": "removed", "target_id": id, "mapping_id": mapping_id });
            audit_operator(&state, &op, crate::audit::MAPPING_CHANGED, None, detail).await;
            enqueue_after_change(&state, id).await;
        }
        Ok(false) => {}
        Err(e) => return internal("eşleme satırı silinemedi", e),
    }
    Redirect::to(&format!("/targets/{id}/mappings")).into_response()
}
// --- END FEATURE: attribute-mapping ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    fn new(attribute: &str, source: &str, text: Option<&str>, ack: bool) -> NewMapping {
        NewMapping {
            attribute: attribute.to_string(),
            source_kind: source.to_string(),
            source_text: text.map(str::to_string),
            transform: "none".to_string(),
            write_if_empty: false,
            sensitive_ack: ack,
        }
    }

    #[test]
    fn validate_mirrors_worker_rules() {
        assert!(validate(
            "ad",
            &new("description", "constant", Some("x"), false),
            false
        )
        .is_ok());
        assert!(
            validate(
                "ad",
                &new("userAccountControl", "constant", Some("512"), false),
                true
            )
            .unwrap_err()
                == "err.attribute_not_allowed"
        );
        assert!(
            validate("ad", &new("mobile", "mobile_phone", None, false), true).unwrap_err()
                == "err.sensitive_not_allowed"
        );
        assert!(
            validate("ad", &new("mobile", "mobile_phone", None, true), false).unwrap_err()
                == "err.sensitive_not_allowed"
        );
        assert!(validate("ad", &new("mobile", "mobile_phone", None, true), true).is_ok());
        assert!(
            validate(
                "ad",
                &new("description", "template", Some(" "), false),
                false
            )
            .unwrap_err()
                == "err.constant_text_required"
        );
        assert!(validate(
            "zimbra",
            &new("employeeID", "employee_number", None, false),
            false
        )
        .is_err());
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn role_admin_adds_and_removes_rows_sensitive_needs_admin_ack() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             VALUES ($1, $2, 'g1', 'provisioned', 'managed')",
        )
        .bind(ids[0])
        .bind(catalog.ad)
        .execute(&pool)
        .await
        .unwrap();
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let cookie = |authorities: &'static [&'static str]| {
            let pool = pool.clone();
            async move {
                let operator = Operator {
                    subject: "sub".to_string(),
                    username: "rol.yoneticisi".to_string(),
                    email: "r@example.org".to_string(),
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
        let role_admin = cookie(&["role_admin"]).await;
        let admin = cookie(&["admin"]).await;
        let url = format!("/targets/{}/mappings", catalog.ad);

        // Varsayilan AD eslemeleri seed'den gelir (ADR-012, sAMAccountName/UPN yok).
        let seeded = list(&pool, catalog.ad).await.unwrap();
        assert_eq!(seeded.len(), 8);
        assert!(seeded.iter().all(|r| r.attribute != "sAMAccountName"));

        let r = app
            .clone()
            .oneshot(request("POST", &url, "target_attribute=description&source_kind=constant&source_text=Personel&transform=none&write_if_empty=1", &role_admin))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &url,
                "target_attribute=description&source_kind=constant&source_text=X&transform=none",
                &role_admin,
            ))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "aynı öznitelik ikinci kez: hata sayfada"
        );
        let r = app
            .clone()
            .oneshot(request("POST", &url, "target_attribute=mobile&source_kind=mobile_phone&transform=phone_national&sensitive_ack=1", &role_admin))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "hassas: role_admin yetmez");
        let r = app
            .clone()
            .oneshot(request("POST", &url, "target_attribute=mobile&source_kind=mobile_phone&transform=phone_national&sensitive_ack=1", &admin))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let rows = list(&pool, catalog.ad).await.unwrap();
        assert_eq!(rows.len(), 10);
        let mobile = rows.iter().find(|r| r.attribute == "mobile").unwrap();
        assert!(mobile.sensitive);
        let description = rows.iter().find(|r| r.attribute == "description").unwrap();
        assert!(description.write_if_empty && description.source_text == "Personel");
        let jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND priority = 2")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            jobs, 1,
            "bağlı kimlik için toplu iş; açık iş tekrar açılmaz"
        );

        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("{url}/{}/delete", mobile.id),
                "",
                &role_admin,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(list(&pool, catalog.ad).await.unwrap().len(), 9);
        let page = app
            .clone()
            .oneshot(request("GET", &url, "", &role_admin))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE event_type = $1")
                .bind(crate::audit::MAPPING_CHANGED)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(events, 3);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
