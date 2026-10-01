// --- START FEATURE: reconcile ---
// Mutabakat ekrani (ADR-099): yonetilen kapsamdaki AD hesaplarinin OpenSicil'deki
// karsiligiyla karsilastirilmis hali. Backend AD'ye bagianmaz (ADR-004); bulgulari
// worker okuma seridinde yazar, bu ekran yalnizca `reconcile_findings`'i basar.
//
// "Yeniden tara" okuma seridine istek yazar (ADR-051); taramayi worker yapar.
use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
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
        // --- START FEATURE: bulk-adoption ---
        .route("/targets/{id}/reconcile/adopt", post(adopt))
    // --- END FEATURE: bulk-adoption ---
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
    /// ADR-102 toplu sahiplenme: yetki + formun doldurulacak secenekleri
    can_adopt: bool,
    candidates: Vec<crate::bulk_adopt::Candidate>,
    departments: Vec<crate::identity::Choice>,
    roles: Vec<crate::identity::Choice>,
    today: String,
}

async fn render_page(
    state: &AppState,
    op: &crate::operator_session::Operator,
    target: i64,
    notice: Notice,
) -> Response {
    let loaded = tokio::try_join!(
        load(&state.pool, target, &state.time_zone),
        crate::bulk_adopt::candidates(&state.pool, target),
        crate::identity::form_options(&state.pool),
        crate::identity::today(&state.pool, &state.time_zone),
    );
    match loaded {
        Ok((v, candidates, options, today)) => render(&ReconcileTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            target_id: target,
            v,
            notice,
            can_scan: allowed(op, &SCAN_AUTHORITIES),
            can_adopt: allowed(op, crate::bulk_adopt::AUTHORITIES),
            candidates,
            departments: options.departments,
            roles: options.roles,
            today,
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
        sqlx::query(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, external_id, \
             account_name, given_name, surname) VALUES ($1, $2, 'unmanaged', 'g1', \
             'harry.potter', 'Harry', 'Potter')",
        )
        .bind(target)
        .bind(read_job)
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
        assert!(hr.contains(r#"name="finding""#), "satır kutusu yok");
        assert!(!hr.contains("onclick"), "satır içi script CSP'ye takılır");
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
