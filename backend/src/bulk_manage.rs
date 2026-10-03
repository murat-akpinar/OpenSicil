// --- START FEATURE: bulk-manage ---
// Toplu yonetime alma (ADR-018 "Yonetime alma", ADR-043, ADR-051, ADR-087): hedefin
// gozlem modundaki baglantilari, okuma seridinin hesapladigi "yonetime alinirsa ne
// degisir" farkiyla listelenir; operator secer. Esik yalnizca yetki ya da hesap durumu
// farki olan secimi sayar (ADR-037/043); asarsa secim sahnelenir ve baska bir Sistem
// yoneticisi onaylar (ADR-026, onay aninda yeniden sayim — ADR-055 madde 1). Backend
// yine yalnizca `manage_requested_at` yazar, modu worker cevirir (ADR-015/087).

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity::{self, ManageOutcome};
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::operator_session::Operator;
use crate::org_web::Notice;
use crate::shell::Shell;
use crate::web::{render, AppState};

/// Tekil yonetime almayla ayni (ADR-087 madde 4); onay yalnizca Sistem yoneticisi.
pub const AUTHORITIES: &[&str] = &["hr", "admin"];
pub const APPROVE_AUTHORITIES: &[&str] = &["admin"];

pub struct Row {
    pub identity_id: i64,
    pub person: String,
    pub account: String,
    /// Okuma seridinin yazdigi fark metni; bos = hesaplanmadi
    pub diff: String,
    /// Some(true) esige giren fark, Some(false) yalnizca oznitelik, None hesaplanmadi
    pub applies: Option<bool>,
    pub diff_at: String,
    /// ADR-087: istek zaten yazildi, worker'i bekliyor
    pub requested: bool,
    /// ADR-103 madde 5: rolu yer tutucu, yonetime alinamaz
    pub role_undefined: bool,
}

impl Row {
    pub fn selectable(&self) -> bool {
        !self.requested && !self.role_undefined
    }

    pub fn applies_key(&self) -> &'static str {
        match self.applies {
            Some(true) => "manage.applies",
            Some(false) => "manage.attributes_only",
            None => "manage.not_computed",
        }
    }
}

type RowTuple = (
    i64,
    String,
    String,
    Option<String>,
    Option<bool>,
    Option<String>,
    bool,
    bool,
);

/// Hedefin gozlem baglantilari (silinmemis kimlik, bizim silmedigimiz hesap).
pub async fn load(pool: &PgPool, target: i64) -> Result<Vec<Row>, sqlx::Error> {
    let rows: Vec<RowTuple> = sqlx::query_as(
        "SELECT i.id, i.given_name || ' ' || i.surname, \
         COALESCE(i.username, l.external_id), l.observed_diff, l.observed_diff_applies, \
         to_char(l.observed_diff_at, 'YYYY-MM-DD HH24:MI'), l.manage_requested_at IS NOT NULL, \
         r.placeholder \
         FROM account_links l JOIN identities i ON i.id = l.identity_id \
         JOIN roles r ON r.id = i.primary_role_id \
         WHERE l.target_system_id = $1 AND l.mode = 'observed' AND l.deleted_by_us_at IS NULL \
         AND i.deleted_at IS NULL ORDER BY i.surname, i.given_name, i.id",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(identity_id, person, account, diff, applies, diff_at, requested, role_undefined)| {
                Row {
                    identity_id,
                    person,
                    account,
                    diff: diff.unwrap_or_default(),
                    applies,
                    diff_at: diff_at.unwrap_or_default(),
                    requested,
                    role_undefined,
                }
            },
        )
        .collect())
}

/// Hedef basina hesap kapsami: kac baglanti yonetiliyor, kac tanesi gozlemde.
/// `load` ile ayni dosyada duruyor ki kosullar ayrismasin — rapor "5 gozlemde"
/// diyorsa o hedefin listesi bes satir gostermeli.
pub struct Coverage {
    pub target_id: i64,
    pub name: String,
    pub managed: i64,
    pub observed: i64,
}

impl Coverage {
    pub fn total(&self) -> i64 {
        self.managed + self.observed
    }
}

/// Baglantisi olan hedefler; hic hesabi olmayan hedef (henuz baglanmamis
/// Zimbra gibi) raporda yer kaplamaz.
pub async fn coverage(pool: &PgPool) -> Result<Vec<Coverage>, sqlx::Error> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT t.id, t.name, \
         count(i.id) FILTER (WHERE l.mode = 'managed'), \
         count(i.id) FILTER (WHERE l.mode = 'observed') \
         FROM target_systems t \
         LEFT JOIN account_links l ON l.target_system_id = t.id AND l.deleted_by_us_at IS NULL \
         LEFT JOIN identities i ON i.id = l.identity_id AND i.deleted_at IS NULL \
         GROUP BY t.id, t.name HAVING count(i.id) > 0 ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(target_id, name, managed, observed)| Coverage {
            target_id,
            name,
            managed,
            observed,
        })
        .collect())
}

/// ADR-037/043: esige yalnizca yetki ya da hesap durumu farki olan secim girer.
pub fn affected(rows: &[Row], selected: &[i64]) -> usize {
    rows.iter()
        .filter(|r| selected.contains(&r.identity_id) && r.applies == Some(true))
        .count()
}

/// Formdaki `link=<kimlik>` alanlari; bicimi bozuk olan atlanir, tekrar tekillenir.
fn selected_ids(form: &[(String, String)]) -> Vec<i64> {
    let mut ids: Vec<i64> = form
        .iter()
        .filter(|(name, _)| name == "link")
        .filter_map(|(_, value)| value.parse().ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Default, Debug)]
pub struct Requested {
    pub requested: Vec<i64>,
    /// Atlanan kimlik ve nedeni (i18n anahtari)
    pub skipped: Vec<(i64, &'static str)>,
}

/// Secilen her kimlik icin tekil yonetime alma istegi (ayni kapi, ayni kurallar).
pub async fn request(pool: &PgPool, target: i64, ids: &[i64]) -> Result<Requested, sqlx::Error> {
    let mut outcome = Requested::default();
    for id in ids {
        match identity::request_management(pool, *id, target).await? {
            ManageOutcome::Requested => outcome.requested.push(*id),
            ManageOutcome::NoObservedAccount => {
                outcome.skipped.push((*id, "err.observed_account_missing"))
            }
            ManageOutcome::RoleUndefined => {
                outcome.skipped.push((*id, "err.role_undefined_manage"))
            }
        }
    }
    Ok(outcome)
}

// ---- esigi asan secim: sahneleme ve onay ----

pub struct Batch {
    pub id: i64,
    pub target: i64,
    pub ids: Vec<i64>,
    pub affected_at_stage: i32,
    pub by_subject: String,
    pub by_username: String,
    pub age_seconds: i64,
}

impl Batch {
    pub fn approvable_by(&self, approver_subject: &str, timelock_hours: u32) -> bool {
        crate::change_set::approvable(
            &self.by_subject,
            self.age_seconds,
            approver_subject,
            timelock_hours,
        )
    }
}

pub struct BatchSummary {
    pub id: i64,
    pub by_username: String,
    pub count: usize,
    pub age_minutes: i64,
}

pub async fn stage(
    pool: &PgPool,
    target: i64,
    ids: &[i64],
    affected: usize,
    by: (&str, &str),
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO manage_batches (target_system_id, identity_ids, affected, by_subject, \
         by_username) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(target)
    .bind(ids)
    .bind(affected as i32)
    .bind(by.0)
    .bind(by.1)
    .fetch_one(pool)
    .await
}

pub async fn pending(pool: &PgPool, target: i64, id: i64) -> Result<Option<Batch>, sqlx::Error> {
    let row: Option<(Vec<i64>, i32, String, String, i64)> = sqlx::query_as(
        "SELECT identity_ids, affected, by_subject, by_username, \
         EXTRACT(EPOCH FROM now() - created_at)::bigint \
         FROM manage_batches WHERE id = $1 AND target_system_id = $2",
    )
    .bind(id)
    .bind(target)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(ids, affected_at_stage, by_subject, by_username, age_seconds)| Batch {
            id,
            target,
            ids,
            affected_at_stage,
            by_subject,
            by_username,
            age_seconds,
        },
    ))
}

pub async fn list_pending(pool: &PgPool, target: i64) -> Result<Vec<BatchSummary>, sqlx::Error> {
    let rows: Vec<(i64, String, i32, i64)> = sqlx::query_as(
        "SELECT id, by_username, cardinality(identity_ids), \
         (EXTRACT(EPOCH FROM now() - created_at) / 60)::bigint \
         FROM manage_batches WHERE target_system_id = $1 ORDER BY id",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, by_username, count, age_minutes)| BatchSummary {
            id,
            by_username,
            count: usize::try_from(count).unwrap_or_default(),
            age_minutes,
        })
        .collect())
}

pub async fn discard(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM manage_batches WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() == 1)
}

// ---- web ----

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/targets/{id}/manage", get(page).post(submit))
        .route("/targets/{id}/manage/diff", post(request_diff))
        .route("/targets/{id}/manage/{batch}", get(batch_page))
        .route("/targets/{id}/manage/{batch}/approve", post(approve))
        .route("/targets/{id}/manage/{batch}/reject", post(reject))
}

#[derive(Template)]
#[template(path = "bulk_manage.html")]
struct ManageTemplate {
    lang: Lang,
    shell: Shell,
    notice: Notice,
    target_id: i64,
    target_name: String,
    rows: Vec<Row>,
    pending: Vec<BatchSummary>,
    can_manage: bool,
    threshold: usize,
    /// Acik fark hesabi isi var
    diff_running: bool,
}

#[derive(Template)]
#[template(path = "bulk_manage_batch.html")]
struct ManageBatchTemplate {
    lang: Lang,
    shell: Shell,
    notice: Notice,
    target_id: i64,
    target_name: String,
    batch_id: i64,
    batch_sub: String,
    rows: Vec<Row>,
    affected: usize,
    changed_since: bool,
    changed_text: String,
    can_approve: bool,
    can_reject: bool,
    approver_is_initiator: bool,
}

/// Eylemin yapildigi GET sayfasi: POST'lar buraya yonlendirir (ADR-126 madde 1).
fn page_path(target: i64) -> String {
    format!("/targets/{target}/manage")
}

async fn target_name(pool: &PgPool, target: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM target_systems WHERE id = $1")
        .bind(target)
        .fetch_optional(pool)
        .await
}

async fn diff_running(pool: &PgPool, target: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM read_jobs WHERE kind = 'manage_diff' \
         AND target_system_id = $1 AND status IN ('queued', 'running'))",
    )
    .bind(target)
    .fetch_one(pool)
    .await
}

async fn render_page(state: &AppState, op: &Operator, target: i64, notice: Notice) -> Response {
    let loaded = tokio::try_join!(
        target_name(&state.pool, target),
        load(&state.pool, target),
        list_pending(&state.pool, target),
        diff_running(&state.pool, target),
    );
    match loaded {
        Ok((Some(target_name), rows, pending, diff_running)) => render(&ManageTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            notice,
            target_id: target,
            target_name,
            rows,
            pending,
            can_manage: allowed(op, AUTHORITIES),
            threshold: state.change_set_threshold,
            diff_running,
        }),
        Ok((None, ..)) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("toplu yönetime alma sayfası okunamadı", e),
    }
}

async fn page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(target): Path<i64>,
) -> Response {
    let notice = Notice::take(&state.pool, &op.username).await;
    render_page(&state, &op, target, notice).await
}

/// "Farkı hesapla": okuma seridine `manage_diff` isi; acik is varken ikincisi acilmaz.
async fn request_diff(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(target): Path<i64>,
) -> Response {
    if !allowed(&op, AUTHORITIES) {
        return forbidden(op.lang);
    }
    let opened = sqlx::query(
        "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
         VALUES ('manage_diff', $1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(target)
    .bind(&op.username)
    .execute(&state.pool)
    .await;
    let key = match opened {
        Ok(done) if done.rows_affected() == 1 => "manage.diff_requested",
        Ok(_) => "manage.diff_already_open",
        Err(e) => return internal("fark hesabı isteği yazılamadı", e),
    };
    Notice::info(op.lang.t(key).to_string())
        .redirect(&state.pool, &op, &page_path(target))
        .await
}

/// Secim: esigi asmayan secim hemen istek yazar, asan sahnelenir.
async fn submit(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(target): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, AUTHORITIES) {
        return forbidden(op.lang);
    }
    let ids = selected_ids(&form);
    if ids.is_empty() {
        return Notice::err(op.lang.t("manage.none_selected").to_string())
            .redirect(&state.pool, &op, &page_path(target))
            .await;
    }
    let rows = match load(&state.pool, target).await {
        Ok(rows) => rows,
        Err(e) => return internal("gözlem bağlantıları okunamadı", e),
    };
    let affected = affected(&rows, &ids);
    if affected > state.change_set_threshold {
        let by = (op.subject.as_str(), op.username.as_str());
        let id = match stage(&state.pool, target, &ids, affected, by).await {
            Ok(id) => id,
            Err(e) => return internal("toplu yönetime alma sahnelenemedi", e),
        };
        let detail = serde_json::json!({ "batch_id": id, "target_system_id": target,
            "selected": ids.len(), "affected": affected });
        audit_operator(&state, &op, crate::audit::MANAGE_STAGED, None, detail).await;
        return Redirect::to(&format!("/targets/{target}/manage/{id}")).into_response();
    }
    match request_and_audit(&state, &op, target, &ids).await {
        Ok(text) => {
            Notice::info(text)
                .redirect(&state.pool, &op, &page_path(target))
                .await
        }
        Err(e) => internal("yönetime alma isteği yazılamadı", e),
    }
}

/// Istek + denetim + toplu oncelikli is; bildirim metni doner.
async fn request_and_audit(
    state: &AppState,
    op: &Operator,
    target: i64,
    ids: &[i64],
) -> Result<String, sqlx::Error> {
    let outcome = request(&state.pool, target, ids).await?;
    for id in &outcome.requested {
        let detail = serde_json::json!({ "target_system_id": target, "bulk": true });
        audit_operator(
            state,
            op,
            crate::audit::ACCOUNT_MANAGE_REQUESTED,
            Some(*id),
            detail,
        )
        .await;
        crate::jobs::enqueue(&state.pool, *id, target, crate::jobs::Priority::Bulk).await?;
    }
    Ok(op.lang.tn(
        "manage.requested_n",
        &[
            &outcome.requested.len().to_string(),
            &outcome.skipped.len().to_string(),
        ],
    ))
}

async fn load_batch(
    state: &AppState,
    op: &Operator,
    target: i64,
    id: i64,
) -> Result<(Batch, Vec<Row>), Box<Response>> {
    let batch = match pending(&state.pool, target, id).await {
        Ok(Some(batch)) => batch,
        Ok(None) => {
            let notice = Notice::err(op.lang.t("err.manage_batch_not_found").to_string());
            return Err(Box::new(render_page(state, op, target, notice).await));
        }
        Err(e) => {
            return Err(Box::new(internal(
                "toplu yönetime alma partisi okunamadı",
                e,
            )))
        }
    };
    match load(&state.pool, target).await {
        Ok(rows) => Ok((batch, rows)),
        Err(e) => Err(Box::new(internal("gözlem bağlantıları okunamadı", e))),
    }
}

async fn render_batch(
    state: &AppState,
    op: &Operator,
    batch: Batch,
    rows: Vec<Row>,
    notice: Notice,
) -> Response {
    let Ok(Some(target_name)) = target_name(&state.pool, batch.target).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // ADR-055 madde 1: onay anindaki sayim; secili olup artik gozlemde olmayan dusmustur
    let rows: Vec<Row> = rows
        .into_iter()
        .filter(|r| batch.ids.contains(&r.identity_id))
        .collect();
    let affected = affected(&rows, &batch.ids);
    let staged = usize::try_from(batch.affected_at_stage).unwrap_or_default();
    let approver_is_initiator = !batch.approvable_by(&op.subject, state.approval_timelock_hours);
    render(&ManageBatchTemplate {
        lang: op.lang,
        shell: Shell::of(op),
        notice,
        target_id: batch.target,
        target_name,
        batch_id: batch.id,
        batch_sub: op.lang.tn(
            "manage.batch_sub",
            &[&batch.by_username, &(batch.age_seconds / 60).to_string()],
        ),
        affected,
        changed_since: affected != staged,
        changed_text: op.lang.tn(
            "import.changed_since",
            &[&staged.to_string(), &affected.to_string()],
        ),
        can_approve: allowed(op, APPROVE_AUTHORITIES) && !approver_is_initiator && !rows.is_empty(),
        can_reject: allowed(op, APPROVE_AUTHORITIES) || batch.by_subject == op.subject,
        approver_is_initiator,
        rows,
    })
}

async fn batch_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((target, id)): Path<(i64, i64)>,
) -> Response {
    match load_batch(&state, &op, target, id).await {
        Ok((batch, rows)) => {
            let notice = Notice::take(&state.pool, &op.username).await;
            render_batch(&state, &op, batch, rows, notice).await
        }
        Err(response) => *response,
    }
}

async fn approve(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((target, id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, APPROVE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let (batch, _rows) = match load_batch(&state, &op, target, id).await {
        Ok(loaded) => loaded,
        Err(response) => return *response,
    };
    if !batch.approvable_by(&op.subject, state.approval_timelock_hours) {
        let to = format!("{}/{id}", page_path(target));
        return Notice::err(op.lang.t("err.approver_is_initiator").to_string())
            .redirect(&state.pool, &op, &to)
            .await;
    }
    let text = match request_and_audit(&state, &op, target, &batch.ids).await {
        Ok(text) => text,
        Err(e) => return internal("toplu yönetime alma uygulanamadı", e),
    };
    if let Err(e) = discard(&state.pool, id).await {
        eprintln!("web: onaylanan toplu yönetime alma partisi silinemedi: {e}");
    }
    let detail =
        serde_json::json!({ "batch_id": id, "target_system_id": target, "by": batch.by_username });
    audit_operator(&state, &op, crate::audit::MANAGE_APPROVED, None, detail).await;
    Notice::info(text)
        .redirect(&state.pool, &op, &page_path(target))
        .await
}

async fn reject(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((target, id)): Path<(i64, i64)>,
) -> Response {
    let batch = match pending(&state.pool, target, id).await {
        Ok(Some(batch)) => batch,
        Ok(None) => {
            return Notice::err(op.lang.t("err.manage_batch_not_found").to_string())
                .redirect(&state.pool, &op, &page_path(target))
                .await;
        }
        Err(e) => return internal("toplu yönetime alma partisi okunamadı", e),
    };
    if !allowed(&op, APPROVE_AUTHORITIES) && batch.by_subject != op.subject {
        return forbidden(op.lang);
    }
    if let Err(e) = discard(&state.pool, id).await {
        return internal("toplu yönetime alma partisi silinemedi", e);
    }
    let detail =
        serde_json::json!({ "batch_id": id, "target_system_id": target, "by": batch.by_username });
    audit_operator(&state, &op, crate::audit::MANAGE_REJECTED, None, detail).await;
    Notice::info(op.lang.t("manage.rejected").to_string())
        .redirect(&state.pool, &op, &page_path(target))
        .await
}
// --- END FEATURE: bulk-manage ---

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, applies: Option<bool>) -> Row {
        Row {
            identity_id: id,
            person: String::new(),
            account: String::new(),
            diff: String::new(),
            applies,
            diff_at: String::new(),
            requested: false,
            role_undefined: false,
        }
    }

    /// ADR-043: yalnizca esige giren fark sayilir; secilmeyen ve hesaplanmayan girmez.
    #[test]
    fn affected_counts_only_selected_rows_with_applicable_diff() {
        let rows = [
            row(1, Some(true)),
            row(2, Some(false)),
            row(3, None),
            row(4, Some(true)),
        ];
        assert_eq!(affected(&rows, &[1, 2, 3]), 1);
        assert_eq!(affected(&rows, &[1, 4]), 2);
        assert_eq!(affected(&rows, &[]), 0);
        let form = vec![
            ("link".to_string(), "4".to_string()),
            ("link".to_string(), "x".to_string()),
            ("other".to_string(), "1".to_string()),
            ("link".to_string(), "4".to_string()),
            ("link".to_string(), "1".to_string()),
        ];
        assert_eq!(selected_ids(&form), vec![1, 4]);
    }

    /// Liste, fark rozeti, fark hesabi istegi; esik altinda hemen istek + is + denetim;
    /// esik ustunde sahneleme, baslatan onaylayamaz, baska admin onaylar; auditor 403;
    /// yer tutucu rollu kimlik atlanir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn lists_observed_links_requests_or_stages_by_threshold() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        for (id, applies, diff) in [
            (
                ids[0],
                Some(true),
                "gözlem modunda, yönetime alınırsa: gruba eklenecek: GG-VPN",
            ),
            (
                ids[1],
                Some(false),
                "gözlem modunda, yönetime alınırsa: yalnızca öznitelik",
            ),
        ] {
            sqlx::query(
                "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, \
                 mode, observed_diff, observed_diff_applies, observed_diff_at) \
                 VALUES ($1, $2, $3, 'adopted', 'observed', $4, $5, now())",
            )
            .bind(id)
            .bind(target)
            .bind(format!("guid-{id}"))
            .bind(diff)
            .bind(applies)
            .execute(&pool)
            .await
            .unwrap();
        }
        let rows = load(&pool, target).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().filter(|r| r.applies == Some(true)).count(), 1);
        // ADR-130: raporun kapsam satiri ayni kosullardan okur — hesabi olmayan
        // hedef (Zimbra) listelenmez, iki gozlem baglantisi burada sayilir.
        let cov = coverage(&pool).await.unwrap();
        assert_eq!(cov.len(), 1, "yalnizca hesabi olan hedef listelenir");
        assert_eq!((cov[0].managed, cov[0].observed, cov[0].total()), (0, 2, 2));

        let mut state = crate::web::test_state(pool.clone(), "https://localhost");
        state.change_set_threshold = 1;
        let app = crate::web::routes().with_state(state);
        let session = |subject: &'static str, authority: &'static str| {
            let pool = pool.clone();
            async move {
                let operator = crate::operator_session::Operator {
                    subject: subject.to_string(),
                    username: subject.to_string(),
                    email: String::new(),
                    authorities: vec![authority.to_string()],
                    auth_source: crate::operator_session::AuthSource::Oidc,
                    lang: crate::i18n::DEFAULT,
                };
                let token = crate::operator_session::create_session(&pool, &operator)
                    .await
                    .unwrap();
                format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME)
            }
        };
        let send = |method: &'static str, uri: String, body: String, cookie: String| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header(header::COOKIE, cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };
        let text = |r: axum::response::Response| async move {
            let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        };
        let base = format!("/targets/{target}/manage");

        let auditor = session("auditor-sub", "auditor").await;
        let page = text(send("GET", base.clone(), String::new(), auditor.clone()).await).await;
        assert!(
            page.contains("GG-VPN") && !page.contains("name=\"link\""),
            "{page}"
        );
        let r = send("POST", base.clone(), format!("link={}", ids[0]), auditor).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let hr = session("hr-sub", "hr").await;
        // fark hesabi istegi: ilki acilir, ikincisi "zaten acik". ADR-126: POST
        // yonlendirir, mesaj sonraki GET'te cikar ve is surerken sayfa kendini tazeler.
        let r = send("POST", format!("{base}/diff"), String::new(), hr.clone()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers()[header::LOCATION].to_str().unwrap(), base);
        let page = text(send("GET", base.clone(), String::new(), hr.clone()).await).await;
        assert!(
            page.contains("name=\"link\"")
                && page.contains(crate::i18n::DEFAULT.t("manage.diff_requested"))
                && page.contains("data-reload"),
            "{page}"
        );
        // flash bir kez gorunur: ikinci GET'te mesaj yok, tazeleme surdugu icin oznitelik var
        let page = text(send("GET", base.clone(), String::new(), hr.clone()).await).await;
        assert!(
            !page.contains(crate::i18n::DEFAULT.t("manage.diff_requested"))
                && page.contains("data-reload"),
            "{page}"
        );
        let open: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM read_jobs WHERE kind = 'manage_diff' AND target_system_id = $1",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 1);
        send("POST", format!("{base}/diff"), String::new(), hr.clone()).await;
        let open: i64 =
            sqlx::query_scalar("SELECT count(*) FROM read_jobs WHERE kind = 'manage_diff'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(open, 1, "açık iş varken ikincisi açılmaz");

        // esik 1: yalnizca oznitelik farki olan ids[1] esige girmez → hemen istek
        send("POST", base.clone(), format!("link={}", ids[1]), hr.clone()).await;
        let requested: Option<bool> = sqlx::query_scalar(
            "SELECT manage_requested_at IS NOT NULL FROM account_links WHERE identity_id = $1",
        )
        .bind(ids[1])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(requested, Some(true));
        let jobs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE identity_id = $1 AND priority = 2")
                .bind(ids[1])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(jobs, 1);

        // esik 1, iki uygulanacak fark: ids[0] + yeni gozlem baglantili ucuncu kimlik → sahnelenir
        let third: i64 = sqlx::query_scalar(
            "INSERT INTO identities (given_name, surname, department_id, primary_role_id, \
             employment_type, start_date) SELECT 'Üçüncü', 'Kişi', department_id, primary_role_id, \
             'permanent', current_date FROM identities WHERE id = $1 RETURNING id",
        )
        .bind(ids[0])
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, \
             observed_diff, observed_diff_applies, observed_diff_at) \
             VALUES ($1, $2, 'guid-3', 'adopted', 'observed', 'gruptan çıkarılacak: GG-X', TRUE, now())",
        )
        .bind(third)
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        let body = format!("link={}&link={third}", ids[0]);
        let r = send("POST", base.clone(), body, hr.clone()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let location = r.headers()[header::LOCATION].to_str().unwrap().to_string();
        assert!(location.starts_with(&format!("{base}/")), "{location}");
        let same_admin = session("hr-sub", "admin").await;
        let r = send(
            "POST",
            format!("{location}/approve"),
            String::new(),
            same_admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers()[header::LOCATION].to_str().unwrap(), location);
        let page = text(send("GET", location.clone(), String::new(), same_admin).await).await;
        assert!(
            page.contains("Ayşe Yılmaz")
                && page.contains("Üçüncü Kişi")
                && page.contains(crate::i18n::DEFAULT.t("err.approver_is_initiator")),
            "{page}"
        );
        let still: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM account_links WHERE identity_id = ANY($1) AND manage_requested_at IS NULL",
        )
        .bind(vec![ids[0], third])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(still, 2, "başlatan onaylayamaz");

        // ucuncu kisinin rolu yer tutucu olursa onayda atlanir (ADR-103 madde 5)
        sqlx::query(
            "UPDATE identities SET primary_role_id = (SELECT id FROM roles WHERE placeholder) WHERE id = $1",
        )
        .bind(third)
        .execute(&pool)
        .await
        .unwrap();
        let other_admin = session("admin-sub", "admin").await;
        let r = send(
            "POST",
            format!("{location}/approve"),
            String::new(),
            other_admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers()[header::LOCATION].to_str().unwrap(), base);
        let page = text(send("GET", base.clone(), String::new(), other_admin).await).await;
        assert!(page.contains("1"), "{page}");
        let requested: Vec<i64> = sqlx::query_scalar(
            "SELECT identity_id FROM account_links WHERE manage_requested_at IS NOT NULL ORDER BY identity_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            requested,
            vec![ids[0], ids[1]],
            "yer tutucu rollü üçüncü atlandı"
        );
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM manage_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM audit_log WHERE event_type LIKE 'manage.%' OR event_type = $1 ORDER BY id",
        )
        .bind(crate::audit::ACCOUNT_MANAGE_REQUESTED)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            events,
            vec![
                "account.manage_requested",
                "manage.staged",
                "account.manage_requested",
                "manage.approved"
            ]
        );

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
