// --- START FEATURE: intervention-queue ---
// Mudahale bekleyen isler listesi: panelin "N is mudahale bekliyor" seridinin
// gittigi yer. Kendi tablosu yok — `jobs` + `identities` + `target_systems`;
// satir kisi sayfasina, "tekrar dene" var olan rotaya baglanir (ADR-052, ADR-085).
//
// Serit sayisi (`dashboard::Totals::needs_intervention`) ile bu listenin satir
// sayisi ayni kosulu okur: operator "1 is bekliyor" deyip bos liste gormesin.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity::{split_error, throttle_reason, INTERVENTION_STATUS};
use crate::identity_web::{allowed, internal, OperatorSession, RETRY_AUTHORITIES};
use crate::shell::Shell;
use crate::web::{render, AppState};

pub struct Row {
    pub job_id: i64,
    pub identity_id: i64,
    pub person: String,
    pub target: String,
    pub attempts: i32,
    pub next_attempt_at: String,
    /// Worker hatasinin "sebep" yarisi (ADR-078 madde 6)
    pub summary: String,
    /// Ayni hatanin katlanir teknik yarisi
    pub detail: String,
    /// ADR-050/091 freni: hata degil bekleme, operatorun dilinde
    pub waiting: String,
    /// "Tekrar dene" zaten istenmis mi (ADR-052)
    pub retry_requested: bool,
    /// Ad henuz uretilmedi: satir kisi sayfasindaki ad mudahale bloguna gider
    /// (ADR-022/042/081) — operator "ne yapmam gerekiyor"u listede gorsun
    pub name_pending: bool,
}

impl Row {
    /// Satirin hedefi: ad bekleyen is kisi sayfasindaki ad blogunu acar.
    pub fn href(&self) -> String {
        match self.name_pending {
            true => format!("/identities/{}#ad", self.identity_id),
            false => format!("/identities/{}", self.identity_id),
        }
    }
}

const ROWS_SQL: &str = "SELECT j.id, j.identity_id, \
    btrim(i.given_name || ' ' || i.surname), t.name, j.attempts, \
    to_char(j.next_attempt_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), \
    j.last_error, j.retry_requested, i.username IS NULL \
    FROM jobs j \
    JOIN identities i ON i.id = j.identity_id \
    JOIN target_systems t ON t.id = j.target_system_id \
    WHERE j.status = $1 \
    ORDER BY j.next_attempt_at, j.id";

/// job_id, identity_id, person, target, attempts, next_attempt_at, last_error,
/// retry_requested, name_pending
type JobRow = (
    i64,
    i64,
    String,
    String,
    i32,
    String,
    Option<String>,
    bool,
    bool,
);

pub async fn load(pool: &PgPool, lang: Lang, time_zone: &str) -> Result<Vec<Row>, sqlx::Error> {
    let rows: Vec<JobRow> = sqlx::query_as(ROWS_SQL)
        .bind(INTERVENTION_STATUS)
        .bind(time_zone)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| row_from(lang, r)).collect())
}

fn row_from(lang: Lang, r: JobRow) -> Row {
    let (job_id, identity_id, person, target, attempts, next, error, retry, name_pending) = r;
    let waiting = error
        .as_deref()
        .and_then(|e| throttle_reason(lang, e))
        .unwrap_or_default();
    let (summary, detail) = match waiting.is_empty() {
        true => error.as_deref().map(split_error).unwrap_or_default(),
        false => (String::new(), String::new()),
    };
    Row {
        job_id,
        identity_id,
        person,
        target,
        attempts,
        next_attempt_at: next,
        summary,
        detail,
        waiting,
        retry_requested: retry,
        name_pending,
    }
}

#[derive(Template)]
#[template(path = "interventions.html")]
struct InterventionsTemplate {
    tabs: crate::shell::Tabs,
    lang: Lang,
    shell: Shell,
    rows: Vec<Row>,
    can_retry: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/interventions", get(page))
}

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    match tokio::try_join!(
        load(&state.pool, op.lang, &time_zone),
        crate::identity_web::personnel_tabs(&state.pool, op.lang, "/interventions")
    ) {
        Ok((rows, tabs)) => render(&InterventionsTemplate {
            tabs,
            lang: op.lang,
            shell: Shell::of(&op),
            rows,
            can_retry: allowed(&op, RETRY_AUTHORITIES),
        }),
        Err(e) => internal("müdahale bekleyen işler okunamadı", e),
    }
}
// --- END FEATURE: intervention-queue ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_waiting_for_a_name_points_at_the_name_block() {
        let row = |name_pending| Row {
            job_id: 1,
            identity_id: 7,
            person: "Harry Potter".into(),
            target: "Active Directory".into(),
            attempts: 0,
            next_attempt_at: String::new(),
            summary: String::new(),
            detail: String::new(),
            waiting: String::new(),
            retry_requested: false,
            name_pending,
        };
        assert_eq!(row(true).href(), "/identities/7#ad");
        assert_eq!(row(false).href(), "/identities/7");
    }

    /// Fren (bekleme) ile gercek hata ayri kolonlara dusuyor mu: operator
    /// "sayac dolu" ile "ad cakisti"yi karistirmamali (F-12).
    #[test]
    fn waiting_and_failing_are_separated() {
        let base = (
            1i64,
            7i64,
            "Harry Potter".to_string(),
            "Active Directory".to_string(),
            2i32,
            "2026-10-01 09:00".to_string(),
        );
        let throttled = row_from(
            crate::i18n::DEFAULT,
            (
                base.0,
                base.1,
                base.2.clone(),
                base.3.clone(),
                base.4,
                base.5.clone(),
                Some("throttle:grant:50/50".to_string()),
                false,
                false,
            ),
        );
        assert!(!throttled.waiting.is_empty(), "fren beklemedir");
        assert_eq!(throttled.summary, "");

        let failed = row_from(
            crate::i18n::DEFAULT,
            (
                base.0,
                base.1,
                base.2,
                base.3,
                base.4,
                base.5,
                Some("ad çakışıyor: mevcut.personel bağlı değil".to_string()),
                true,
                true,
            ),
        );
        assert_eq!(failed.summary, "ad çakışıyor");
        assert_eq!(failed.detail, "mevcut.personel bağlı değil");
        assert!(failed.waiting.is_empty());
        assert!(failed.retry_requested);
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_list_shows_every_intervention_and_the_strip_leads_to_it() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));

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
        let body_of = |path: String| {
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
                assert_eq!(r.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };

        // Mudahaledeki is yokken serit cikmaz ve liste bos
        assert!(load(&pool, crate::i18n::DEFAULT, "Europe/Istanbul")
            .await
            .unwrap()
            .is_empty());
        // Menunun `data-match` oneki sayfada; aranan seridin baglantisi
        assert!(!body_of("/".into())
            .await
            .contains("href=\"/interventions\""));

        // Tek is: serit dogrudan kisi sayfasina gider (ADR-103 kalibi)
        sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority, status, last_error) \
             VALUES ($1, $2, 2, 'needs_intervention', 'ad çakışıyor: mevcut.personel bağlı değil')",
        )
        .bind(ids[0])
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        let home = body_of("/".into()).await;
        assert!(
            home.contains(&format!("href=\"/identities/{}\"", ids[0])),
            "tek işte şerit kişi sayfasına gitmeli"
        );

        // Ikinci is: serit artik listeye gider
        sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority, status, last_error) \
             VALUES ($1, $2, 2, 'needs_intervention', 'throttle:grant:50/50')",
        )
        .bind(ids[1])
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            body_of("/".into())
                .await
                .contains("href=\"/interventions\""),
            "iki işte şerit listeye gitmeli"
        );

        // Liste: iki satir, hata ve bekleme ayri, "tekrar dene" hr'de var
        let list = body_of("/interventions".into()).await;
        assert!(list.contains("ad çakışıyor"), "hatanın sebebi görünmeli");
        assert!(
            list.contains(crate::i18n::DEFAULT.key("counter", "grant")),
            "fren sınıfı operatör dilinde görünmeli"
        );
        assert!(
            list.contains(&format!("/identities/{}/jobs/", ids[0])),
            "tekrar dene formu var olan rotaya gitmeli"
        );
        assert_eq!(
            load(&pool, crate::i18n::DEFAULT, "Europe/Istanbul")
                .await
                .unwrap()
                .len(),
            2,
            "liste şeritteki sayıyla aynı koşulu okur"
        );

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
