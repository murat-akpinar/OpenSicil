// --- START FEATURE: identity-registration ---
// Kimlik kayit formu, kisi sayfasi ve "tekrar dene" rotalari (F-12, ADR-078).
// Yetki: kayit ve tekrar dene hr/admin, sayfa her operator; yardim masasi yalnizca
// ilk parola ister (ADR-019/085, first_password.rs).

use askama::Template;
use axum::extract::{Form, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::cookie::{get_cookie, OPERATOR_SESSION_COOKIE_NAME};
use crate::i18n::Lang;
use crate::identity::{self, FormOptions, IdentityForm, PersonPage};
use crate::operator_session::Operator;
use crate::shell::{Shell, Tabs};
use crate::web::{render, AppState};

const REGISTER_AUTHORITIES: &[&str] = &["hr", "admin"];
pub(crate) const RETRY_AUTHORITIES: &[&str] = &["hr", "admin"];

pub struct OperatorSession(pub Operator);

pub struct NoOperatorSession;

impl IntoResponse for NoOperatorSession {
    fn into_response(self) -> Response {
        Redirect::to("/login").into_response()
    }
}

/// Yetkisi olmayan oturumu da kabul eder; yalnizca dil secici kullanir.
pub struct AnySession(pub Operator);

impl FromRequestParts<AppState> for AnySession {
    type Rejection = NoOperatorSession;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token =
            get_cookie(&parts.headers, OPERATOR_SESSION_COOKIE_NAME).ok_or(NoOperatorSession)?;
        match crate::operator_session::validate_session(&state.pool, &token).await {
            Ok(Some(operator)) => Ok(AnySession(operator)),
            _ => Err(NoOperatorSession),
        }
    }
}

impl FromRequestParts<AppState> for OperatorSession {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AnySession(operator) = AnySession::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;
        // ADR-095 madde 2: yonetim grubunda olmayan kullanici girer ama hicbir
        // ekrani goremez. Handler basina degil burada: okuma ekranlari tek tek
        // unutuluyordu (guvenlik denetimi OS-01).
        if !has_any_authority(&operator) {
            return Err(forbidden(operator.lang));
        }
        Ok(OperatorSession(operator))
    }
}

pub(crate) fn has_any_authority(operator: &Operator) -> bool {
    allowed(operator, &crate::shell::AUTHORITY_ORDER)
}

pub(crate) fn allowed(operator: &Operator, any_of: &[&str]) -> bool {
    operator
        .authorities
        .iter()
        .any(|a| any_of.contains(&a.as_str()))
}

pub(crate) fn forbidden(lang: Lang) -> Response {
    crate::errors::page(lang, StatusCode::FORBIDDEN)
}

pub(crate) fn internal(what: &str, e: impl std::fmt::Display) -> Response {
    log_error!("web: {what}: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/identities/new", get(new_form))
        .route("/identities", get(list_page).post(create))
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

/// Personel listesinin sayfa boyu. Ust bardaki arama kutusu hizli bir onizleme;
/// tam liste burada ve sayfali (N-03: 20.000 kimlik tek sayfaya basilamaz).
pub(crate) const PAGE_SIZE: i64 = 50;

/// Personel calisma alaninin sekmeleri (ADR-134 madde 2–3); rozetler mudahale
/// ve silme sekmelerinde.
const PERSONNEL_TABS: [(&str, &str); 6] = [
    ("/identities", "nav.identity_list"),
    ("/imports", "nav.imports"),
    ("/upcoming", "nav.upcoming"),
    ("/interventions", "nav.interventions"),
    ("/deletions", "nav.deletions"),
    ("/used-names", "nav.used_names"),
];

pub(crate) async fn personnel_tabs(
    pool: &sqlx::PgPool,
    lang: Lang,
    current: &str,
) -> Result<Tabs, sqlx::Error> {
    let (interventions, deletions) = crate::deletions::pending_counts(pool).await?;
    let mut tabs = Tabs::new(lang, "nav.identities", &PERSONNEL_TABS, current);
    tabs.items[3].count = interventions;
    tabs.items[4].count = deletions;
    Ok(tabs)
}

/// Sorgu dizesindeki sayisal parametre. Formdaki "Tumu" secenegi `role=` diye
/// bos gelir ve serde `Option<i64>`u 400 ile reddeder; ayristirilamayan deger
/// "filtre yok" sayilir. Elle yazilan `?days=abc` de sayfayi kirmaz.
pub(crate) fn empty_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw.and_then(|value| value.trim().parse().ok()))
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    #[serde(default, deserialize_with = "empty_as_none")]
    offset: Option<i64>,
    /// `unassigned=1`: yalnizca rolu yer tutucu olanlar (ADR-103 madde 4)
    unassigned: Option<String>,
    /// ADR-117: panelin sayac kartindan gelen pencere (`joined`/`departed`/`changed`)
    window: Option<String>,
    /// Pencerenin gun sayisi; panelinkiyle ayni izinli liste (`dashboard::window`)
    #[serde(default, deserialize_with = "empty_as_none")]
    days: Option<i32>,
    /// ADR-117 E: arac cubugunun filtreleri ve siralama
    #[serde(default, deserialize_with = "empty_as_none")]
    department: Option<i64>,
    #[serde(default, deserialize_with = "empty_as_none")]
    role: Option<i64>,
    sort: Option<String>,
    /// `dir=desc` azalan; baska her deger artan
    dir: Option<String>,
}

/// Arac cubugunun acilir listesindeki bir secenek.
pub struct Choice {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

/// Siralanabilir kolon basligi: tiklaninca yonu cevirir, acik filtreleri korur.
pub struct SortHead {
    pub key: &'static str,
    /// Basligin adresi (mevcut sorgu + bu kolon, yon cevrilmis)
    pub href: String,
    /// `asc` | `desc` | "" (bu kolona gore siralanmiyor)
    pub active: &'static str,
}

#[derive(Template)]
#[template(path = "identities.html")]
struct IdentitiesTemplate {
    lang: Lang,
    shell: Shell,
    tabs: Tabs,
    rows: Vec<identity::Listed>,
    q: String,
    total: i64,
    /// "1–50 / 312": aralik burada kurulur, sablon aritmetik yapmaz
    range: String,
    prev_offset: Option<i64>,
    next_offset: Option<i64>,
    can_register: bool,
    /// Liste bosken "AD'de sahiplenilmeyi bekleyen hesap var" yonlendirmesi
    /// (ADR-103 madde 3); dolu listede sorgu hic calismaz
    unadopted: Vec<crate::reconcile::Unadopted>,
    /// ADR-103 madde 4: filtre acik mi ve (kapaliyken) rolu atanmamis kac kisi var
    unassigned_only: bool,
    role_unassigned: i64,
    /// ADR-117: acik pencere filtresinin serit metni; bos = filtre yok
    window_note: String,
    /// ADR-117 E: arac cubugunun filtreleri ve siralanabilir basliklar
    departments: Vec<Choice>,
    roles: Vec<Choice>,
    heads: Vec<SortHead>,
    /// Filtre acik mi (serit "temizle" baglantisi icin)
    filtered: bool,
}

/// Pencere seridinin metin anahtari. `tn` derleme zamani sabit anahtar istiyor,
/// bu yuzden ad buradan esleniyor; `identity::WINDOW_FILTERS` ile ayni uc deger.
fn window_text_key(window: &str) -> &'static str {
    match window {
        "joined" => "identities.window_joined",
        "departed" => "identities.window_departed",
        _ => "identities.window_changed",
    }
}

/// Personel listesi: okuma her operatorde (auditor dahil), "Yeni kimlik"
/// dugmesi yalnizca kayit yetkisinde.
async fn list_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let query = q.q.unwrap_or_default();
    let offset = q.offset.unwrap_or(0).max(0);
    let unassigned_only = q.unassigned.as_deref() == Some("1");
    // Izinli listeler: pencere adi `identity::window_filter`, gun sayisi panelin
    // `dashboard::window`i. Sorguya keyfi metin ya da sayi girmez.
    let window = identity::window_filter(q.window.as_deref());
    let window_days = crate::dashboard::window(q.days);
    let descending = q.dir.as_deref() == Some("desc");
    // Sablon `listing`i gormuyor (icinde `query`ye odunc var); acik filtre
    // bilgisi burada bir bool'a aliniyor
    let filtered = q.department.is_some_and(|id| id > 0) || q.role.is_some_and(|id| id > 0);
    let listing = identity::Listing {
        query: &query,
        unassigned_only,
        window,
        window_days,
        department: q.department.filter(|id| *id > 0),
        role: q.role.filter(|id| *id > 0),
        order: identity::sort_clause(q.sort.as_deref(), descending),
        offset,
        limit: PAGE_SIZE,
    };
    let role_unassigned = match identity::unassigned_role_count(&state.pool).await {
        Ok(n) => n,
        Err(e) => return internal("rolü atanmamış sayısı okunamadı", e),
    };
    let (rows, total) = match identity::page(&state.pool, &time_zone, &listing).await {
        Ok(listed) => listed,
        Err(e) => return internal("personel listesi okunamadı", e),
    };
    // Yalnizca bos listede sorulur: dolu listede yonlendirme basilmiyor
    let unadopted = match rows.is_empty() && query.is_empty() {
        true => match crate::reconcile::unadopted(&state.pool).await {
            Ok(rows) => rows,
            Err(e) => return internal("sahiplenme bekleyen hesaplar okunamadı", e),
        },
        false => Vec::new(),
    };
    // Arac cubugunun secenekleri; okuma her operatorde, yazma yetkisi gerekmez
    let picked = match tokio::try_join!(
        crate::org::list_departments(&state.pool),
        crate::org::list_roles(&state.pool),
        personnel_tabs(&state.pool, op.lang, "/identities"),
    ) {
        Ok(lists) => lists,
        Err(e) => return internal("filtre listeleri okunamadı", e),
    };
    let departments = picked
        .0
        .into_iter()
        .map(|d| Choice {
            selected: listing.department == Some(d.id),
            label: format!("{}{}", d.indent, d.name),
            id: d.id,
        })
        .collect();
    // Yalnizca birincil roller: filtre `primary_role_id`ye bakiyor
    let roles = picked
        .1
        .into_iter()
        .filter(|r| r.kind == "primary")
        .map(|r| Choice {
            selected: listing.role == Some(r.id),
            label: r.name,
            id: r.id,
        })
        .collect();
    let heads = sort_heads(&q_string(&listing), q.sort.as_deref(), descending);
    let (range, prev_offset, next_offset) = pagination(op.lang, offset, rows.len() as i64, total);
    render(&IdentitiesTemplate {
        lang: op.lang,
        can_register: allowed(&op, REGISTER_AUTHORITIES),
        shell: Shell::of(&op),
        q: query,
        total,
        range,
        prev_offset,
        next_offset,
        unadopted,
        unassigned_only,
        role_unassigned,
        departments,
        roles,
        heads,
        filtered,
        tabs: picked.2,
        window_note: match window {
            Some(w) => op.lang.tn(
                window_text_key(w),
                &[&window_days.to_string(), &total.to_string()],
            ),
            None => String::new(),
        },
        rows,
    })
}

/// Siralama baglantilarinin tasidigi mevcut sorgu. Kolon basligina tiklamak
/// acik filtreyi silmemeli: arama, "rolu atanmamis" ve panelin sayac kartindan
/// gelen pencere adres satirinda kalir. Departman/rol tasinmaz — ikisi de
/// aractaki `<select>`ten geliyor ve form gonderilince yeniden yaziliyor.
fn q_string(listing: &identity::Listing<'_>) -> String {
    let mut out = String::new();
    if !listing.query.is_empty() {
        out.push_str("&q=");
        out.push_str(&urlencode(listing.query));
    }
    if listing.unassigned_only {
        out.push_str("&unassigned=1");
    }
    if let Some(window) = listing.window {
        out.push_str(&format!("&window={window}&days={}", listing.window_days));
    }
    out
}

/// Sorgu dizesine giden deger icin yuzde kacisi; sablonda filtre yok, burada.
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Kolon basliklarinin adresi ve o anki siralama yonu. Ayni kolona ikinci kez
/// tiklamak yonu cevirir; baska kolona gecmek artanla baslar.
fn sort_heads(carry: &str, sort: Option<&str>, descending: bool) -> Vec<SortHead> {
    identity::SORTS
        .iter()
        .map(|(key, ..)| {
            let active = match (sort.unwrap_or(identity::SORTS[0].0) == *key, descending) {
                (false, _) => "",
                (true, false) => "asc",
                (true, true) => "desc",
            };
            SortHead {
                key,
                href: format!(
                    "/identities?sort={key}&dir={}{carry}",
                    match active {
                        "asc" => "desc",
                        _ => "asc",
                    }
                ),
                active,
            }
        })
        .collect()
}

/// "1–50 / 312" metni ve onceki/sonraki sayfa ofsetleri; sablon aritmetik yapmaz.
pub(crate) fn pagination(
    lang: Lang,
    offset: i64,
    shown: i64,
    total: i64,
) -> (String, Option<i64>, Option<i64>) {
    let from = if shown == 0 { 0 } else { offset + 1 };
    let range = lang.tn(
        "identities.range",
        &[
            &from.to_string(),
            &(offset + shown).to_string(),
            &total.to_string(),
        ],
    );
    let prev = (offset > 0).then(|| (offset - PAGE_SIZE).max(0));
    let next = (offset + shown < total).then_some(offset + PAGE_SIZE);
    (range, prev, next)
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
        return forbidden(op.lang);
    }
    use identity::ManageOutcome;
    match identity::request_management(&state.pool, id, target_id).await {
        Ok(ManageOutcome::Requested) => {
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
        Ok(ManageOutcome::NoObservedAccount) => bad(op.lang.t("err.observed_account_missing")),
        // ADR-103 madde 5: gerekce operatorun dilinde, baglanti gozlemde kalir
        Ok(ManageOutcome::RoleUndefined) => bad(op.lang.t("err.role_undefined_manage")),
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
    departure_note: String,
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
            Err(e) => log_error!("web: yönetime alma istenemedi (kimlik {id}): {e}"),
        }
    }
    if let Err(e) = identity::enqueue_all_targets(&state.pool, id, priority).await {
        log_error!("web: iş açılamadı (kimlik {id}): {e}");
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

async fn departure(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if let Some(refused) = refuse_lifecycle(&state, &op, id).await {
        return refused;
    }
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let handover = opt(&form.handover_manager_id).and_then(|h| h.parse::<i64>().ok());
    let departure = identity::Departure {
        end_date: form.end_date.trim(),
        handover,
        note: &form.departure_note,
    };
    let outcome = identity::set_departure(&state.pool, &time_zone, id, &departure).await;
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
        Ok(identity::LifecycleChange::Rejected(key)) => bad(op.lang.t(key)),
        Err(e) => internal("ayrılış yazılamadı", e),
    }
}

async fn emergency(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if let Some(refused) = refuse_lifecycle(&state, &op, id).await {
        return refused;
    }
    let Some(reason) = opt(&form.reason) else {
        return bad(op.lang.t("err.emergency_reason_required"));
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
        Ok(false) => bad(op.lang.t("err.identity_deleted_or_self_handover")),
        Err(e) => internal("acil ayrılış yazılamadı", e),
    }
}

async fn revert(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if let Some(refused) = refuse_lifecycle(&state, &op, id).await {
        return refused;
    }
    let return_day = opt(&form.return_day);
    if return_day.is_some_and(|d| crate::desired_state::Date::from_iso(d).is_none()) {
        return bad(op.lang.t("err.return_day_format"));
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
        Ok(false) => bad(op.lang.t("err.revert_not_possible")),
        Err(e) => internal("geri alma yazılamadı", e),
    }
}

async fn cancel(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(refused) = refuse_lifecycle(&state, &op, id).await {
        return refused;
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
        Ok(false) => bad(op.lang.t("err.cancel_not_possible")),
        Err(e) => internal("iptal yazılamadı", e),
    }
}

async fn suspend(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<LifecycleForm>,
) -> Response {
    if let Some(refused) = refuse_lifecycle(&state, &op, id).await {
        return refused;
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
        Ok(false) => bad(op.lang.t("err.suspension_dates")),
        Err(e) => internal("askı yazılamadı", e),
    }
}

async fn lift(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if let Some(refused) = refuse_lifecycle(&state, &op, id).await {
        return refused;
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
        Ok(false) => bad(op.lang.t("err.no_suspension")),
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
        return forbidden(op.lang);
    }
    match identity::load_form(&state.pool, id).await {
        Ok(Some(form)) => render_form(&state, &op, form, String::new(), FormMode::Edit(id)).await,
        Ok(None) => (StatusCode::NOT_FOUND, op.lang.t("err.identity_not_found")).into_response(),
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
        return forbidden(op.lang);
    }
    let new = match identity::validate(&form) {
        Ok(n) => n,
        Err(key) => return form_error(&state, &op, form, key, FormMode::Edit(id)).await,
    };
    if let Some(refused) = refuse_own_role_change(
        &state,
        &op,
        id,
        Some((new.primary_role_id, new.department_id)),
    )
    .await
    {
        return refused;
    }
    match identity::placeholder_refused(&state.pool, new.primary_role_id, Some(id)).await {
        Ok(false) => {}
        Ok(true) => {
            let key = "err.role_undefined_managed";
            return form_error(&state, &op, form, key, FormMode::Edit(id)).await;
        }
        Err(e) => return internal("rol kontrolü", e),
    }
    if let Err(e) = identity::update_mover(&state.pool, id, &new).await {
        let db = e.as_database_error();
        if db.is_some_and(|d| d.is_unique_violation()) {
            let key = "err.duplicate_employee_number";
            return form_error(&state, &op, form, key, FormMode::Edit(id)).await;
        }
        // docs/03: kadrolu disinda bitis zorunlu; bitis ayrilis ekranindan girilir (3c)
        if db.is_some_and(|d| d.is_check_violation()) {
            let key = "err.non_permanent_needs_end";
            return form_error(&state, &op, form, key, FormMode::Edit(id)).await;
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

/// ADR-005: yasam dongusu yetkisi; operator kendi ayrilisini ve askisini
/// isleyemez, uzatamaz, geri alamaz (guvenlik denetimi OS-03). operator_guard
/// yalnizca tarih gectikten sonra reddeder. None: devam.
async fn refuse_lifecycle(state: &AppState, op: &Operator, id: i64) -> Option<Response> {
    if !allowed(op, REGISTER_AUTHORITIES) {
        return Some(forbidden(op.lang));
    }
    match crate::operator_guard::is_own_record(&state.pool, id, &op.username).await {
        Ok(false) => None,
        Ok(true) => Some(forbidden(op.lang)),
        Err(e) => Some(internal("kimlik sahibi okunamadı", e)),
    }
}

/// docs/07: kendi kaydinda rol ya da departman degisikligi 403; okunamazsa 500. None: devam.
async fn refuse_own_role_change(
    state: &AppState,
    op: &Operator,
    id: i64,
    edit: Option<(i64, i64)>,
) -> Option<Response> {
    match identity::own_role_change(&state.pool, id, &op.username, edit).await {
        Ok(false) => None,
        Ok(true) => Some(forbidden(op.lang)),
        Err(e) => Some(internal("rol değişikliği sahibi okunamadı", e)),
    }
}

async fn enqueue_single(state: &AppState, id: i64) {
    if let Err(e) =
        identity::enqueue_all_targets(&state.pool, id, crate::jobs::Priority::Single).await
    {
        log_error!("web: iş açılamadı (kimlik {id}): {e}");
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
        return forbidden(op.lang);
    }
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    if let Some(refused) = refuse_own_role_change(&state, &op, id, None).await {
        return refused;
    }
    let Ok(role_id) = form.role_id.trim().parse::<i64>() else {
        return (
            StatusCode::BAD_REQUEST,
            op.lang.t("err.additional_role_required"),
        )
            .into_response();
    };
    let ends_on = form.ends_on.trim();
    let ends_on = (!ends_on.is_empty()).then_some(ends_on);
    if ends_on.is_some_and(|d| crate::desired_state::Date::from_iso(d).is_none()) {
        return (StatusCode::BAD_REQUEST, op.lang.t("err.end_date_format")).into_response();
    }
    match identity::assign_role(&state.pool, &time_zone, id, role_id, ends_on).await {
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
        Ok(false) => (StatusCode::BAD_REQUEST, op.lang.t("err.past_end_date_role")).into_response(),
        Err(e)
            if e.as_database_error()
                .is_some_and(|d| d.is_foreign_key_violation()) =>
        {
            (
                StatusCode::BAD_REQUEST,
                op.lang.t("err.only_additional_roles"),
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
        return forbidden(op.lang);
    }
    if let Some(refused) = refuse_own_role_change(&state, &op, id, None).await {
        return refused;
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
        return forbidden(op.lang);
    }
    let requested = match identity::valid_requested_username(&form.requested_username) {
        Ok(r) => r,
        Err(key) => return (StatusCode::BAD_REQUEST, op.lang.t(key)).into_response(),
    };
    let override_conflicts = form.name_conflict_override.is_some();
    match identity::request_names(&state.pool, id, requested.as_deref(), override_conflicts).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::CONFLICT,
                op.lang.t("err.username_already_created"),
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
        log_error!("web: tekrar dene yazılamadı (kimlik {id}): {e}");
    }
    Redirect::to(&format!("/identities/{id}")).into_response()
}

#[derive(Template)]
#[template(path = "identity_form.html")]
struct IdentityFormTemplate {
    shell: Shell,
    lang: Lang,
    form: IdentityForm,
    options: FormOptions,
    /// (calisma tipi anahtari, secili mi): secim sablonda degil burada hesaplanir
    employment_types: Vec<(&'static str, bool)>,
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
    shell: Shell,
    lang: Lang,
    page: PersonPage,
    can_retry: bool,
    can_edit_names: bool,
    can_first_password: bool,
}

async fn new_form(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let form = IdentityForm {
        national_id_country: "TR".to_string(),
        ..IdentityForm::default()
    };
    render_form(&state, &op, form, String::new(), FormMode::New).await
}

// Formun gorunumu: yeni kayit, mukerrer uyarisi ya da duzenleme (ADR-078/083).
#[derive(Clone, Copy)]
enum FormMode {
    New,
    Duplicate,
    Edit(i64),
}

/// Formu i18n anahtarindan cozulmus hata mesajiyla yeniden gosterir
async fn form_error(
    state: &AppState,
    op: &Operator,
    form: IdentityForm,
    key: &'static str,
    mode: FormMode,
) -> Response {
    let error = op.lang.t(key).to_string();
    render_form(state, op, form, error, mode).await
}

async fn render_form(
    state: &AppState,
    op: &Operator,
    form: IdentityForm,
    error: String,
    mode: FormMode,
) -> Response {
    let employment_types = identity::EMPLOYMENT_TYPES
        .iter()
        .map(|kind| (*kind, *kind == form.employment_type))
        .collect();
    let loaded = tokio::try_join!(
        identity::form_options(&state.pool),
        identity::placeholder_role_id(&state.pool)
    );
    match loaded {
        Ok((mut options, placeholder)) => {
            // Yeni kayit yer tutucuyu secemez; duzenlemede kalir (secili deger kaybolmasin)
            if !matches!(mode, FormMode::Edit(_)) {
                let placeholder = placeholder.map(|p| p.to_string());
                options
                    .roles
                    .retain(|c| Some(&c.id) != placeholder.as_ref());
            }
            render(&IdentityFormTemplate {
                lang: op.lang,
                shell: Shell::of(op),
                form,
                options,
                employment_types,
                error,
                duplicate_warning: matches!(mode, FormMode::Duplicate),
                editing: matches!(mode, FormMode::Edit(_)),
                action: match mode {
                    FormMode::Edit(id) => format!("/identities/{id}/edit"),
                    _ => "/identities".to_string(),
                },
                ownership_enabled: state.common().await.is_ok_and(|c| c.ownership_mode_enabled),
            })
        }
        Err(e) => internal("form seçenekleri okunamadı", e),
    }
}

async fn create(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<IdentityForm>,
) -> Response {
    if !allowed(&op, REGISTER_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let new = match identity::validate(&form) {
        Ok(n) => n,
        Err(key) => return form_error(&state, &op, form, key, FormMode::New).await,
    };
    match identity::placeholder_refused(&state.pool, new.primary_role_id, None).await {
        Ok(false) => {}
        Ok(true) => {
            let key = "err.role_undefined_managed";
            return form_error(&state, &op, form, key, FormMode::New).await;
        }
        Err(e) => return internal("rol kontrolü", e),
    }
    match duplicate_person(&state, &form, &new).await {
        Ok(true) => {
            return render_form(&state, &op, form, String::new(), FormMode::Duplicate).await
        }
        Ok(false) => {}
        Err(e) => return internal("mükerrer kişi kontrolü", e),
    }
    // ADR-056: tek adim yalnizca bugun ya da gecmiste baslayan kayda ve ilk parola yetkisiyle
    let issue = !form.issue_first_password.is_empty();
    if issue {
        if !allowed(&op, crate::first_password::AUTHORITIES) {
            return forbidden(op.lang);
        }
        match identity::starts_by_today(&state.pool, &new.start_date, &time_zone).await {
            Ok(true) => {}
            Ok(false) => {
                let key = "err.first_password_start_date";
                return form_error(&state, &op, form, key, FormMode::New).await;
            }
            Err(e) => return internal("tarih kontrolü", e),
        }
    }
    let keys = crate::national_id::Keys {
        aead: &state.aead_key,
        blind_index: &state.blind_index_key,
    };
    let requested_by = issue.then_some(op.username.as_str());
    let created = identity::create(&state.pool, &keys, &time_zone, &new, requested_by).await;
    let (id, first_password) = match created {
        Ok(created) => created,
        Err(identity::CreateError::DuplicateNationalId) => {
            let key = "err.duplicate_national_id";
            return form_error(&state, &op, form, key, FormMode::New).await;
        }
        Err(identity::CreateError::Db(e)) => return internal("kimlik kaydedilemedi", e),
    };
    finish_create(&state, &op, &new, id, first_password).await
}

/// ADR-078: kimlik no girilmediyse ad-soyad mukerrerligi uyarilir, operator onaylarsa gecilir
async fn duplicate_person(
    state: &AppState,
    form: &IdentityForm,
    new: &identity::NewIdentity,
) -> Result<bool, sqlx::Error> {
    if new.national_id.is_some() || form.confirm_duplicate.is_some() {
        return Ok(false);
    }
    identity::similar_name_exists(&state.pool, &new.given_name, &new.surname).await
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
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    match identity::load_page(&state.pool, &time_zone, &state.aead_key, id, op.lang).await {
        Ok(Some(page)) => render(&PersonTemplate {
            lang: op.lang,
            shell: Shell::of(&op),
            page,
            can_retry: allowed(&op, RETRY_AUTHORITIES),
            can_edit_names: allowed(&op, REGISTER_AUTHORITIES),
            can_first_password: allowed(&op, crate::first_password::AUTHORITIES),
        }),
        Ok(None) => (StatusCode::NOT_FOUND, op.lang.t("err.identity_not_found")).into_response(),
        Err(e) => internal("kişi sayfası okunamadı", e),
    }
}

async fn retry(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((id, job_id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, RETRY_AUTHORITIES) {
        return forbidden(op.lang);
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
        log_error!("web: denetim kaydı yazılamadı ({event_type}): {e}");
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
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(pool, &operator)
            .await
            .unwrap();
        format!("{OPERATOR_SESSION_COOKIE_NAME}={token}")
    }

    /// Araç çubuğundaki "Tümü" seçeneği `department=&role=` diye boş gelir.
    /// `Option<i64>` bunu "Failed to deserialize query string" ile 400'e
    /// çeviriyordu; artık filtre yok sayılır ve liste açılır.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn empty_filter_values_do_not_break_the_list() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let cookie = operator_cookie(&pool, &["hr"]).await;
        for uri in [
            "/identities?q=&department=&role=",
            "/identities?department=&role=&offset=&days=",
            // elle yazilan bozuk deger de sayfayi kirmaz
            "/identities?role=abc",
            "/upcoming?days=",
        ] {
            let response = app
                .clone()
                .oneshot(request("GET", uri, "", &cookie))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                axum::http::StatusCode::OK,
                "{uri} 200 dönmeli"
            );
        }
        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
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

    /// ADR-103 madde 4 ve 5: yer tutucu rol seed'li gelir ve tektir; rolu o olan
    /// kisi panelde ve listede sayilir, filtreyle listelenir, yonetime alinamaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_placeholder_role_is_counted_filtered_and_blocks_take_over() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let lang = crate::i18n::DEFAULT;
        let get = |path: &'static str| {
            let (app, hr) = (app.clone(), hr.clone());
            async move { body_string(app.oneshot(request("GET", path, "", &hr)).await.unwrap()).await }
        };

        // Seed: tek yer tutucu, birincil, yetki ogesi yok; ikincisi acilamaz
        let (role, name, kind): (i64, String, String) =
            sqlx::query_as("SELECT id, name, kind FROM roles WHERE placeholder")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((name.as_str(), kind.as_str()), ("Tanımsız", "primary"));
        let entitlements: i64 =
            sqlx::query_scalar("SELECT count(*) FROM role_entitlements WHERE role_id = $1")
                .bind(role)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(entitlements, 0);
        let second = sqlx::query(
            "INSERT INTO roles (kind, name, placeholder) VALUES ('primary', 'İkinci', true)",
        )
        .execute(&pool)
        .await;
        assert!(second.is_err(), "ikinci yer tutucu rol açılamaz");

        // Kimse yer tutucu rolde degil: ne panelde ne listede serit var
        assert!(!get("/").await.contains("/identities?unassigned=1"));
        assert!(!get("/identities")
            .await
            .contains("/identities?unassigned=1"));

        // Ali Kaya yer tutucu role gecer: sayac 1, serit iki ekranda, filtre yalnizca onu listeler
        let ali: i64 = sqlx::query_scalar(
            "UPDATE identities SET primary_role_id = $1 WHERE surname = 'Kaya' RETURNING id",
        )
        .bind(role)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(identity::unassigned_role_count(&pool).await.unwrap(), 1);
        let strip = lang.t1("dash.role_unassigned", 1);
        for path in ["/", "/identities"] {
            let body = get(path).await;
            assert!(
                body.contains(&strip) && body.contains("/identities?unassigned=1"),
                "{path}: {body}"
            );
        }
        let filtered = get("/identities?unassigned=1").await;
        assert!(
            filtered.contains("Ali Kaya") && !filtered.contains("Ayşe Yılmaz"),
            "{filtered}"
        );
        assert!(
            filtered.contains(lang.t("identities.unassigned_clear")),
            "filtreyi kaldır bağlantısı"
        );

        // Kapi: gozlem modundaki hesap yonetime alinamaz, istek yazilmaz, sayfa nedenini soyler
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             VALUES ($1, $2, 'guid-ali', 'adopted', 'observed')",
        )
        .bind(ali)
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        let manage = format!("/identities/{ali}/accounts/{target}/manage");
        let r = app
            .clone()
            .oneshot(request("POST", &manage, "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert!(body_string(r)
            .await
            .contains(lang.t("err.role_undefined_manage")));
        let requested: Option<bool> = sqlx::query_scalar(
            "SELECT manage_requested_at IS NOT NULL FROM account_links WHERE identity_id = $1",
        )
        .bind(ali)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(requested, Some(false), "istek yazılmadı, bağlantı gözlemde");
        let person = body_string(
            app.clone()
                .oneshot(request("GET", &format!("/identities/{ali}"), "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(person.contains(lang.t("person.role_undefined")), "{person}");
        assert!(!person.contains(&manage), "düğme yerine neden yazılır");

        // Rol atanınca kapı açılır
        sqlx::query(
            "UPDATE identities SET primary_role_id = (SELECT id FROM roles WHERE kind = 'primary' \
             AND NOT placeholder LIMIT 1) WHERE id = $1",
        )
        .bind(ali)
        .execute(&pool)
        .await
        .unwrap();
        let r = app
            .clone()
            .oneshot(request("POST", &manage, "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(identity::unassigned_role_count(&pool).await.unwrap(), 0);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// docs/07: operator kendi kaydinda rol degistiremez; baskasinin kaydinda ve
    /// rolu degistirmeyen duzenlemede engel yok.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn an_operator_cannot_change_roles_on_their_own_record() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [own, other] = crate::test_support::seed_two_identities(&pool).await;
        sqlx::query("UPDATE identities SET username = 'IK.Operatoru' WHERE id = $1")
            .bind(own)
            .execute(&pool)
            .await
            .unwrap();
        let (dept, role): (i64, i64) =
            sqlx::query_as("SELECT department_id, primary_role_id FROM identities WHERE id = $1")
                .bind(own)
                .fetch_one(&pool)
                .await
                .unwrap();
        let additional: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, slug) VALUES ('additional', 'Yönetici', 'yonetici') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let second: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, slug) VALUES ('primary', 'Başka', 'baska') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let post = |uri: String, body: String| {
            let (app, hr) = (app.clone(), hr.clone());
            async move {
                app.oneshot(request("POST", &uri, &body, &hr))
                    .await
                    .unwrap()
                    .status()
            }
        };
        let edit_in = |dept: i64, primary: i64| {
            format!(
                "given_name=Ay%C5%9Fe&surname=Y%C4%B1lmaz&department_id={dept}&primary_role_id={primary}\
                 &employment_type=permanent&start_date=2026-10-01"
            )
        };
        let edit = |primary: i64| edit_in(dept, primary);

        let assign = format!("role_id={additional}");
        assert_eq!(
            post(format!("/identities/{own}/roles"), assign.clone()).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post(format!("/identities/{own}/edit"), edit(second)).await,
            StatusCode::FORBIDDEN
        );
        let unchanged = post(format!("/identities/{own}/edit"), edit(role)).await;
        assert_eq!(
            unchanged,
            StatusCode::SEE_OTHER,
            "rolü değiştirmeyen düzenleme serbest"
        );
        // Guvenlik denetimi OS-04: departman da korunur; UPN bicimli ad da eslesir
        let other_dept: i64 = sqlx::query_scalar(
            "INSERT INTO departments (name, code, slug) VALUES ('Başka', 'BS', 'baska') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            post(format!("/identities/{own}/edit"), edit_in(other_dept, role)).await,
            StatusCode::FORBIDDEN,
            "kendi departmanı"
        );
        let upn =
            crate::test_support::operator_cookie(&pool, "ik.operatoru@corp.example", &["hr"]).await;
        let r = app
            .clone()
            .oneshot(request(
                "POST",
                &format!("/identities/{own}/roles"),
                &format!("role_id={additional}"),
                &upn,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "UPN biçimli oturum");
        // Baskasinin kaydinda engel yok; kendi kaydindaki ek rol de kaldirilamaz
        assert_eq!(
            post(format!("/identities/{other}/roles"), assign).await,
            StatusCode::SEE_OTHER
        );
        sqlx::query("INSERT INTO identity_additional_roles (identity_id, role_id) VALUES ($1, $2)")
            .bind(own)
            .bind(additional)
            .execute(&pool)
            .await
            .unwrap();
        let remove = format!("/identities/{own}/roles/{additional}/delete");
        assert_eq!(post(remove, String::new()).await, StatusCode::FORBIDDEN);
        let primary: i64 =
            sqlx::query_scalar("SELECT primary_role_id FROM identities WHERE id = $1")
                .bind(own)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(primary, role);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// Guvenlik denetimi OS-03 (ADR-005): planlanmis ayrilis ve aski henuz
    /// islemediginde operator_guard reddetmez; kendi kaydi kurali burada tutar.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn an_operator_cannot_touch_their_own_departure_or_suspension() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [own, other] = crate::test_support::seed_two_identities(&pool).await;
        sqlx::query(
            "UPDATE identities SET username = 'IK.Operatoru', end_at = now() + interval '5 days', \
             suspension_start = current_date + 3 WHERE id = $1",
        )
        .bind(own)
        .execute(&pool)
        .await
        .unwrap();
        let app =
            crate::server::build_router(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let post = |uri: String, body: &'static str| {
            let (app, hr) = (app.clone(), hr.clone());
            async move {
                app.oneshot(request("POST", &uri, body, &hr))
                    .await
                    .unwrap()
                    .status()
            }
        };

        for (path, body) in [
            ("revert", ""),
            ("departure", "end_date=2099-12-31"),
            ("emergency", "reason=x"),
            ("cancel", ""),
            ("suspension", "suspension_start=2099-01-01"),
            ("suspension/lift", ""),
        ] {
            let status = post(format!("/identities/{own}/{path}"), body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        }
        let (kept_end, kept_suspension): (bool, bool) = sqlx::query_as(
            "SELECT end_at < now() + interval '6 days', suspension_start IS NOT NULL \
             FROM identities WHERE id = $1",
        )
        .bind(own)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(kept_end && kept_suspension, "takvim yerinde");
        let status = post(
            format!("/identities/{other}/departure"),
            "end_date=2099-12-31",
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "başkasının ayrılışı serbest");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// Degismez kural: `mode = 'managed'` baglantinin kimliginde yer tutucu rol olmaz.
    /// Neden: kapi kalkarsa gece dolumu yonetilen hesabin rolunu degistirir ve is
    /// acilmadigi icin hesap eski OU'da kalir. Kapi bilerek kaldirilirsa bu test
    /// kirmizi olur; o zaman is acma karari yazilir (todo: "Değişmez kural test edilir").
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn a_managed_link_never_carries_the_placeholder_role() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [ayse, ali] = crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let placeholder = identity::placeholder_role_id(&pool).await.unwrap().unwrap();
        let dept: i64 = sqlx::query_scalar("SELECT department_id FROM identities WHERE id = $1")
            .bind(ali)
            .fetch_one(&pool)
            .await
            .unwrap();
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let role_of = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT primary_role_id FROM identities WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let body = |surname: &str| {
            format!(
                "given_name=Veli&surname={surname}&department_id={dept}&primary_role_id={placeholder}\
                 &employment_type=permanent&start_date=2026-10-01&national_id_country=TR"
            )
        };
        let post = |uri: String, body: String| {
            let (app, hr) = (app.clone(), hr.clone());
            async move {
                app.oneshot(request("POST", &uri, &body, &hr))
                    .await
                    .unwrap()
            }
        };

        // Kayit: yer tutucu formda yok, elle gonderilse de kimlik acilmaz
        let form = body_string(
            app.clone()
                .oneshot(request("GET", "/identities/new", "", &hr))
                .await
                .unwrap(),
        )
        .await;
        assert!(!form.contains(">Tanımsız</option>"), "{form}");
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        let r = post("/identities".into(), body("Can")).await;
        assert!(body_string(r).await.contains("Tanımsız"));
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after, "yer tutucu rolle kayıt açılmaz");

        // Duzenleme: yonetilen hesabi olan kisi Tanimsiz'a cekilemez
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             VALUES ($1, $2, 'guid-ali', 'provisioned', 'managed'), ($3, $2, 'guid-ayse', 'adopted', 'observed')",
        )
        .bind(ali)
        .bind(target)
        .bind(ayse)
        .execute(&pool)
        .await
        .unwrap();
        let ali_role = role_of(ali).await;
        let r = post(format!("/identities/{ali}/edit"), body("Kaya")).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            role_of(ali).await,
            ali_role,
            "yönetilen kişinin rolü değişmedi"
        );
        // Gozlemdeki kisi Tanimsiz'da kalabilir (toplu sahiplenme varsayilani, ADR-103)
        let r = post(format!("/identities/{ayse}/edit"), body("Y%C4%B1lmaz")).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(role_of(ayse).await, placeholder);

        // Tek ve toplu "Yonetime al": yer tutucu rollu gozlem baglantisi yonetime gecmez
        assert!(matches!(
            identity::request_management(&pool, ayse, target)
                .await
                .unwrap(),
            identity::ManageOutcome::RoleUndefined
        ));
        let bulk = crate::bulk_manage::request(&pool, target, &[ayse])
            .await
            .unwrap();
        assert!(bulk.requested.is_empty());
        let managed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM account_links l JOIN identities i ON i.id = l.identity_id \
             JOIN roles r ON r.id = i.primary_role_id \
             WHERE (l.mode = 'managed' OR l.manage_requested_at IS NOT NULL) AND r.placeholder",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(managed, 0, "değişmez kural");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_personnel_list_renders_searches_and_hides_the_register_button() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let hr = operator_cookie(&pool, &["hr"]).await;
        let auditor = operator_cookie(&pool, &["auditor"]).await;

        // Oturumsuz istek girise doner
        let r = app
            .clone()
            .oneshot(request("GET", "/identities", "", ""))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER);

        // hr listeyi gorur ve kayit dugmesi acik
        let r = app
            .clone()
            .oneshot(request("GET", "/identities", "", &hr))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_string(r).await;
        assert!(
            body.contains("Ayşe Yılmaz") && body.contains("Ali Kaya"),
            "{body}"
        );
        assert!(body.contains("Test Birimi"), "departman kolonu: {body}");
        assert!(body.contains("/identities/new"), "kayıt düğmesi: {body}");

        // auditor okur ama kayit dugmesi yok (yetki kontrolu servis katmaninda)
        let r = app
            .clone()
            .oneshot(request("GET", "/identities", "", &auditor))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_string(r).await;
        assert!(body.contains("Ayşe Yılmaz"), "{body}");
        assert!(
            !body.contains("/identities/new"),
            "auditor düğmesiz: {body}"
        );

        // Arama hem listeyi daraltir hem kutuda kalir
        let r = app
            .clone()
            .oneshot(request("GET", "/identities?q=kaya", "", &hr))
            .await
            .unwrap();
        let body = body_string(r).await;
        assert!(
            body.contains("Ali Kaya") && !body.contains("Ayşe Yılmaz"),
            "{body}"
        );
        assert!(
            body.contains("value=\"kaya\""),
            "sorgu kutuda kalır: {body}"
        );

        // Raporlar kapagi her operatorde acilir ve mutabakat baglantisini verir
        let r = app
            .clone()
            .oneshot(request("GET", "/reports", "", &auditor))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = body_string(r).await;
        assert!(body.contains("/reconcile"), "{body}");
        assert!(
            body.contains("/upcoming") && body.contains("/used-names"),
            "{body}"
        );

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
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
        let role: i64 = sqlx::query_scalar(
            "SELECT id FROM roles WHERE kind = 'primary' AND NOT placeholder LIMIT 1",
        )
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
            body.contains("Yeni kimlik kaydedildi"),
            "denetim satırı ham olay anahtarı değil, operatör dilinde listelenir"
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
