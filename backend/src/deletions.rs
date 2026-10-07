// --- START FEATURE: deletion-queue ---
// "Silinmeyi bekleyenler" (ADR-024): ayrilmis kimliklerin yonetilen hesaplari —
// saklama suresi dolmus ve onay bekleyen (hedef onay istiyorsa), onaylanmis,
// kendiliginden silinecek ya da henuz saklamada olanlar (liste arsiv
// hatirlatmasi da olur). Onay `account_links.deletion_approved`i yazar: backend'in
// bu tabloda yazabildigi ikinci kolon (ADR-015; ilki yonetime alma istegi, ADR-087).
// Silmeyi yine worker yapar: zamanlayici `open_retention_jobs` onayli satiri
// alir, burada is hemen de acilir ki operator bir dakika beklemesin.

use askama::Template;
use axum::extract::{Form, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::org_web::Notice;
use crate::shell::Shell;
use crate::web::{render, AppState};

/// Onay yetkisi (ADR-024: IK operatoru ya da Sistem yoneticisi); okuma her operatorde.
pub const APPROVE_AUTHORITIES: &[&str] = &["hr", "admin"];

/// Satir durumlari; ekran karsiligi i18n'de (`deletionstatus.<anahtar>`).
pub const AWAITING: &str = "awaiting_approval";
pub const APPROVED: &str = "approved";
pub const SCHEDULED: &str = "scheduled";
pub const IN_RETENTION: &str = "in_retention";

pub struct Row {
    pub identity_id: i64,
    pub target_id: i64,
    pub person: String,
    pub target: String,
    pub departed_on: String,
    /// Saklama suresinin bittigi gun (kurulum saat dilimi)
    pub retention_ends: String,
    pub status: &'static str,
}

impl Row {
    /// Formun geri yolladigi anahtar: `account_links`in birlesik anahtari.
    pub fn key(&self) -> String {
        format!("{}:{}", self.identity_id, self.target_id)
    }

    pub fn awaiting(&self) -> bool {
        self.status == AWAITING
    }
}

/// Saf: saklama dolmus mu, hedef onay istiyor mu, onay var mi → durum.
pub fn status(retention_over: bool, requires_approval: bool, approved: bool) -> &'static str {
    match (retention_over, requires_approval, approved) {
        (false, _, _) => IN_RETENTION,
        (true, false, _) => SCHEDULED,
        (true, true, false) => AWAITING,
        (true, true, true) => APPROVED,
    }
}

const ROWS_SQL: &str = "SELECT l.identity_id, l.target_system_id, \
    btrim(i.given_name || ' ' || i.surname), t.name, \
    to_char(i.end_at AT TIME ZONE $1, 'YYYY-MM-DD'), \
    to_char((i.end_at + make_interval(days => t.retention_days)) AT TIME ZONE $1, 'YYYY-MM-DD'), \
    i.end_at + make_interval(days => t.retention_days) <= now(), \
    t.delete_requires_approval, l.deletion_approved \
    FROM account_links l \
    JOIN identities i ON i.id = l.identity_id \
    JOIN target_systems t ON t.id = l.target_system_id \
    WHERE l.mode = 'managed' AND l.deleted_by_us_at IS NULL AND i.deleted_at IS NULL \
      AND i.end_at IS NOT NULL AND i.end_at <= now() AND NOT i.cancelled \
    ORDER BY i.end_at + make_interval(days => t.retention_days), 3, t.id";

/// identity_id, target_id, person, target, departed_on, retention_ends,
/// retention_over, requires_approval, approved
type LinkRow = (i64, i64, String, String, String, String, bool, bool, bool);

pub async fn load(pool: &PgPool, time_zone: &str) -> Result<Vec<Row>, sqlx::Error> {
    let rows: Vec<LinkRow> = sqlx::query_as(ROWS_SQL)
        .bind(time_zone)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(identity_id, target_id, person, target, departed_on, ends, over, req, ok)| Row {
                identity_id,
                target_id,
                person,
                target,
                departed_on,
                retention_ends: ends,
                status: status(over, req, ok),
            },
        )
        .collect())
}

/// Onay bekleyen hesap sayisi ve en eskisinin yasi (saniye; saklama bitisinden
/// bu yana) — ADR-024 / F-19 metrikleri, rapor kapagi sayiyi okur.
pub async fn awaiting(pool: &PgPool) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT count(*), \
           COALESCE(EXTRACT(EPOCH FROM max(now() - (i.end_at + make_interval(days => t.retention_days))))::bigint, 0) \
         FROM account_links l \
         JOIN identities i ON i.id = l.identity_id \
         JOIN target_systems t ON t.id = l.target_system_id \
         WHERE l.mode = 'managed' AND l.deleted_by_us_at IS NULL AND i.deleted_at IS NULL \
           AND i.end_at IS NOT NULL AND NOT i.cancelled AND NOT l.deletion_approved \
           AND t.delete_requires_approval \
           AND i.end_at + make_interval(days => t.retention_days) <= now()",
    )
    .fetch_one(pool)
    .await
}

pub async fn awaiting_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    Ok(awaiting(pool).await?.0)
}

/// Yalnizca gercekten onay bekleyen satir onaylanir (saklama dolmus, hedef onay
/// istiyor, henuz onaysiz); gerisi sessizce atlanir. Doner: onaylanan ciftler.
pub async fn approve(pool: &PgPool, pairs: &[(i64, i64)]) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let mut approved = Vec::new();
    for &(identity_id, target_id) in pairs {
        let done = sqlx::query(
            "UPDATE account_links l SET deletion_approved = TRUE \
             FROM identities i, target_systems t \
             WHERE l.identity_id = $1 AND l.target_system_id = $2 \
               AND i.id = l.identity_id AND t.id = l.target_system_id \
               AND l.mode = 'managed' AND l.deleted_by_us_at IS NULL AND NOT l.deletion_approved \
               AND t.delete_requires_approval AND i.deleted_at IS NULL AND NOT i.cancelled \
               AND i.end_at IS NOT NULL \
               AND i.end_at + make_interval(days => t.retention_days) <= now()",
        )
        .bind(identity_id)
        .bind(target_id)
        .execute(pool)
        .await?;
        if done.rows_affected() == 1 {
            approved.push((identity_id, target_id));
        }
    }
    Ok(approved)
}

#[derive(Template)]
#[template(path = "deletions.html")]
struct DeletionsTemplate {
    lang: Lang,
    shell: Shell,
    rows: Vec<Row>,
    notice: Notice,
    can_approve: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/deletions", get(page).post(approve_selected))
}

async fn render_page(
    state: &AppState,
    op: &crate::operator_session::Operator,
    notice: Notice,
) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    match load(&state.pool, &time_zone).await {
        Ok(rows) => render(&DeletionsTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            rows,
            notice,
            can_approve: allowed(op, APPROVE_AUTHORITIES),
        }),
        Err(e) => internal("silinmeyi bekleyenler okunamadı", e),
    }
}

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    let notice = Notice::take(&state.pool, &op.username).await;
    render_page(&state, &op, notice).await
}

/// Formdaki `link=<kimlik>:<hedef>` alanlari; bicimi bozuk olan atlanir.
fn selected_pairs(form: &[(String, String)]) -> Vec<(i64, i64)> {
    form.iter()
        .filter(|(name, _)| name == "link")
        .filter_map(|(_, value)| {
            let (identity, target) = value.split_once(':')?;
            Some((identity.parse().ok()?, target.parse().ok()?))
        })
        .collect()
}

async fn approve_selected(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, APPROVE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let pairs = selected_pairs(&form);
    if pairs.is_empty() {
        let notice = Notice {
            error: op.lang.t("deletions.none_selected").into(),
            ..Notice::default()
        };
        return render_page(&state, &op, notice).await;
    }
    let approved = match approve(&state.pool, &pairs).await {
        Ok(approved) => approved,
        Err(e) => return internal("silme onayı yazılamadı", e),
    };
    for &(identity_id, target_id) in &approved {
        let detail = serde_json::json!({ "target_system_id": target_id });
        audit_operator(
            &state,
            &op,
            crate::audit::ACCOUNT_DELETION_APPROVED,
            Some(identity_id),
            detail,
        )
        .await;
        // Zamanlayici da acardi (ADR-024/028); operator bir dakika beklemesin
        if let Err(e) = crate::jobs::enqueue(
            &state.pool,
            identity_id,
            target_id,
            crate::jobs::Priority::Single,
        )
        .await
        {
            return internal("silme işi açılamadı", e);
        }
    }
    // ADR-126 madde 1: basari yolu yonlendirir, mesaj flash'tan bir kez basilir
    Notice::info(op.lang.t1("deletions.approved_n", approved.len()))
        .redirect(&state.pool, &op, "/deletions")
        .await
}
// --- END FEATURE: deletion-queue ---

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-024: dort durum tek yerde; saklama dolmadan onay sorusu yok.
    #[test]
    fn status_follows_retention_and_approval() {
        assert_eq!(status(false, true, false), IN_RETENTION);
        assert_eq!(status(false, false, true), IN_RETENTION);
        assert_eq!(status(true, false, false), SCHEDULED);
        assert_eq!(status(true, true, false), AWAITING);
        assert_eq!(status(true, true, true), APPROVED);
    }

    #[test]
    fn only_well_formed_link_fields_are_read() {
        let form = vec![
            ("link".to_string(), "7:1".to_string()),
            ("link".to_string(), "bozuk".to_string()),
            ("other".to_string(), "9:1".to_string()),
            ("link".to_string(), "8:x".to_string()),
        ];
        assert_eq!(selected_pairs(&form), vec![(7, 1)]);
    }

    /// Saklamasi dolan ve onay isteyen hesap listede "onay bekliyor"; hr onaylayinca
    /// is acilir ve denetim duser; auditor dugmesiz ve 403; saklamadaki hesap tarihle.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn awaiting_accounts_are_listed_and_approved_with_authority() {
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
        sqlx::query("UPDATE target_systems SET delete_requires_approval = TRUE WHERE id = $1")
            .bind(target)
            .execute(&pool)
            .await
            .unwrap();
        // ids[0]: 100 gun once ayrildi (90 gun saklama doldu); ids[1]: 10 gun once
        for (id, days) in [(ids[0], 100), (ids[1], 10)] {
            sqlx::query(
                "UPDATE identities SET end_at = now() - make_interval(days => $2) WHERE id = $1",
            )
            .bind(id)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, \
                 mode, applied_state) VALUES ($1, $2, $3, 'provisioned', 'managed', 'departed')",
            )
            .bind(id)
            .bind(target)
            .bind(format!("guid-{id}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let rows = load(&pool, "Europe/Istanbul").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].identity_id, rows[0].status), (ids[0], AWAITING));
        assert_eq!(
            (rows[1].identity_id, rows[1].status),
            (ids[1], IN_RETENTION)
        );
        assert_eq!(
            rows[1].retention_ends.len(),
            10,
            "{}",
            rows[1].retention_ends
        );
        assert_eq!(awaiting_count(&pool).await.unwrap(), 1);

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
        let send = |method: &'static str, body: String, cookie: String| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/deletions")
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
        let link = format!("link={}:{target}", ids[0]);

        let auditor = session("auditor").await;
        let page = text(send("GET", String::new(), auditor.clone()).await).await;
        assert!(
            page.contains("Ali Kaya") && page.contains("Ayşe Yılmaz"),
            "{page}"
        );
        assert!(
            !page.contains(r#"name="link""#),
            "auditor onay kutusu görmez"
        );
        let r = send("POST", link.clone(), auditor).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let hr = session("hr").await;
        let page = text(send("GET", String::new(), hr.clone()).await).await;
        assert_eq!(
            page.matches(r#"name="link""#).count(),
            1,
            "yalnızca bekleyen satırda kutu"
        );
        assert!(page.contains(r#"data-select-all="link""#));
        // Saklamadaki hesabi onaylamaya calismak sessizce atlanir, bekleyen onaylanir
        let body = format!("{link}&link={}:{target}", ids[1]);
        let r = send("POST", body, hr.clone()).await;
        assert_eq!(
            r.status(),
            StatusCode::SEE_OTHER,
            "ADR-126: başarı yolu yönlendirir"
        );
        let page = text(send("GET", String::new(), hr.clone()).await).await;
        assert!(
            page.contains(&crate::i18n::DEFAULT.t1("deletions.approved_n", 1)),
            "{page}"
        );
        let approved: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT identity_id, deletion_approved FROM account_links ORDER BY identity_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(approved, vec![(ids[0], true), (ids[1], false)]);
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE identity_id = $1")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 1, "silme işi hemen açıldı");
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE event_type = $1 AND identity_id = $2",
        )
        .bind(crate::audit::ACCOUNT_DELETION_APPROVED)
        .bind(ids[0])
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);
        assert_eq!(awaiting_count(&pool).await.unwrap(), 0);
        let rows = load(&pool, "Europe/Istanbul").await.unwrap();
        assert_eq!(rows[0].status, APPROVED);

        // Bos secim: hata bildirimi, yazma yok
        let page = text(send("POST", String::new(), hr).await).await;
        assert!(page.contains(crate::i18n::DEFAULT.t("deletions.none_selected")));

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
