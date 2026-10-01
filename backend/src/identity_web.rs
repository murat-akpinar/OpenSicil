// --- START FEATURE: identity-registration ---
// Kimlik kayit formu, kisi sayfasi ve "tekrar dene" rotalari (F-12, ADR-078).
// Yetki: kayit ve tekrar dene hr/admin, sayfa her operator; yardim masasi yalnizca
// ilk parola ister (ADR-019/085, first_password.rs).

use askama::Template;
use axum::extract::{Form, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::cookie::{get_cookie, OPERATOR_SESSION_COOKIE_NAME};
use crate::identity::{self, FormOptions, IdentityForm, PersonPage};
use crate::operator_session::Operator;
use crate::web::{render, AppState};

const REGISTER_AUTHORITIES: &[&str] = &["hr", "admin"];
const RETRY_AUTHORITIES: &[&str] = &["hr", "admin"];

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
        .route("/identities/{id}/names", post(request_names))
        .route("/identities/{id}/edit", get(edit_form).post(edit_submit))
        .route("/identities/{id}/roles", post(assign_role))
        .route("/identities/{id}/roles/{role_id}/delete", post(remove_role))
        .route("/identities/{id}/departure", post(departure))
        .route("/identities/{id}/emergency", post(emergency))
        .route("/identities/{id}/revert", post(revert))
        .route("/identities/{id}/cancel", post(cancel))
        .route("/identities/{id}/suspension", post(suspend))
        .route("/identities/{id}/suspension/lift", post(lift))
        .route("/identities/{id}/accounts/{target_id}/manage", post(manage))
}

// --- START FEATURE: adoption ---
// ADR-018/087 yonetime alma: operator gozlem farkini gorup onaylar, backend istegi
// yazar ve is acar; modu worker cevirir, farki o is uygular.
async fn manage(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((id, target_id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    match identity::request_management(&state.pool, id, target_id).await {
        Ok(true) => {
            let detail = serde_json::json!({ "target_system_id": target_id });
            audit_operator(
                &state,
                &op,
                crate::audit::ACCOUNT_MANAGE_REQUESTED,
                Some(id),
                detail,
            )
            .await;
            if let Err(e) =
                crate::jobs::enqueue(&state.pool, id, target_id, crate::jobs::Priority::Single)
                    .await
            {
                return internal("yönetime alma işi açılamadı", e);
            }
            Redirect::to(&format!("/identities/{id}")).into_response()
        }
        Ok(false) => bad("Yönetime alınacak gözlem modunda hesap yok (ADR-018)."),
        Err(e) => internal("yönetime alma isteği yazılamadı", e),
    }
}
// --- END FEATURE: adoption ---

// --- Yasam dongusu (docs/04 Leaver, aski, iptal; ADR-084) ---

#[derive(Deserialize)]
struct LifecycleForm {
    #[serde(default)]
    end_date: String,
    #[serde(default)]
    handover_manager_id: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    return_day: String,
    #[serde(default)]
    suspension_start: String,
    #[serde(default)]
    suspension_end: String,
}

fn opt(value: &str) -> Option<&str> {
    let v = value.trim();
    (!v.is_empty()).then_some(v)
}

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

async fn finish_lifecycle(
    state: &AppState,
    op: &Operator,
    id: i64,
    event: &str,
    detail: serde_json::Value,
    priority: crate::jobs::Priority,
) -> Response {
    audit_operator(state, op, event, Some(id), detail).await;
    // ADR-018: ayrilis gozlem modundaki kimlikte yonetime almayi da icerir; yoksa
    // "ayrilis kaydedildi ama hicbir sey olmadi" durumu olusur.
    if event == crate::audit::IDENTITY_DEPARTURE_SET
        || event == crate::audit::IDENTITY_EMERGENCY_DEPARTURE
    {
        match identity::request_management_observed(&state.pool, id).await {
            Ok(0) => {}
            Ok(_) => {
                let detail = serde_json::json!({ "reason": "departure" });
                let event = crate::audit::ACCOUNT_MANAGE_REQUESTED;
                audit_operator(state, op, event, Some(id), detail).await;
            }
            Err(e) => eprintln!("web: yönetime alma istenemedi (kimlik {id}): {e}"),
        }
    }
    if let Err(e) = identity::enqueue_all_targets(&state.pool, id, priority).await {
        eprintln!("web: iş açılamadı (kimlik {id}): {e}");
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

async fn departure(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let handover = opt(&form.handover_manager_id).and_then(|h| h.parse::<i64>().ok());
    let outcome = identity::set_departure(
        &state.pool,
        &state.time_zone,
        id,
        form.end_date.trim(),
        handover,
    )
    .await;
    let detail =
        serde_json::json!({ "end_date": form.end_date.trim(), "handover_manager_id": handover });
    match outcome {
        Ok(identity::LifecycleChange::Applied) => {
            let event = crate::audit::IDENTITY_DEPARTURE_SET;
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Single,
            )
            .await
        }
        // ADR-059 madde 2: ayrildidan her cikis geri almadir (yikici sayaca girer, 3f)
        Ok(identity::LifecycleChange::Reverted) => {
            let event = crate::audit::IDENTITY_DEPARTURE_REVERTED;
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Single,
            )
            .await
        }
        Ok(identity::LifecycleChange::Rejected(msg)) => bad(&msg),
        Err(e) => internal("ayrılış yazılamadı", e),
    }
}

async fn emergency(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let Some(reason) = opt(&form.reason) else {
        return bad("Acil ayrılış için gerekçe zorunlu.");
    };
    let handover = opt(&form.handover_manager_id).and_then(|h| h.parse::<i64>().ok());
    match identity::set_emergency_departure(&state.pool, id, handover).await {
        Ok(true) => {
            let detail = serde_json::json!({ "reason": reason, "handover_manager_id": handover });
            let event = crate::audit::IDENTITY_EMERGENCY_DEPARTURE;
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Emergency,
            )
            .await
        }
        Ok(false) => bad("Kimlik silinmiş ya da devir yöneticisi kendisi."),
        Err(e) => internal("acil ayrılış yazılamadı", e),
    }
}

async fn revert(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let return_day = opt(&form.return_day);
    if return_day.is_some_and(|d| crate::desired_state::Date::from_iso(d).is_none()) {
        return bad("Dönüş günü YYYY-AA-GG olmalı.");
    }
    match identity::revert_departure(&state.pool, id, return_day).await {
        Ok(true) => {
            let detail = serde_json::json!({ "return_day": return_day });
            let event = crate::audit::IDENTITY_DEPARTURE_REVERTED;
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Single,
            )
            .await
        }
        Ok(false) => bad(
            "Geri alınacak bitiş yok, kimlik silinmiş ya da kadrolu dışı: kadrolu dışında ileri tarihli yeni bitiş girin.",
        ),
        Err(e) => internal("geri alma yazılamadı", e),
    }
}

async fn cancel(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    match identity::cancel_registration(&state.pool, id).await {
        Ok(true) => {
            let event = crate::audit::IDENTITY_CANCELLED;
            let detail = serde_json::json!({});
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Single,
            )
            .await
        }
        Ok(false) => {
            bad("Sahiplenilmiş hesabı olan ya da silinmiş kimlik iptal edilemez (ADR-048).")
        }
        Err(e) => internal("iptal yazılamadı", e),
    }
}

async fn suspend(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let end = opt(&form.suspension_end);
    match identity::set_suspension(&state.pool, id, form.suspension_start.trim(), end).await {
        Ok(true) => {
            let detail = serde_json::json!({ "suspension_start": form.suspension_start.trim(), "suspension_end": end });
            let event = crate::audit::IDENTITY_SUSPENDED;
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Single,
            )
            .await
        }
        Ok(false) => bad("Askı tarihleri YYYY-AA-GG olmalı, son gün ilk günden önce olamaz."),
        Err(e) => internal("askı yazılamadı", e),
    }
}

async fn lift(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    match identity::lift_suspension(&state.pool, id).await {
        Ok(true) => {
            let event = crate::audit::IDENTITY_SUSPENSION_LIFTED;
            let detail = serde_json::json!({});
            finish_lifecycle(
                &state,
                &op,
                id,
                event,
                detail,
                crate::jobs::Priority::Single,
            )
            .await
        }
        Ok(false) => bad("Kaldırılacak askı yok."),
        Err(e) => internal("askı kaldırılamadı", e),
    }
}

// --- Gorev degisikligi (docs/04 Mover, ADR-083): alanlar, ek roller ---

async fn edit_form(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    match identity::load_form(&state.pool, id).await {
        Ok(Some(form)) => render_form(&state, form, String::new(), false, Some(id)).await,
        Ok(None) => (StatusCode::NOT_FOUND, "Kimlik bulunamadı.").into_response(),
        Err(e) => internal("kimlik okunamadı", e),
    }
}

async fn edit_submit(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<IdentityForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let new = match identity::validate(&form) {
        Ok(n) => n,
        Err(msg) => return render_form(&state, form, msg, false, Some(id)).await,
    };
    if let Err(e) = identity::update_mover(&state.pool, id, &new).await {
        let db = e.as_database_error();
        if db.is_some_and(|d| d.is_unique_violation()) {
            let msg = "Bu sicil no başka bir kimlikte kayıtlı".to_string();
            return render_form(&state, form, msg, false, Some(id)).await;
        }
        // docs/03: kadrolu disinda bitis zorunlu; bitis ayrilis ekranindan girilir (3c)
        if db.is_some_and(|d| d.is_check_violation()) {
            let msg = "Kadrolu dışı çalışma tipi için önce bitiş tarihi girilmeli".to_string();
            return render_form(&state, form, msg, false, Some(id)).await;
        }
        return internal("kimlik güncellenemedi", e);
    }
    let detail = serde_json::json!({
        "employee_number": new.employee_number, "department_id": new.department_id,
        "primary_role_id": new.primary_role_id, "manager_id": new.manager_id,
        "employment_type": new.employment_type,
    });
    audit_operator(
        &state,
        &op,
        crate::audit::IDENTITY_CHANGED,
        Some(id),
        detail,
    )
    .await;
    enqueue_single(&state, id).await;
    Redirect::to(&format!("/identities/{id}")).into_response()
}

async fn enqueue_single(state: &AppState, id: i64) {
    if let Err(e) =
        identity::enqueue_all_targets(&state.pool, id, crate::jobs::Priority::Single).await
    {
        eprintln!("web: iş açılamadı (kimlik {id}): {e}");
    }
}

#[derive(Deserialize)]
struct RoleForm {
    #[serde(default)]
    role_id: String,
    #[serde(default)]
    ends_on: String,
}

// ADR-020: ek rol istege bagli bitis tarihli; gecmis tarih reddedilir.
async fn assign_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<RoleForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let Ok(role_id) = form.role_id.trim().parse::<i64>() else {
        return (StatusCode::BAD_REQUEST, "Ek rol seçilmeli.").into_response();
    };
    let ends_on = form.ends_on.trim();
    let ends_on = (!ends_on.is_empty()).then_some(ends_on);
    if ends_on.is_some_and(|d| crate::desired_state::Date::from_iso(d).is_none()) {
        return (StatusCode::BAD_REQUEST, "Bitiş tarihi YYYY-AA-GG olmalı.").into_response();
    }
    match identity::assign_role(&state.pool, &state.time_zone, id, role_id, ends_on).await {
        Ok(true) => {
            let detail = serde_json::json!({ "role_id": role_id, "ends_on": ends_on });
            audit_operator(
                &state,
                &op,
                crate::audit::IDENTITY_ROLE_ASSIGNED,
                Some(id),
                detail,
            )
            .await;
            enqueue_single(&state, id).await;
            Redirect::to(&format!("/identities/{id}")).into_response()
        }
        Ok(false) => (
            StatusCode::BAD_REQUEST,
            "Bitiş tarihi geçmiş bir ek rol kaydedilemez (ADR-020).",
        )
            .into_response(),
        Err(e)
            if e.as_database_error()
                .is_some_and(|d| d.is_foreign_key_violation()) =>
        {
            (
                StatusCode::BAD_REQUEST,
                "Yalnızca ek tür roller atanabilir.",
            )
                .into_response()
        }
        Err(e) => internal("ek rol yazılamadı", e),
    }
}

async fn remove_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((id, role_id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    match identity::remove_role(&state.pool, id, role_id).await {
        Ok(true) => {
            let detail = serde_json::json!({ "role_id": role_id });
            audit_operator(
                &state,
                &op,
                crate::audit::IDENTITY_ROLE_REMOVED,
                Some(id),
                detail,
            )
            .await;
            enqueue_single(&state, id).await;
        }
        Ok(false) => {}
        Err(e) => return internal("ek rol silinemedi", e),
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

#[derive(Deserialize)]
struct NamesForm {
    #[serde(default)]
    requested_username: String,
    #[serde(default)]
    name_conflict_override: Option<String>,
}

// ADR-022/042 mudahale secenekleri: farkli ad, "farkli kisi, siradaki adi ver";
// ad olustuktan sonra degistirilemez (409). Kayit, mudahaledeki isleri yeniden dener.
async fn request_names(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<NamesForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let requested = match identity::valid_requested_username(&form.requested_username) {
        Ok(r) => r,
        Err(msg) => return (StatusCode::BAD_REQUEST, msg).into_response(),
    };
    let override_conflicts = form.name_conflict_override.is_some();
    match identity::request_names(&state.pool, id, requested.as_deref(), override_conflicts).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::CONFLICT,
                "Kullanıcı adı zaten oluşmuş, değiştirilemez (ADR-011).",
            )
                .into_response()
        }
        Err(e) => return internal("ad isteği yazılamadı", e),
    }
    let detail = serde_json::json!({
        "requested_username": requested,
        "name_conflict_override": override_conflicts,
    });
    audit_operator(
        &state,
        &op,
        crate::audit::IDENTITY_NAME_REQUESTED,
        Some(id),
        detail,
    )
    .await;
    if let Err(e) = crate::jobs::request_retry_all(&state.pool, id).await {
        eprintln!("web: tekrar dene yazılamadı (kimlik {id}): {e}");
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

#[derive(Template)]
#[template(path = "identity_form.html")]
struct IdentityFormTemplate {
    form: IdentityForm,
    options: FormOptions,
    employment_types: &'static [(&'static str, &'static str)],
    error: String,
    duplicate_warning: bool,
    /// Duzenleme: tarih, kimlik no ve kullanici adi alanlari gizli (ADR-083)
    editing: bool,
    action: String,
    /// ADR-018: mevcut hesap ipucu yalnizca sahiplenme acikken gosterilir (ortak ayar)
    ownership_enabled: bool,
}

#[derive(Template)]
#[template(path = "identity.html")]
struct PersonTemplate {
    page: PersonPage,
    can_retry: bool,
    can_edit_names: bool,
    can_first_password: bool,
}

async fn new_form(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden();
    }
    let form = IdentityForm {
        national_id_country: "TR".to_string(),
        ..IdentityForm::default()
    };
    render_form(&state, form, String::new(), false, None).await
}

async fn render_form(
    state: &AppState,
    form: IdentityForm,
    error: String,
    duplicate_warning: bool,
    editing: Option<i64>,
) -> Response {
    match identity::form_options(&state.pool).await {
        Ok(options) => render(&IdentityFormTemplate {
            form,
            options,
            employment_types: &identity::EMPLOYMENT_TYPES,
            error,
            duplicate_warning,
            editing: editing.is_some(),
            action: match editing {
                Some(id) => format!("/identities/{id}/edit"),
                None => "/identities".to_string(),
            },
            ownership_enabled: crate::common_settings::CommonSettings::from_env()
                .is_ok_and(|c| c.ownership_mode_enabled),
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
        Err(msg) => return render_form(&state, form, msg, false, None).await,
    };
    if new.national_id.is_none() && form.confirm_duplicate.is_none() {
        match identity::similar_name_exists(&state.pool, &new.given_name, &new.surname).await {
            Ok(true) => return render_form(&state, form, String::new(), true, None).await,
            Ok(false) => {}
            Err(e) => return internal("mükerrer kişi kontrolü", e),
        }
    }
    // ADR-056: tek adim yalnizca bugun ya da gecmiste baslayan kayda ve ilk parola yetkisiyle
    let issue = !form.issue_first_password.is_empty();
    if issue {
        if !allowed(&op, crate::first_password::AUTHORITIES) {
            return forbidden();
        }
        match identity::starts_by_today(&state.pool, &new.start_date, &state.time_zone).await {
            Ok(true) => {}
            Ok(false) => {
                let msg = "İlk parola yalnızca başlangıç tarihi bugün ya da geçmişte olan kayda verilir; önce kaydedin, kişi gelince kişi sayfasından isteyin".to_string();
                return render_form(&state, form, msg, false, None).await;
            }
            Err(e) => return internal("tarih kontrolü", e),
        }
    }
    let keys = crate::national_id::Keys {
        aead: &state.aead_key,
        blind_index: &state.blind_index_key,
    };
    let requested_by = issue.then_some(op.username.as_str());
    let created = identity::create(&state.pool, &keys, &state.time_zone, &new, requested_by).await;
    let (id, first_password) = match created {
        Ok(created) => created,
        Err(identity::CreateError::DuplicateNationalId) => {
            let msg = "Bu kimlik numarası zaten kayıtlı".to_string();
            return render_form(&state, form, msg, false, None).await;
        }
        Err(identity::CreateError::Db(e)) => return internal("kimlik kaydedilemedi", e),
    };
    finish_create(&state, &op, &new, id, first_password).await
}

// Denetim satiri ve yonlendirme: ilk parola istendiyse teslim sayfasi, yoksa kisi sayfasi.
async fn finish_create(
    state: &AppState,
    op: &Operator,
    new: &identity::NewIdentity,
    id: i64,
    first_password: Option<i64>,
) -> Response {
    let detail = serde_json::json!({
        "employee_number": new.employee_number,
        "department_id": new.department_id,
        "primary_role_id": new.primary_role_id,
        "employment_type": new.employment_type,
        "start_date": new.start_date,
        "end_date": new.end_date,
        "national_id_set": new.national_id.is_some(),
        "first_password_id": first_password,
    });
    audit_operator(state, op, crate::audit::IDENTITY_CREATED, Some(id), detail).await;
    match first_password {
        Some(fp) => Redirect::to(&format!("/identities/{id}/first-password/{fp}")).into_response(),
        None => Redirect::to(&format!("/identities/{id}")).into_response(),
    }
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
            can_edit_names: allowed(&op, REGISTER_AUTHORITIES),
            can_first_password: allowed(&op, crate::first_password::AUTHORITIES),
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

        // ADR-056 tek adim: bugun/gecmis baslangic → teslim sayfasi; gelecek → form hatasi.
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                "/identities",
                &format!("{base}&confirm_duplicate=1&issue_first_password=1"),
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let fp_location = r.headers()["location"].to_str().unwrap().to_string();
        assert!(fp_location.contains("/first-password/"), "{fp_location}");
        let future = base.replace("start_date=2026-10-01", "start_date=2099-01-01");
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                "/identities",
                &format!("{future}&confirm_duplicate=1&issue_first_password=1"),
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_string(r)
            .await
            .contains("başlangıç tarihi bugün ya da geçmişte"));

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

        // ADR-022/042 mudahale: ad istegi + "siradaki adi ver" → bayrak + tekrar dene.
        sqlx::query("UPDATE jobs SET retry_requested = FALSE WHERE id = $1")
            .bind(job_id)
            .execute(&pool)
            .await
            .unwrap();
        let names = format!("/identities/{id}/names");
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &names,
                "requested_username=Ozel.Ad&name_conflict_override=1",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let (requested, over, retry): (Option<String>, bool, bool) = sqlx::query_as(
            "SELECT i.requested_username, i.name_conflict_override, j.retry_requested \
             FROM identities i JOIN jobs j ON j.identity_id = i.id WHERE i.id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (requested.as_deref(), over, retry),
            (Some("ozel.ad"), true, true)
        );
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            body.contains("Kullanıcı adı müdahalesi") && body.contains("ozel.ad"),
            "{body}"
        );
        let r = app
            .clone()
            .oneshot(request("POST", &names, "requested_username=a+b", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        sqlx::query("UPDATE identities SET username = 'ozel.ad' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let r = app
            .clone()
            .oneshot(request("POST", &names, "requested_username=baska", &hr))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::CONFLICT,
            "ad oluştuktan sonra değişmez"
        );

        // Gorev degisikligi (ADR-083): duzenleme formu dolu gelir, kayit alanlari gunceller,
        // is acar; ek rol ekleme/kaldirma, gecmis tarih reddi.
        let edit = format!("/identities/{id}/edit");
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &edit, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            body.contains("Kimliği düzenle") && body.contains("value=\"Ayşe\""),
            "{body}"
        );
        sqlx::query("UPDATE jobs SET status = 'succeeded'")
            .execute(&pool)
            .await
            .unwrap();
        // Kadrolu disina gecis bitis tarihi ister (DB CHECK); operator dilinde hata.
        let contract_body = format!(
            "given_name=Ay%C5%9Fe&surname=Demir&employee_number=S-9&department_id={dept}\
             &primary_role_id={role}&employment_type=contract&start_date=2026-10-01&end_date=2027-01-01"
        );
        let r = app
            .clone()
            .oneshot(request("POST", &edit, &contract_body, &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_string(r).await.contains("önce bitiş tarihi"));
        let edit_body = format!(
            "given_name=Ay%C5%9Fe&surname=Demir&employee_number=S-9&department_id={dept}\
             &primary_role_id={role}&employment_type=permanent&start_date=2026-10-01"
        );
        let r = app
            .clone()
            .oneshot(request("POST", &edit, &edit_body, &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let (surname, employee_number, employment_type): (String, Option<String>, String) =
            sqlx::query_as(
                "SELECT surname, employee_number, employment_type FROM identities WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            (
                surname.as_str(),
                employee_number.as_deref(),
                employment_type.as_str()
            ),
            ("Demir", Some("S-9"), "permanent")
        );
        let open_jobs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND status = 'queued'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_jobs, 2, "görev değişikliği her hedefe iş açar");
        let additional: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name) VALUES ('additional', 'Nöbet') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let roles = format!("/identities/{id}/roles");
        let past = format!("role_id={additional}&ends_on=2020-01-01");
        let r = app
            .clone()
            .oneshot(request("POST", &roles, &past, &hr))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::BAD_REQUEST,
            "geçmiş tarihli ek rol (ADR-020)"
        );
        let future = format!("role_id={additional}&ends_on=2099-12-31");
        let r = app
            .clone()
            .oneshot(request("POST", &roles, &future, &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let primary_as_additional = format!("role_id={role}");
        let r = app
            .clone()
            .oneshot(request("POST", &roles, &primary_as_additional, &hr))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::BAD_REQUEST,
            "birincil rol ek rol olarak atanamaz"
        );
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            body.contains("Nöbet") && body.contains("2099-12-31"),
            "{body}"
        );
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("{roles}/{additional}/delete"),
                "",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM identity_additional_roles WHERE identity_id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);

        // ADR-084 yasam dongusu rotalari: acil gerekcesiz 400, aski → sayfada donus gunu,
        // kaldirma, planli ayrilis → denetim satiri.
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{id}/emergency"),
                "reason=",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{id}/suspension"),
                "suspension_start=2030-01-05&suspension_end=2030-01-15",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("2030-01-16 00:00"), "{body}");
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{id}/suspension/lift"),
                "",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{id}/departure"),
                "end_date=2030-06-30",
                &auditor,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{id}/departure"),
                "end_date=2030-06-30",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let departures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE identity_id = $1 AND event_type = $2",
        )
        .bind(id)
        .bind(crate::audit::IDENTITY_DEPARTURE_SET)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(departures, 1);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-018/087: gozlem baglantisinda motorun yazdigi fark ekranda gorunur, "Yonetime al"
    // yalnizca yetkilide ve yalnizca istegi yazar; ayrilis yonetime almayi da icerir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn observed_account_shows_diff_and_is_taken_under_management() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [id, other] = crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let auditor = operator_cookie(&pool, &["auditor"]).await;
        let ad: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let observe = |identity: i64| {
            sqlx::query(
                "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
                 VALUES ($1, $2, $3, 'adopted', 'observed')",
            )
            .bind(identity)
            .bind(ad)
            .bind(format!("guid-{identity}"))
            .execute(&pool)
        };
        observe(id).await.unwrap();
        observe(other).await.unwrap();
        // motorun gozlem isinde yazdigi fark
        sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority, status, result, finished_at) \
             VALUES ($1, $2, 1, 'succeeded', 'gözlem modunda, yönetime alınırsa: 2 grup eklendi, OU taşındı', now())",
        )
        .bind(id)
        .bind(ad)
        .execute(&pool)
        .await
        .unwrap();

        // Fark her operatorde gorunur; dugme yalnizca yetkilide.
        let page = format!("/identities/{id}");
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &auditor))
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("yönetime alınırsa: 2 grup eklendi"), "{body}");
        assert!(!body.contains("Yönetime al"));
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("Yönetime al"), "{body}");

        let manage = format!("/identities/{id}/accounts/{ad}/manage");
        let r = app
            .clone()
            .oneshot(request("POST", &manage, "", &auditor))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let r = app
            .clone()
            .oneshot(request("POST", &manage, "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let requested: Vec<(i64, bool, String)> = sqlx::query_as(
            "SELECT identity_id, manage_requested_at IS NOT NULL, mode FROM account_links \
             ORDER BY identity_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            requested,
            vec![
                (id, true, "observed".to_string()),
                (other, false, "observed".to_string())
            ],
            "yalnizca istek yazilir, modu worker cevirir"
        );
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND status <> 'succeeded'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1, "istek bir iş açar");
        let body = body_string(
            app.clone()
                .oneshot(request("GET", &page, "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(body.contains("yönetime alma istendi"), "{body}");

        // worker modu cevirdikten sonra ikinci istek anlamsiz: 400
        sqlx::query("UPDATE account_links SET mode = 'managed' WHERE identity_id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let r = app
            .clone()
            .oneshot(request("POST", &manage, "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);

        // ADR-018: ayrilis gozlem modundaki kimlikte yonetime almayi da icerir
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{other}/departure"),
                "end_date=2030-06-30",
                &hr,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let requested: bool = sqlx::query_scalar(
            "SELECT manage_requested_at IS NOT NULL FROM account_links WHERE identity_id = $1",
        )
        .bind(other)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(requested, "ayrılış kaydedildi ama hiçbir şey olmadı durumu");
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE event_type = $1 AND actor_username = 'ik.operatoru'",
        )
        .bind(crate::audit::ACCOUNT_MANAGE_REQUESTED)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 2, "düğme + ayrılış");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
