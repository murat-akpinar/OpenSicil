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
}

async fn render_page(
    state: &AppState,
    op: &crate::operator_session::Operator,
    target: i64,
    notice: Notice,
) -> Response {
    match load(&state.pool, target, &state.time_zone).await {
        Ok(v) => render(&ReconcileTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            target_id: target,
            v,
            notice,
            can_scan: allowed(op, &SCAN_AUTHORITIES),
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

#[cfg(test)]
mod tests {
    use super::*;

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

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
