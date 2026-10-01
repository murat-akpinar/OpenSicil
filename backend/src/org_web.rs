// --- START FEATURE: role-department-screens ---
// Rol, departman ve hedef sistem ekranlari (ADR-080). Yazma role_admin/admin,
// okuma her operator. Kayit sonrasi etkilenen kimlikler icin toplu is acilir.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::operator_session::Operator;
use crate::org::{self, CatalogOptions, Definition, Owner, SaveError, TargetSetting};
use crate::web::{render, AppState};

const WRITE_AUTHORITIES: &[&str] = &["role_admin", "admin"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/roles", get(roles_page).post(create_role))
        .route("/roles/{id}", get(role_page).post(save_role))
        .route(
            "/departments",
            get(departments_page).post(create_department),
        )
        .route(
            "/departments/{id}",
            get(department_page).post(save_department),
        )
        .route("/targets", get(targets_page))
        .route("/targets/{id}", post(save_target))
}

// Tekrar eden alanlar (entitlement) ve hedef basina ayar alanlari (pa.<hedef> ...).
struct Fields(Vec<(String, String)>);

impl Fields {
    fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    fn all_i64(&self, key: &str) -> Vec<i64> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .filter_map(|(_, v)| v.parse().ok())
            .collect()
    }

    fn opt_i64(&self, key: &str) -> Option<i64> {
        self.get(key).trim().parse().ok()
    }

    fn settings(&self, targets: &[TargetSetting]) -> Vec<TargetSetting> {
        targets
            .iter()
            .map(|t| TargetSetting {
                target_id: t.target_id,
                target_name: t.target_name.clone(),
                provision_account: match self.get(&format!("pa.{}", t.target_id)) {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
                container_item_id: self.opt_i64(&format!("ct.{}", t.target_id)),
                email_domain: self.get(&format!("ed.{}", t.target_id)).to_string(),
                upn_suffix: self.get(&format!("us.{}", t.target_id)).to_string(),
            })
            .collect()
    }
}

struct ItemView {
    id: i64,
    label: String,
    selected: bool,
}

struct TargetView {
    target_id: i64,
    target_name: String,
    memberships: Vec<ItemView>,
    containers: Vec<ItemView>,
    provision: &'static str,
    email_domain: String,
    upn_suffix: String,
}

fn item_label(c: &org::CatalogChoice) -> String {
    let mut label = c.display_name.clone();
    if !c.location.is_empty() && c.location != c.display_name {
        label.push_str(&format!(" ({})", c.location));
    }
    if c.missing {
        label.push_str(" (kayıp)");
    }
    label
}

fn target_views(def: &Definition, options: &CatalogOptions) -> Vec<TargetView> {
    let items = |list: &[org::CatalogChoice], target: i64, pick: &dyn Fn(i64) -> bool| {
        list.iter()
            .filter(|c| c.target_id == target)
            .map(|c| ItemView {
                id: c.id,
                label: item_label(c),
                selected: pick(c.id),
            })
            .collect::<Vec<_>>()
    };
    def.settings
        .iter()
        .map(|s| TargetView {
            target_id: s.target_id,
            target_name: s.target_name.clone(),
            memberships: items(&options.memberships, s.target_id, &|id| {
                def.entitlement_ids.contains(&id)
            }),
            containers: items(&options.containers, s.target_id, &|id| {
                s.container_item_id == Some(id)
            }),
            provision: match s.provision_account {
                Some(true) => "true",
                Some(false) => "false",
                None => "",
            },
            email_domain: s.email_domain.clone(),
            upn_suffix: s.upn_suffix.clone(),
        })
        .collect()
}

fn settings_json(settings: &[TargetSetting]) -> serde_json::Value {
    settings
        .iter()
        .map(|s| {
            serde_json::json!({
                "target_id": s.target_id,
                "provision_account": s.provision_account,
                "container_item_id": s.container_item_id,
                "email_domain": s.email_domain,
                "upn_suffix": s.upn_suffix,
            })
        })
        .collect()
}

// Kutu: clippy result_large_err (Response buyuk); hata yolu sicak degil.
fn save_error(e: SaveError, what: &str) -> Result<String, Box<Response>> {
    match e {
        SaveError::Invalid(msg) => Ok(msg),
        SaveError::Db(e) => Err(Box::new(internal(what, e))),
    }
}

#[derive(Template)]
#[template(path = "roles.html")]
struct RolesTemplate {
    roles: Vec<org::RoleRow>,
    kinds: &'static [(&'static str, &'static str)],
    error: String,
    can_edit: bool,
}

#[derive(Template)]
#[template(path = "role.html")]
struct RoleTemplate {
    role: org::RoleDetail,
    targets: Vec<TargetView>,
    show_settings: bool,
    error: String,
    can_edit: bool,
}

#[derive(Template)]
#[template(path = "departments.html")]
struct DepartmentsTemplate {
    departments: Vec<org::DepartmentRow>,
    error: String,
    can_edit: bool,
}

#[derive(Template)]
#[template(path = "department.html")]
struct DepartmentTemplate {
    dept: org::DepartmentDetail,
    parents: Vec<ItemView>,
    targets: Vec<TargetView>,
    error: String,
    can_edit: bool,
}

// Ust departman secenekleri: kendisi haric (dongu dogrulamasi yine de sunucuda).
fn parent_options(dept: &org::DepartmentDetail, all: &[org::DepartmentRow]) -> Vec<ItemView> {
    all.iter()
        .filter(|d| d.id != dept.def.id)
        .map(|d| ItemView {
            id: d.id,
            label: format!("{}{}", d.indent, d.name),
            selected: dept.parent_id == Some(d.id),
        })
        .collect()
}

struct TargetFormView {
    target: org::TargetRow,
    containers: Vec<ItemView>,
}

#[derive(Template)]
#[template(path = "targets.html")]
struct TargetsTemplate {
    targets: Vec<TargetFormView>,
    error: String,
    can_edit: bool,
}

async fn render_roles(state: &AppState, op: &Operator, error: String) -> Response {
    match org::list_roles(&state.pool).await {
        Ok(roles) => render(&RolesTemplate {
            roles,
            kinds: &org::ROLE_KINDS,
            error,
            can_edit: allowed(op, WRITE_AUTHORITIES),
        }),
        Err(e) => internal("roller okunamadı", e),
    }
}

async fn roles_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
) -> Response {
    render_roles(&state, &op, String::new()).await
}

async fn create_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden();
    }
    let f = Fields(form);
    match org::create_role(&state.pool, f.get("kind"), f.get("name"), f.get("title")).await {
        Ok(id) => {
            let detail = serde_json::json!({ "action": "created", "role_id": id, "kind": f.get("kind"), "name": f.get("name") });
            audit_operator(&state, &op, crate::audit::ROLE_CHANGED, None, detail).await;
            Redirect::to(&format!("/roles/{id}")).into_response()
        }
        Err(e) => match save_error(e, "rol oluşturulamadı") {
            Ok(msg) => render_roles(&state, &op, msg).await,
            Err(response) => *response,
        },
    }
}

async fn render_role(state: &AppState, op: &Operator, id: i64, error: String) -> Response {
    let (role, options) = match (
        org::load_role(&state.pool, id).await,
        org::catalog_options(&state.pool).await,
    ) {
        (Ok(Some(role)), Ok(options)) => (role, options),
        (Ok(None), _) => return (StatusCode::NOT_FOUND, "Rol bulunamadı.").into_response(),
        (Err(e), _) | (_, Err(e)) => return internal("rol okunamadı", e),
    };
    render(&RoleTemplate {
        targets: target_views(&role.def, &options),
        show_settings: role.kind == "primary",
        role,
        error,
        can_edit: allowed(op, WRITE_AUTHORITIES),
    })
}

async fn role_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    render_role(&state, &op, id, String::new()).await
}

async fn save_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden();
    }
    let current = match org::load_role(&state.pool, id).await {
        Ok(Some(role)) => role,
        Ok(None) => return (StatusCode::NOT_FOUND, "Rol bulunamadı.").into_response(),
        Err(e) => return internal("rol okunamadı", e),
    };
    let f = Fields(form);
    let edit = org::DefinitionEdit {
        name: f.get("name").to_string(),
        entitlement_ids: f.all_i64("entitlement"),
        settings: f.settings(&current.def.settings),
    };
    if let Err(e) = org::save_role(&state.pool, id, f.get("title"), &edit).await {
        return match save_error(e, "rol kaydedilemedi") {
            Ok(msg) => render_role(&state, &op, id, msg).await,
            Err(response) => *response,
        };
    }
    let detail = serde_json::json!({
        "action": "saved", "role_id": id, "name": edit.name,
        "entitlement_ids": edit.entitlement_ids, "settings": settings_json(&edit.settings),
    });
    audit_operator(&state, &op, crate::audit::ROLE_CHANGED, None, detail).await;
    enqueue_affected(&state, Owner::Role, id).await;
    Redirect::to(&format!("/roles/{id}")).into_response()
}

// Is acilamazsa model yine kaydedilmistir; log'a duser, zamanlayici farki yakalar.
async fn enqueue_affected(state: &AppState, owner: Owner, id: i64) {
    match org::enqueue_affected(&state.pool, owner, id).await {
        Ok(n) if n > 0 => println!("web: tanım değişti, {n} kimlik için iş açıldı"),
        Ok(_) => {}
        Err(e) => eprintln!("web: etkilenen kimlikler için iş açılamadı: {e}"),
    }
}

async fn render_departments(state: &AppState, op: &Operator, error: String) -> Response {
    match org::list_departments(&state.pool).await {
        Ok(departments) => render(&DepartmentsTemplate {
            departments,
            error,
            can_edit: allowed(op, WRITE_AUTHORITIES),
        }),
        Err(e) => internal("departmanlar okunamadı", e),
    }
}

async fn departments_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
) -> Response {
    render_departments(&state, &op, String::new()).await
}

async fn create_department(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden();
    }
    let f = Fields(form);
    let parent = f.opt_i64("parent_id");
    match org::create_department(&state.pool, f.get("name"), f.get("code"), parent).await {
        Ok(id) => {
            let detail = serde_json::json!({ "action": "created", "department_id": id, "name": f.get("name"), "parent_id": parent });
            audit_operator(&state, &op, crate::audit::DEPARTMENT_CHANGED, None, detail).await;
            Redirect::to(&format!("/departments/{id}")).into_response()
        }
        Err(e) => match save_error(e, "departman oluşturulamadı") {
            Ok(msg) => render_departments(&state, &op, msg).await,
            Err(response) => *response,
        },
    }
}

async fn render_department(state: &AppState, op: &Operator, id: i64, error: String) -> Response {
    let dept = match org::load_department(&state.pool, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "Departman bulunamadı.").into_response(),
        Err(e) => return internal("departman okunamadı", e),
    };
    let (departments, options) = match (
        org::list_departments(&state.pool).await,
        org::catalog_options(&state.pool).await,
    ) {
        (Ok(d), Ok(o)) => (d, o),
        (Err(e), _) | (_, Err(e)) => return internal("departman seçenekleri okunamadı", e),
    };
    render(&DepartmentTemplate {
        targets: target_views(&dept.def, &options),
        parents: parent_options(&dept, &departments),
        dept,
        error,
        can_edit: allowed(op, WRITE_AUTHORITIES),
    })
}

async fn department_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    render_department(&state, &op, id, String::new()).await
}

async fn save_department(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden();
    }
    let current = match org::load_department(&state.pool, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "Departman bulunamadı.").into_response(),
        Err(e) => return internal("departman okunamadı", e),
    };
    let f = Fields(form);
    let parent = f.opt_i64("parent_id");
    let edit = org::DefinitionEdit {
        name: f.get("name").to_string(),
        entitlement_ids: f.all_i64("entitlement"),
        settings: f.settings(&current.def.settings),
    };
    if let Err(e) = org::save_department(&state.pool, id, f.get("code"), parent, &edit).await {
        return match save_error(e, "departman kaydedilemedi") {
            Ok(msg) => render_department(&state, &op, id, msg).await,
            Err(response) => *response,
        };
    }
    let detail = serde_json::json!({
        "action": "saved", "department_id": id, "name": edit.name, "code": f.get("code"),
        "parent_id": parent, "entitlement_ids": edit.entitlement_ids,
        "settings": settings_json(&edit.settings),
    });
    audit_operator(&state, &op, crate::audit::DEPARTMENT_CHANGED, None, detail).await;
    enqueue_affected(&state, Owner::Department, id).await;
    Redirect::to(&format!("/departments/{id}")).into_response()
}

async fn render_targets(state: &AppState, op: &Operator, error: String) -> Response {
    let (targets, options) = match (
        org::list_targets(&state.pool).await,
        org::catalog_options(&state.pool).await,
    ) {
        (Ok(t), Ok(o)) => (t, o),
        (Err(e), _) | (_, Err(e)) => return internal("hedef sistemler okunamadı", e),
    };
    let views = targets
        .into_iter()
        .map(|target| TargetFormView {
            containers: options
                .containers
                .iter()
                .filter(|c| c.target_id == target.id)
                .map(|c| ItemView {
                    id: c.id,
                    label: item_label(c),
                    selected: target.default_container_item_id == Some(c.id),
                })
                .collect(),
            target,
        })
        .collect();
    render(&TargetsTemplate {
        targets: views,
        error,
        can_edit: allowed(op, WRITE_AUTHORITIES),
    })
}

async fn targets_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
) -> Response {
    render_targets(&state, &op, String::new()).await
}

async fn save_target(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden();
    }
    let Some(mut target) = org::list_targets(&state.pool)
        .await
        .map(|t| t.into_iter().find(|t| t.id == id))
        .unwrap_or_default()
    else {
        return (StatusCode::NOT_FOUND, "Hedef sistem bulunamadı.").into_response();
    };
    let f = Fields(form);
    target.provision_account_default = f.get("provision_account_default") == "1";
    target.default_container_item_id = f.opt_i64("default_container_item_id");
    target.delete_requires_approval = f.get("delete_requires_approval") == "1";
    let (Some(retention), Some(delay)) = (
        f.get("retention_days").trim().parse().ok(),
        f.get("password_reset_delay_days").trim().parse().ok(),
    ) else {
        return render_targets(&state, &op, "Gün sayıları tam sayı olmalı".to_string()).await;
    };
    target.retention_days = retention;
    target.password_reset_delay_days = delay;
    if let Err(e) = org::save_target(&state.pool, &target).await {
        return match save_error(e, "hedef sistem kaydedilemedi") {
            Ok(msg) => render_targets(&state, &op, msg).await,
            Err(response) => *response,
        };
    }
    let detail = serde_json::json!({
        "target_id": id, "provision_account_default": target.provision_account_default,
        "default_container_item_id": target.default_container_item_id,
        "retention_days": retention, "delete_requires_approval": target.delete_requires_approval,
        "password_reset_delay_days": delay,
    });
    audit_operator(&state, &op, crate::audit::TARGET_CHANGED, None, detail).await;
    Redirect::to("/targets").into_response()
}
// --- END FEATURE: role-department-screens ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    async fn cookie(pool: &sqlx::PgPool, authorities: &[&str]) -> String {
        let operator = Operator {
            subject: "sub-rol".to_string(),
            username: "rol.yoneticisi".to_string(),
            email: "rol@example.org".to_string(),
            authorities: authorities.iter().map(|a| a.to_string()).collect(),
        };
        let token = crate::operator_session::create_session(pool, &operator)
            .await
            .unwrap();
        format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME)
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

    fn location(response: &Response) -> String {
        response.headers()["location"].to_str().unwrap().to_string()
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn role_admin_edits_roles_departments_and_targets_auditor_only_reads() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let admin = cookie(&pool, &["role_admin"]).await;
        let auditor = cookie(&pool, &["auditor"]).await;
        let send = |method: &'static str, uri: String, body: String, c: String| {
            let app = app.clone();
            async move { app.oneshot(request(method, &uri, &body, &c)).await.unwrap() }
        };

        // Auditor okur, yazamaz.
        let r = send("GET", "/roles".into(), String::new(), auditor.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(!body_string(r).await.contains("Kaydet"));
        let r = send(
            "POST",
            "/roles".into(),
            "kind=primary&name=X".into(),
            auditor.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Rol olustur → duzenle: uyelik + ayar; etkilenen kimlikler icin toplu is.
        let r = send(
            "POST",
            "/roles".into(),
            "kind=primary&name=Uzman&title=Sistem+Uzman%C4%B1".into(),
            admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let role_url = location(&r);
        let role_id: i64 = role_url.rsplit('/').next().unwrap().parse().unwrap();
        sqlx::query("UPDATE identities SET primary_role_id = $1 WHERE id = $2")
            .bind(role_id)
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        let body = format!(
            "name=Uzman&title=Uzman&entitlement={}&entitlement={}&pa.{}=true&ct.{}={}&ed.{}=example.com&us.{}=",
            catalog.gg_vpn, catalog.gg_nobet, catalog.ad, catalog.ad, catalog.sistem_uzmanlari_ou, catalog.ad, catalog.ad
        );
        let r = send("POST", role_url.clone(), body, admin.clone()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{}", location(&r));
        let page =
            body_string(send("GET", role_url.clone(), String::new(), admin.clone()).await).await;
        assert!(
            page.contains("GG-VPN") && page.contains("checked"),
            "{page}"
        );
        assert!(page.contains("example.com"));
        let jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND priority = 2")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(jobs, 2);

        // Bos ad: operator dilinde hata, sayfa yeniden.
        let r = send("POST", role_url.clone(), "name=".into(), admin.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_string(r).await.contains("boş olamaz"));

        // Departman: olustur, alt departman, dongu reddi.
        let r = send(
            "POST",
            "/departments".into(),
            "name=Ankara&code=ANK".into(),
            admin.clone(),
        )
        .await;
        let root_url = location(&r);
        let root_id: i64 = root_url.rsplit('/').next().unwrap().parse().unwrap();
        let r = send(
            "POST",
            "/departments".into(),
            format!("name=BT&parent_id={root_id}"),
            admin.clone(),
        )
        .await;
        let child_id: i64 = location(&r).rsplit('/').next().unwrap().parse().unwrap();
        let r = send(
            "POST",
            root_url.clone(),
            format!("name=Ankara&code=ANK&parent_id={child_id}"),
            admin.clone(),
        )
        .await;
        assert!(body_string(r).await.contains("alt departmanı olamaz"));
        let page =
            body_string(send("GET", "/departments".into(), String::new(), auditor.clone()).await)
                .await;
        assert!(
            page.contains(&format!(
                "— <a class=\"underline\" href=\"/departments/{child_id}\">BT"
            )),
            "{page}"
        );

        // Hedef sistem varsayilanlari.
        let r = send(
            "POST",
            format!("/targets/{}", catalog.ad),
            format!("provision_account_default=1&default_container_item_id={}&retention_days=45&password_reset_delay_days=7", catalog.personel_ou),
            admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let page =
            body_string(send("GET", "/targets".into(), String::new(), admin.clone()).await).await;
        assert!(page.contains("value=\"45\""), "{page}");
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE event_type IN ('role.changed', 'department.changed', 'target.changed')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 5);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
