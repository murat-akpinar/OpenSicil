// --- START FEATURE: reconcile ---
// Mutabakat ekrani (ADR-099): yonetilen kapsamdaki AD hesaplarinin OpenSicil'deki
// karsiligiyla karsilastirilmis hali. Backend AD'ye bagianmaz (ADR-004); bulgulari
// worker okuma seridinde yazar, bu ekran yalnizca `reconcile_findings`'i basar.
//
// "Yeniden tara" okuma seridine istek yazar (ADR-051); taramayi worker yapar.
use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::org_web::Notice;
use crate::shell::Shell;
use crate::web::{render, AppState};

/// "Yeniden tara" yetkisi; okuma her operatorde (auditor dahil).
const SCAN_AUTHORITIES: [&str; 2] = ["admin", "role_admin"];
/// "Yeniden uygula" (F-13): kimlik isi acar — kayit yetkisi olanlar + rol yoneticisi.
const REAPPLY_AUTHORITIES: [&str; 3] = ["admin", "role_admin", "hr"];

pub struct Row {
    /// managed | observed | unmanaged | missing — ekran karsiligi i18n'de
    pub kind: String,
    pub account_name: String,
    pub display_name: String,
    pub container: String,
    /// AD'de etkin mi; `missing` bulgusunda bilinmez
    pub enabled: Option<bool>,
    pub identity_id: Option<i64>,
}

#[derive(Default)]
pub struct View {
    pub rows: Vec<Row>,
    pub managed: i64,
    pub observed: i64,
    pub unmanaged: i64,
    pub missing: i64,
    /// Son taramanin durumu, zamani ve sonuc metni; hic taranmadiysa bos
    pub status: String,
    pub scanned_at: String,
    pub result: String,
}

pub struct Box {
    pub icon: &'static str,
    pub label: &'static str,
    pub count: i64,
}

impl View {
    /// Sayac kutulari; ilgi sirasi: once dikkat isteyenler.
    pub fn boxes(&self) -> Vec<Box> {
        vec![
            Box {
                icon: "user",
                label: "reconcilekind.unmanaged",
                count: self.unmanaged,
            },
            Box {
                icon: "shield",
                label: "reconcilekind.managed",
                count: self.managed,
            },
            Box {
                icon: "search",
                label: "reconcilekind.observed",
                count: self.observed,
            },
            Box {
                icon: "link",
                label: "reconcilekind.missing",
                count: self.missing,
            },
        ]
    }
}

const ROWS_SQL: &str = "SELECT kind, account_name, COALESCE(display_name, ''), \
    COALESCE(container, ''), enabled, identity_id FROM reconcile_findings \
    WHERE target_system_id = $1 \
    ORDER BY CASE kind WHEN 'missing' THEN 0 WHEN 'unmanaged' THEN 1 \
    WHEN 'observed' THEN 2 ELSE 3 END, account_name";

// Son tarama isi: ekran "ne zaman tarandi" ve "suruyor mu" der (ADR-094 kalibi).
const LAST_SQL: &str = "SELECT status, \
    to_char(COALESCE(finished_at, started_at, created_at) AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), \
    COALESCE(result, '') FROM read_jobs WHERE kind = 'reconcile' AND target_system_id = $1 \
    ORDER BY created_at DESC LIMIT 1";

/// kind, account_name, display_name, container, enabled, identity_id
type FindingRow = (String, String, String, String, Option<bool>, Option<i64>);

pub async fn load(pool: &PgPool, target: i64, time_zone: &str) -> Result<View, sqlx::Error> {
    let rows: Vec<FindingRow> = sqlx::query_as(ROWS_SQL)
        .bind(target)
        .fetch_all(pool)
        .await?;
    let last: Option<(String, String, String)> = sqlx::query_as(LAST_SQL)
        .bind(target)
        .bind(time_zone)
        .fetch_optional(pool)
        .await?;

    let count = |want: &str| rows.iter().filter(|r| r.0 == want).count() as i64;
    let view = View {
        managed: count("managed"),
        observed: count("observed"),
        unmanaged: count("unmanaged"),
        missing: count("missing"),
        status: last.as_ref().map(|l| l.0.clone()).unwrap_or_default(),
        scanned_at: last.as_ref().map(|l| l.1.clone()).unwrap_or_default(),
        result: last.map(|l| l.2).unwrap_or_default(),
        rows: rows
            .into_iter()
            .map(
                |(kind, account_name, display_name, container, enabled, identity_id)| Row {
                    kind,
                    account_name,
                    display_name,
                    container,
                    enabled,
                    identity_id,
                },
            )
            .collect(),
    };
    Ok(view)
}

/// Okuma seridine tarama istegi; acik is varsa yenisi acilmaz (ADR-051).
pub async fn request_scan(pool: &PgPool, target: i64, by: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
         VALUES ('reconcile', $1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(target)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/targets/{id}/reconcile", get(page))
        .route("/targets/{id}/reconcile/scan", post(scan))
        .route(
            "/targets/{id}/reconcile/reapply/{identity_id}",
            post(reapply),
        )
        // --- START FEATURE: bulk-adoption ---
        .route("/targets/{id}/reconcile/adopt", post(adopt))
        // --- END FEATURE: bulk-adoption ---
        // --- START FEATURE: ad-field-diff ---
        .route("/targets/{id}/reconcile/take-ad", post(take_ad))
    // --- END FEATURE: ad-field-diff ---
}

#[derive(Template)]
#[template(path = "reconcile.html")]
struct ReconcileTemplate {
    shell: Shell,
    lang: Lang,
    target_id: i64,
    v: View,
    notice: Notice,
    can_scan: bool,
    /// F-13: bulgu satirinda "yeniden uygula" dugmesi
    can_reapply: bool,
    /// ADR-102 toplu sahiplenme: yetki + formun doldurulacak secenekleri
    can_adopt: bool,
    candidates: Vec<crate::bulk_adopt::Candidate>,
    departments: Vec<crate::identity::Choice>,
    roles: Vec<crate::identity::Choice>,
    /// Formun varsayilan rolu: yer tutucu `Tanimsiz` (ADR-103 madde 4); yoksa bos
    default_role: String,
    today: String,
    /// ADR-112 madde 2: "AD'de farkli" listesi; `auditor` gorur, alamaz
    ad_diffs: Vec<crate::ad_diff::Diff>,
    can_take: bool,
}

async fn render_page(
    state: &AppState,
    op: &crate::operator_session::Operator,
    target: i64,
    notice: Notice,
) -> Response {
    let loaded = tokio::try_join!(
        load(&state.pool, target, &state.time_zone),
        crate::bulk_adopt::candidates(&state.pool, &state.aead_key, target),
        crate::identity::form_options(&state.pool),
        crate::identity::today(&state.pool, &state.time_zone),
        crate::identity::placeholder_role_id(&state.pool),
        crate::ad_diff::list(&state.pool, target),
    );
    match loaded {
        Ok((v, candidates, options, today, placeholder, ad_diffs)) => render(&ReconcileTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            target_id: target,
            v,
            notice,
            can_scan: allowed(op, &SCAN_AUTHORITIES),
            can_reapply: allowed(op, &REAPPLY_AUTHORITIES),
            can_adopt: allowed(op, crate::bulk_adopt::AUTHORITIES),
            candidates,
            departments: options.departments,
            roles: options.roles,
            default_role: placeholder.map(|id| id.to_string()).unwrap_or_default(),
            today,
            ad_diffs,
            can_take: allowed(op, crate::ad_diff::AUTHORITIES),
        }),
        Err(e) => internal("mutabakat bulguları okunamadı", e),
    }
}

async fn page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    render_page(&state, &op, id, Notice::default()).await
}

async fn scan(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, &SCAN_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let opened = match request_scan(&state.pool, id, &op.username).await {
        Ok(opened) => opened,
        Err(e) => return internal("mutabakat isteği yazılamadı", e),
    };
    if opened {
        let detail = serde_json::json!({ "action": "reconcile", "target_id": id });
        audit_operator(&state, &op, crate::audit::TARGET_CHANGED, None, detail).await;
    }
    let key = match opened {
        true => "reconcile.requested",
        false => "reconcile.already_open",
    };
    render_page(&state, &op, id, Notice::info(op.lang.t(key).into())).await
}

// F-13 "yeniden uygula": bulgudaki kimlik icin tek kimlik oncelikli is. Motor
// farki yeniden hesaplar ve yonetilen baglantida uygular; gozlemdekinde yalnizca
// farki tazeler (ADR-087). Yonetilmeyen hesabin kimligi yok, onun yolu sahiplenme.
async fn reapply(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path((target, identity_id)): Path<(i64, i64)>,
) -> Response {
    if !allowed(&op, &REAPPLY_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let listed: bool = match sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM reconcile_findings \
         WHERE target_system_id = $1 AND identity_id = $2)",
    )
    .bind(target)
    .bind(identity_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(listed) => listed,
        Err(e) => return internal("bulgu okunamadı", e),
    };
    if !listed {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        identity_id,
        target,
        crate::jobs::Priority::Single,
    )
    .await
    {
        return internal("yeniden uygulama işi açılamadı", e);
    }
    let detail = serde_json::json!({ "target_system_id": target });
    audit_operator(
        &state,
        &op,
        crate::audit::RECONCILE_REAPPLY,
        Some(identity_id),
        detail,
    )
    .await;
    let notice = Notice::info(op.lang.t("reconcile.reapplied").into());
    render_page(&state, &op, target, notice).await
}
// --- END FEATURE: reconcile ---

// --- START FEATURE: bulk-adoption ---
/// Sahiplenmeyi bekleyen hesaplar: hedef basina "yonetilmeyen" bulgu sayisi.
/// ADR-103 madde 3 — panel seridi ve bos personel listesi ayni yerden okur;
/// operatorun mutabakat ekranini kendi bulmasi gerekmesin.
pub struct Unadopted {
    pub target_id: i64,
    pub target_name: String,
    pub count: i64,
}

pub async fn unadopted(pool: &PgPool) -> Result<Vec<Unadopted>, sqlx::Error> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT f.target_system_id, t.name, count(*) FROM reconcile_findings f \
         JOIN target_systems t ON t.id = f.target_system_id \
         WHERE f.kind = 'unmanaged' \
         GROUP BY f.target_system_id, t.name ORDER BY count(*) DESC, t.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(target_id, target_name, count)| Unadopted {
            target_id,
            target_name,
            count,
        })
        .collect())
}

/// Secilen "yonetilmeyen" hesaplari kimlige cevirir (ADR-102). Hedefe yazma
/// yok: baglantiyi worker `observed` modunda kurar.
async fn adopt(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::extract::Form(form): axum::extract::Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, crate::bulk_adopt::AUTHORITIES) {
        return forbidden(op.lang);
    }
    let f = crate::org_web::Fields(form);
    let batch = match batch_from(&f) {
        Ok(batch) => batch,
        Err(key) => return render_page(&state, &op, id, Notice::err(op.lang.t(key).into())).await,
    };
    let keys = crate::national_id::Keys {
        aead: &state.aead_key,
        blind_index: &state.blind_index_key,
    };
    let selected = f.all_i64("finding");
    let outcome =
        crate::bulk_adopt::adopt(&state.pool, &keys, &state.time_zone, id, &selected, &batch).await;
    match outcome {
        Ok(outcome) => {
            audit_adoption(&state, &op, id, &outcome).await;
            render_page(&state, &op, id, adoption_notice(op.lang, &outcome)).await
        }
        Err(crate::bulk_adopt::AdoptError::Invalid(key)) => {
            render_page(&state, &op, id, Notice::err(op.lang.t(key).into())).await
        }
        Err(crate::bulk_adopt::AdoptError::Db(e)) => internal("toplu sahiplenme", e),
    }
}

/// Partinin ortak alanlari; AD'de karsiligi olmayan ya da guvenilmeyen degerler.
fn batch_from(f: &crate::org_web::Fields) -> Result<crate::bulk_adopt::Batch, &'static str> {
    // Departman isteğe bağlı: AD'nin kendi `department` değeri ağaçta bulunuyorsa
    // o kullanılır, bu alan yalnızca eşleşmeyen satırlar için (ADR-102).
    let department = f.opt_i64("department_id");
    let role = f
        .get("primary_role_id")
        .trim()
        .parse::<i64>()
        .map_err(|_| "err.primary_role_required")?;
    let employment_type = f.get("employment_type").trim().to_string();
    let start_date = f.get("start_date").trim().to_string();
    if start_date.is_empty() {
        return Err("err.start_date_required");
    }
    Ok(crate::bulk_adopt::Batch {
        primary_role_id: role,
        employment_type,
        start_date,
        fallback_department_id: department,
    })
}

/// Her acilan kimlik icin ayri `identity.created` satiri: denetim kaydinda
/// "bu kisi nasil geldi" sorusu tek satirda cevaplanabilsin (ADR-009).
async fn audit_adoption(
    state: &AppState,
    op: &crate::operator_session::Operator,
    target: i64,
    outcome: &crate::bulk_adopt::Outcome,
) {
    for id in &outcome.created {
        let detail = serde_json::json!({ "source": "bulk_adopt", "target_system_id": target });
        audit_operator(state, op, crate::audit::IDENTITY_CREATED, Some(*id), detail).await;
    }
}

fn adoption_notice(lang: Lang, outcome: &crate::bulk_adopt::Outcome) -> Notice {
    let created = lang.t1("reconcile.adopted", outcome.created.len());
    if outcome.skipped.is_empty() {
        return Notice::info(created);
    }
    // Atlananlar ad ve nedeniyle yazilir: operator neyin kaldigini bilmeli
    let detail: Vec<String> = outcome
        .skipped
        .iter()
        .map(|(name, reason)| format!("{name} ({})", lang.t(reason)))
        .collect();
    Notice {
        info: created,
        error: lang.tn(
            "reconcile.adopt_skipped",
            &[&outcome.skipped.len().to_string(), &detail.join(", ")],
        ),
    }
}
// --- END FEATURE: bulk-adoption ---

// --- START FEATURE: ad-field-diff ---
/// Secilen "AD'de farkli" satirlarinda AD'deki degeri kimlige yazar (ADR-112
/// madde 2). Hedefe yazma yok: deger artik AD'dekiyle ayni. Departman alinirsa
/// is acilir (ADR-120 madde 3) — OU ve gruplar ondan turer. Her alinan deger
/// denetime once/sonra olarak girer (docs/07 "once/sonra").
async fn take_ad(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::extract::Form(form): axum::extract::Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, crate::ad_diff::AUTHORITIES) {
        return forbidden(op.lang);
    }
    let f = crate::org_web::Fields(form);
    let outcome = match crate::ad_diff::take(&state.pool, id, &f.all("diff")).await {
        Ok(outcome) => outcome,
        Err(e) => return internal("AD'deki değer alınamadı", e),
    };
    for taken in &outcome.taken {
        let detail = serde_json::json!({
            "source": "ad", "target_system_id": id, "field": taken.field,
            "from": taken.from, "to": taken.to,
        });
        audit_operator(
            &state,
            &op,
            crate::audit::IDENTITY_FIELD_TAKEN,
            Some(taken.identity_id),
            detail,
        )
        .await;
    }
    render_page(&state, &op, id, take_notice(op.lang, &outcome)).await
}

fn take_notice(lang: Lang, outcome: &crate::ad_diff::Outcome) -> Notice {
    let info = lang.t1("addiff.taken", outcome.taken.len());
    if outcome.skipped.is_empty() {
        return Notice::info(info);
    }
    // Atlananlar ad ve degeriyle yazilir: mukerrer sicil operatorun isi
    Notice {
        info,
        error: lang.tn(
            "addiff.skipped",
            &[
                &outcome.skipped.len().to_string(),
                &outcome.skipped.join(", "),
            ],
        ),
    }
}
// --- END FEATURE: ad-field-diff ---

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn bulk_adoption_needs_authority_and_turns_unmanaged_accounts_into_identities() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let department: i64 = sqlx::query_scalar("SELECT id FROM departments LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let role: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE kind = 'primary' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        let finding: i64 = sqlx::query_scalar(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, external_id, \
             account_name, display_name, container, enabled, given_name, surname) \
             VALUES ($1, $2, 'unmanaged', 'g1', 'harry.potter', 'Harry Potter', 'OU=Users', \
             true, 'Harry', 'Potter') RETURNING id",
        )
        .bind(target)
        .bind(read_job)
        .fetch_one(&pool)
        .await
        .unwrap();

        let cookie = |authorities: &'static [&'static str]| {
            let pool = pool.clone();
            async move {
                let operator = crate::operator_session::Operator {
                    subject: "sub-x".to_string(),
                    username: "ik.operatoru".to_string(),
                    email: "ik@example.org".to_string(),
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
        let post = |cookie: String, body: String| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/targets/{target}/reconcile/adopt"))
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header(header::COOKIE, cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };
        let body = format!(
            "finding={finding}&primary_role_id={role}&department_id={department}\
             &employment_type=permanent&start_date=2026-10-01"
        );

        // auditor kimlik acamaz
        let r = post(cookie(&["auditor"]).await, body.clone()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM identities WHERE existing_ad_account_hint IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0, "yetkisiz istek kimlik açmadı");

        // hr sahiplenir: kimlik acilir, ipucu yazilir, denetim satiri duser
        let r = post(cookie(&["hr"]).await, body).await;
        assert_eq!(r.status(), StatusCode::OK);
        let row: (String, String, String) = sqlx::query_as(
            "SELECT given_name, surname, existing_ad_account_hint FROM identities \
             WHERE existing_ad_account_hint IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row,
            ("Harry".into(), "Potter".into(), "harry.potter".into())
        );
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE event_type = $1 \
             AND detail->>'source' = 'bulk_adopt'",
        )
        .bind(crate::audit::IDENTITY_CREATED)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// ADR-112 madde 2 + ADR-120: liste yalnizca ikisi de dolu ve **gercekten**
    /// farkli alanlari basar, `auditor` okur ama alamaz, secilmeyen satir
    /// degismez ve alinan deger denetime once/sonra girer. Mukerrer sicil
    /// yazilmaz; departman ada gore eslesirse yazilir ve is acar, eslesmezse
    /// atlanir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn ad_diff_list_is_read_by_everyone_but_only_authority_takes_the_value() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [ayse, ali] = crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE identities SET employee_number = CASE id WHEN $1 THEN '7' \
             ELSE '00000000009' END, mobile_phone = CASE id WHEN $1 \
             THEN '+905000000000' ELSE NULL END WHERE id IN ($1, $2)",
        )
        .bind(ayse)
        .bind(ali)
        .execute(&pool)
        .await
        .unwrap();
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        // ADR-120: ikinci departman Ayse'nin AD'deki degerinin agactaki karsiligi
        sqlx::query("INSERT INTO departments (name) VALUES ('İkinci Birim')")
            .execute(&pool)
            .await
            .unwrap();
        // Ayse: sicil, cep ve departmanda gercek fark; AD'deki departman adi
        // kucuk harfle yazilmis, agacta yine eslesir. Ali: sicil bastaki sifir
        // farkiyla ayni, cep kimlikte bos (dolumun isi), AD'deki deger sabit hat
        // biciminde — tek satiri agacta karsiligi olmayan departman adi.
        sqlx::query(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, external_id, \
             account_name, identity_id, employee_number, mobile, telephone_number, \
             department_name) VALUES \
             ($1, $2, 'managed', 'g1', 'ayse.yilmaz', $3, '00000000009', '+447700900009', NULL, \
              'ikinci birim'), \
             ($1, $2, 'observed', 'g2', 'ali.kaya', $4, '00000000009', NULL, '01632 960001', \
              'Olmayan Birim')",
        )
        .bind(target)
        .bind(read_job)
        .bind(ayse)
        .bind(ali)
        .execute(&pool)
        .await
        .unwrap();

        let cookie = |authority: &'static str| {
            let pool = pool.clone();
            async move {
                let operator = crate::operator_session::Operator {
                    subject: "sub-x".to_string(),
                    username: "ik.operatoru".to_string(),
                    email: "ik@example.org".to_string(),
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
        let send = |method: &'static str, cookie: String, body: String| {
            let app = app.clone();
            async move {
                let uri = match method {
                    "POST" => format!("/targets/{target}/reconcile/take-ad"),
                    _ => format!("/targets/{target}/reconcile"),
                };
                let r = app
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(uri)
                            .header("content-type", "application/x-www-form-urlencoded")
                            .header(header::COOKIE, cookie)
                            .body(Body::from(body))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = r.status();
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };
        let phone_of = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Option<String>>(
                    "SELECT mobile_phone FROM identities WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };

        // auditor listeyi gorur, secim kutusunu ve dugmeyi gormez
        let (_, auditor) = send("GET", cookie("auditor").await, String::new()).await;
        assert!(
            auditor.contains("+447700900009"),
            "AD'deki değer listede yok"
        );
        assert!(
            auditor.contains("+905000000000"),
            "bizdeki değer listede yok"
        );
        assert!(
            !auditor.contains(r#"name="diff""#),
            "auditor seçim kutusu görmez"
        );
        assert!(
            !auditor.contains(crate::i18n::DEFAULT.t("addiff.submit")),
            "auditor eylem düğmesi görmez"
        );
        // Ali'nin sicili ve cebi listeye girmez: sicilde yalnizca bastaki sifir
        // farki var, AD'deki sabit hat kimligin cep alanina yazilamaz
        assert!(!auditor.contains("01632"), "yazılamayan numara listede");

        let (_, hr_page) = send("GET", cookie("hr").await, String::new()).await;
        assert!(
            hr_page.contains(r#"data-select-all="diff""#),
            "başlık kutusu yok"
        );
        assert!(
            hr_page.contains(&format!(r#"value="{ayse}.mobile_phone""#)),
            "satır kutusunun değeri kimlik+alan anahtarı değil"
        );
        // Tam dort satir: Ayse'nin sicil, cep ve departmani + Ali'nin departmani.
        // Ali'nin sicili (bastaki sifir farki) ve cebi (kimlikte bos) girmez.
        assert_eq!(
            hr_page.matches(r#"name="diff""#).count(),
            4,
            "listede olmaması gereken satır var"
        );

        // auditor yazamaz; 403 ve deger yerinde kalir
        let body = format!("diff={ayse}.mobile_phone");
        let (status, _) = send("POST", cookie("auditor").await, body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(phone_of(ayse).await.as_deref(), Some("+905000000000"));

        // hr yalnizca secili satiri alir; secilmeyen sicil degismez
        let (status, _) = send("POST", cookie("hr").await, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(phone_of(ayse).await.as_deref(), Some("+447700900009"));
        let employee: Option<String> =
            sqlx::query_scalar("SELECT employee_number FROM identities WHERE id = $1")
                .bind(ayse)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(employee.as_deref(), Some("7"), "seçilmeyen alan değişti");
        let audited: (String, String, String) = sqlx::query_as(
            "SELECT detail->>'field', detail->>'from', detail->>'to' FROM audit_log \
             WHERE event_type = $1",
        )
        .bind(crate::audit::IDENTITY_FIELD_TAKEN)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            audited,
            (
                "mobile_phone".into(),
                "+905000000000".into(),
                "+447700900009".into()
            )
        );

        // Sicil tekil: AD'deki deger Ali'de duruyor, yazilmaz ve atlandi denir
        let (status, page) = send(
            "POST",
            cookie("hr").await,
            format!("diff={ayse}.employee_number"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let employee: Option<String> =
            sqlx::query_scalar("SELECT employee_number FROM identities WHERE id = $1")
                .bind(ayse)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(employee.as_deref(), Some("7"), "mükerrer sicil yazıldı");
        assert!(page.contains("atlandı"), "atlanan satır duyurulmadı");

        // ADR-120: Ayse'nin departmani agacta eslesir ve yazilir, Ali'nin AD
        // degeri agacta yok — satir atlanir, yeni departman acilmaz. Departman
        // alindiginda hedefe is acilir (OU ve gruplar duzelsin); sicil ve cep
        // alimlarindan once hic is acilmamisti, tek is bu alimin isi.
        let (status, page) = send(
            "POST",
            cookie("hr").await,
            format!("diff={ayse}.department&diff={ali}.department"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let department_of = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT d.name FROM identities i JOIN departments d ON d.id = i.department_id \
                     WHERE i.id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(department_of(ayse).await, "İkinci Birim");
        assert_eq!(
            department_of(ali).await,
            "Test Birimi",
            "ağaçta karşılığı olmayan ad yazıldı"
        );
        assert!(
            page.contains("Olmayan Birim"),
            "atlanan departman adıyla duyurulmadı"
        );
        let jobs: Vec<i64> = sqlx::query_scalar("SELECT identity_id FROM jobs")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            jobs,
            vec![ayse],
            "departman alımı iş açmadı ya da fazla açtı"
        );
        let taken_fields: Vec<String> = sqlx::query_scalar(
            "SELECT detail->>'field' FROM audit_log WHERE event_type = $1 ORDER BY id",
        )
        .bind(crate::audit::IDENTITY_FIELD_TAKEN)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(taken_fields, vec!["mobile_phone", "department"]);
        let detail: (String, String) = sqlx::query_as(
            "SELECT detail->>'from', detail->>'to' FROM audit_log \
             WHERE event_type = $1 AND detail->>'field' = 'department'",
        )
        .bind(crate::audit::IDENTITY_FIELD_TAKEN)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(detail, ("Test Birimi".into(), "ikinci birim".into()));

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// ADR-103 madde 3 "tumunu sec": baslik kutusu yalnizca sahiplenme formuyla
    /// birlikte basilir (yetkisiz operator formu da kutuyu da gormez) ve satir
    /// kutularinin adini tasir — app.js onlari bu adla bulur, satir ici onclick yok.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_select_all_box_only_renders_with_the_adoption_form() {
        use axum::body::Body;
        use axum::http::{header, Request};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        // Bulguda sifreli TC var: ekrana yalnizca maske cikmali (ADR-010)
        let national_id_enc =
            crate::crypto::encrypt_versioned(&[3u8; crate::crypto::KEY_LEN], b"10000000146");
        sqlx::query(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, external_id, \
             account_name, given_name, surname, national_id_enc) VALUES ($1, $2, 'unmanaged', \
             'g1', 'harry.potter', 'Harry', 'Potter', $3)",
        )
        .bind(target)
        .bind(read_job)
        .bind(national_id_enc)
        .execute(&pool)
        .await
        .unwrap();

        let page = |authority: &'static str| {
            let (app, pool) = (app.clone(), pool.clone());
            async move {
                let operator = crate::operator_session::Operator {
                    subject: "sub-x".to_string(),
                    username: "ik.operatoru".to_string(),
                    email: "ik@example.org".to_string(),
                    authorities: vec![authority.to_string()],
                    auth_source: crate::operator_session::AuthSource::Oidc,
                    lang: crate::i18n::DEFAULT,
                };
                let token = crate::operator_session::create_session(&pool, &operator)
                    .await
                    .unwrap();
                let r = app
                    .oneshot(
                        Request::builder()
                            .uri(format!("/targets/{target}/reconcile"))
                            .header(
                                header::COOKIE,
                                format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME),
                            )
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };

        let hr = page("hr").await;
        assert!(
            hr.contains(r#"data-select-all="finding""#),
            "başlık kutusu yok"
        );
        // ADR-103 madde 4: formun varsayilan rolu yer tutucu
        let placeholder: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE placeholder")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            hr.contains(&format!(r#"value="{placeholder}" selected"#)),
            "varsayılan rol Tanımsız değil"
        );
        assert!(hr.contains(r#"name="finding""#), "satır kutusu yok");
        assert!(!hr.contains("onclick"), "satır içi script CSP'ye takılır");
        assert!(hr.contains("10*******46"), "TC maskeli basılmalı");
        assert!(!hr.contains("10000000146"), "düz TC HTML'e girmez");
        let auditor = page("auditor").await;
        assert!(auditor.contains("harry.potter"), "auditor bulguyu okur");
        assert!(
            !auditor.contains("data-select-all"),
            "formu olmayan kutuyu da görmez"
        );

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// F-13 "yeniden uygula": kimligi olan bulguda dugme, yetkiyle is + denetim;
    /// listede olmayan kimlik 404; yonetilmeyen hesapta dugme yok.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn reapply_opens_a_job_for_a_listed_identity_with_authority_only() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        for (kind, guid, sam, identity) in [
            ("managed", "g-m", "ali.kaya", Some(ids[0])),
            ("unmanaged", "g-u", "yabanci", None),
        ] {
            sqlx::query(
                "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
                 external_id, account_name, identity_id) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(target)
            .bind(read_job)
            .bind(kind)
            .bind(guid)
            .bind(sam)
            .bind(identity)
            .execute(&pool)
            .await
            .unwrap();
        }
        let session = |authority: &'static str| {
            let pool = pool.clone();
            async move {
                let operator = crate::operator_session::Operator {
                    subject: "sub-x".to_string(),
                    username: "ik.operatoru".to_string(),
                    email: "ik@example.org".to_string(),
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
        let send = |method: &'static str, uri: String, cookie: String| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };
        let body = |r: axum::response::Response| async move {
            let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        };
        let reapply = format!("/targets/{target}/reconcile/reapply/{}", ids[0]);
        let page = format!("/targets/{target}/reconcile");

        // Dugme yalnizca kimligi olan satirda ve yalnizca yetkiliye
        let hr = session("hr").await;
        let html = body(send("GET", page.clone(), hr.clone()).await).await;
        assert_eq!(html.matches("/reconcile/reapply/").count(), 1, "{html}");
        let auditor = session("auditor").await;
        let html = body(send("GET", page.clone(), auditor.clone()).await).await;
        assert!(!html.contains("/reconcile/reapply/"), "auditor düğmesiz");

        // Yetkisiz 403, is acilmadi
        let r = send("POST", reapply.clone(), auditor).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE identity_id = $1")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 0);

        // hr: tek kimlik oncelikli is + denetim satiri + ekranda bildirim
        let r = send("POST", reapply.clone(), hr.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        let html = body(r).await;
        assert!(
            html.contains(crate::i18n::DEFAULT.t("reconcile.reapplied")),
            "{html}"
        );
        let job: (i64, i16) = sqlx::query_as(
            "SELECT count(*), min(priority) FROM jobs WHERE identity_id = $1 AND target_system_id = $2",
        )
        .bind(ids[0])
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job, (1, crate::jobs::Priority::Single as i16));
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE event_type = $1 AND identity_id = $2",
        )
        .bind(crate::audit::RECONCILE_REAPPLY)
        .bind(ids[0])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);

        // Listede olmayan kimlik 404
        let r = send(
            "POST",
            format!("/targets/{target}/reconcile/reapply/{}", ids[1]),
            hr,
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// ADR-103 madde 3: panel ve bos personel listesi "sirada ne var" diyor.
    /// Serit sahiplenilmemis hesap kalmayinca kendiliginden kayboluyor mu?
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_panel_and_the_empty_list_point_at_the_unadopted_accounts() {
        use axum::body::Body;
        use axum::http::{header, Request};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, external_id, \
             account_name) VALUES ($1, $2, 'unmanaged', 'g1', 'harry.potter')",
        )
        .bind(target)
        .bind(read_job)
        .execute(&pool)
        .await
        .unwrap();

        let operator = crate::operator_session::Operator {
            subject: "sub-x".to_string(),
            username: "ik.operatoru".to_string(),
            email: "ik@example.org".to_string(),
            authorities: vec!["hr".to_string()],
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(&pool, &operator)
            .await
            .unwrap();
        let cookie = format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME);
        let body_of = |path: &'static str| {
            let (app, cookie) = (app.clone(), cookie.clone());
            async move {
                let r = app
                    .oneshot(
                        Request::builder()
                            .uri(path)
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };

        let link = format!("/targets/{target}/reconcile");
        // Seridin kendi cumlesi: ayni ekranda devreye alma kartinin son adimi da
        // ayni mutabakat ekranina baglaniyor (ADR-103 madde 2), bu yuzden yalnizca
        // baglantiya bakmak serit kalktiginda da dogru cikardi.
        let strip = crate::i18n::DEFAULT.t("adopt.cta");
        for path in ["/", "/identities"] {
            let body = body_of(path).await;
            assert!(body.contains(&link), "{path}: mutabakat bağlantısı yok");
            assert!(body.contains(strip), "{path}: şerit yok");
            assert!(body.contains("Active Directory"), "{path}: hedef adı yok");
        }

        // Hesap sahiplenilince (artik "yonetiliyor") yonlendirme susar
        sqlx::query("UPDATE reconcile_findings SET kind = 'managed'")
            .execute(&pool)
            .await
            .unwrap();
        for path in ["/", "/identities"] {
            assert!(
                !body_of(path).await.contains(strip),
                "{path}: şerit kalmamalı"
            );
        }

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn lists_findings_with_counts_and_deduplicates_open_scan_requests() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        // Hic taranmamis hedef: bos ekran, sayac sifir, "son tarama" bos.
        let empty = load(&pool, target, "Europe/Istanbul").await.unwrap();
        assert!(empty.rows.is_empty());
        assert_eq!(empty.scanned_at, "");

        // Ilk istek acilir, ikincisi acik is yuzunden acilmaz (ADR-051).
        assert!(request_scan(&pool, target, "test-admin").await.unwrap());
        assert!(!request_scan(&pool, target, "test-admin").await.unwrap());

        let job: i64 = sqlx::query_scalar(
            "SELECT id FROM read_jobs WHERE kind = 'reconcile' ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE read_jobs SET status = 'succeeded', finished_at = now(), result = $2 WHERE id = $1")
            .bind(job)
            .bind("3 hesap tarandı")
            .execute(&pool)
            .await
            .unwrap();

        for (kind, name, enabled) in [
            ("unmanaged", "hpotter", Some(true)),
            ("unmanaged", "dmalfoy", Some(false)),
            ("missing", "triddle", None::<bool>),
        ] {
            sqlx::query(
                "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
                 external_id, account_name, enabled) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(target)
            .bind(job)
            .bind(kind)
            .bind(format!("guid-{name}"))
            .bind(name)
            .bind(enabled)
            .execute(&pool)
            .await
            .unwrap();
        }

        let v = load(&pool, target, "Europe/Istanbul").await.unwrap();
        assert_eq!(v.unmanaged, 2);
        assert_eq!(v.missing, 1);
        assert_eq!(v.managed, 0);
        // Once dikkat isteyenler: kayip, sonra yonetilmeyen
        let order: Vec<&str> = v.rows.iter().map(|r| r.account_name.as_str()).collect();
        assert_eq!(order, ["triddle", "dmalfoy", "hpotter"]);
        assert_eq!(v.status, "succeeded");
        assert_eq!(v.result, "3 hesap tarandı");
        assert_ne!(v.scanned_at, "");

        // ADR-103 madde 3: yonlendirme yalnizca "yonetilmeyen" bulguyu sayar;
        // `missing` serit yazdirmaz
        let waiting = unadopted(&pool).await.unwrap();
        assert_eq!(waiting.len(), 1, "tek hedefte bulgu var");
        assert_eq!(waiting[0].target_id, target);
        assert_eq!(waiting[0].count, 2);
        assert_ne!(waiting[0].target_name, "");
        sqlx::query("UPDATE reconcile_findings SET kind = 'managed' WHERE kind = 'unmanaged'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            unadopted(&pool).await.unwrap().is_empty(),
            "hepsi yönetiliyorsa şerit yok"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
