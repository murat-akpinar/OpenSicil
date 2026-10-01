// --- START FEATURE: metrics ---
// Metrik ucu (F-19, ADR-016/052/054/061): Prometheus metin bicimi, paket yok.
// Degerler kisisel veri icermez (hedef adi disinda etiket yok). Bearer token
// backend ortam degiskeni `METRICS_TOKEN`; nginx `/metrics`i disariya kapatir,
// Prometheus backend'e dogrudan gider (ADR-061 madde 8).

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;

use crate::web::AppState;

pub const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Hedef sistem basina son basarili temas (okuma seridi ya da yazma isi) ve
/// son basarili mutabakat; yoksa satir basilmaz.
pub struct TargetContact {
    pub name: String,
    pub last_success: Option<i64>,
    pub last_reconcile: Option<i64>,
}

pub struct Snapshot {
    /// ADR-052: ayrilmis (bitisi bir saatten eski) ama hedefte kapatilamamis baglanti
    pub departed_unclosed: i64,
    pub needs_intervention: i64,
    /// En eski acik (queued/running) isin yasi, saniye; is yoksa 0
    pub oldest_open_job_age: i64,
    pub pending_change_sets: i64,
    /// (sinif, son bir saatte sayilan kimlik) — `hourly_counter_usage` gorunumu
    pub counters: Vec<(String, i64)>,
    /// (yikici, verme, ilk parola) saatlik sinirlari — ortak ayarlar (ADR-039)
    pub limits: (u32, u32, u32),
    pub awaiting_deletions: i64,
    pub oldest_awaiting_deletion_age: i64,
    pub targets: Vec<TargetContact>,
    /// Worker'in bildirdigi mod ve son gorulmesi; worker hic yazmadiysa yok
    pub dry_run: Option<bool>,
    pub worker_last_seen: Option<i64>,
}

const COUNTER_CLASSES: [&str; 3] = ["destructive", "grant", "first_password"];

pub async fn snapshot(pool: &PgPool, limits: (u32, u32, u32)) -> Result<Snapshot, sqlx::Error> {
    let departed_unclosed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_links l JOIN identities i ON i.id = l.identity_id \
         WHERE l.mode = 'managed' AND l.deleted_by_us_at IS NULL AND i.deleted_at IS NULL \
           AND i.end_at IS NOT NULL AND i.end_at <= now() - interval '1 hour' \
           AND COALESCE(l.applied_state, '') NOT IN ('departed', 'deleted')",
    )
    .fetch_one(pool)
    .await?;
    let (needs_intervention, oldest_open_job_age): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'needs_intervention'), \
           COALESCE(EXTRACT(EPOCH FROM now() - min(created_at) \
             FILTER (WHERE status IN ('queued', 'running')))::bigint, 0) FROM jobs",
    )
    .fetch_one(pool)
    .await?;
    let pending_change_sets: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM roles WHERE pending_definition IS NOT NULL) \
              + (SELECT count(*) FROM departments WHERE pending_definition IS NOT NULL)",
    )
    .fetch_one(pool)
    .await?;
    let counters: Vec<(String, i64)> =
        sqlx::query_as("SELECT operation_class, identities FROM hourly_counter_usage")
            .fetch_all(pool)
            .await?;
    let (awaiting_deletions, oldest_awaiting_deletion_age) =
        crate::deletions::awaiting(pool).await?;
    let targets = target_contacts(pool).await?;
    let worker: Option<(bool, i64)> =
        sqlx::query_as("SELECT dry_run, EXTRACT(EPOCH FROM seen_at)::bigint FROM worker_status")
            .fetch_optional(pool)
            .await?;
    Ok(Snapshot {
        departed_unclosed,
        needs_intervention,
        oldest_open_job_age,
        pending_change_sets,
        counters,
        limits,
        awaiting_deletions,
        oldest_awaiting_deletion_age,
        targets,
        dry_run: worker.map(|w| w.0),
        worker_last_seen: worker.map(|w| w.1),
    })
}

/// Hedefle son basarili temas: okuma seridinin biten isi ya da yazma seridinin
/// basarili sonucu — hangisi daha yeniyse (docs/09 "servis hesabi parolasi
/// dolunca worker sessizce durur" alarmi buna kurulur).
/// hedef adi, son okuma, son yazma, son mutabakat (epoch)
type ContactRow = (String, Option<i64>, Option<i64>, Option<i64>);

async fn target_contacts(pool: &PgPool) -> Result<Vec<TargetContact>, sqlx::Error> {
    let rows: Vec<ContactRow> = sqlx::query_as(
        "SELECT t.name, \
           (SELECT EXTRACT(EPOCH FROM max(r.finished_at))::bigint FROM read_jobs r \
              WHERE r.target_system_id = t.id AND r.status = 'succeeded'), \
           (SELECT EXTRACT(EPOCH FROM max(a.occurred_at))::bigint FROM audit_log a \
              WHERE a.target_system_id = t.id AND a.intent_id IS NOT NULL \
                AND a.outcome = 'succeeded'), \
           (SELECT EXTRACT(EPOCH FROM max(r.finished_at))::bigint FROM read_jobs r \
              WHERE r.target_system_id = t.id AND r.kind = 'reconcile' \
                AND r.status = 'succeeded') \
         FROM target_systems t ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, read, write, reconcile)| TargetContact {
            name,
            last_success: read.max(write),
            last_reconcile: reconcile,
        })
        .collect())
}

/// Bir metrik: ad, aciklama, satirlar (etiket metni + deger).
type Metric<'a> = (&'a str, &'a str, Vec<(String, i64)>);

fn single(value: i64) -> Vec<(String, i64)> {
    vec![(String::new(), value)]
}

fn class_rows(values: [i64; 3]) -> Vec<(String, i64)> {
    COUNTER_CLASSES
        .iter()
        .zip(values)
        .map(|(class, v)| (format!("{{class=\"{class}\"}}"), v))
        .collect()
}

fn per_target(
    targets: &[TargetContact],
    pick: fn(&TargetContact) -> Option<i64>,
) -> Vec<(String, i64)> {
    targets
        .iter()
        .filter_map(|t| pick(t).map(|v| (target_label(&t.name), v)))
        .collect()
}

/// Prometheus metin bicimi; saf. Etiket degerinde `\`, `"` ve satir sonu kacislanir.
/// Worker hic yazmadiysa bayrak ve son gorulme satiri basilmaz (Prometheus kalibi).
pub fn render(s: &Snapshot) -> String {
    let used = class_rows(COUNTER_CLASSES.map(|class| {
        s.counters
            .iter()
            .find(|(c, _)| c == class)
            .map_or(0, |(_, n)| *n)
    }));
    let limits = class_rows([s.limits.0, s.limits.1, s.limits.2].map(i64::from));
    let mut metrics: Vec<Metric<'_>> = vec![
        ("opensicil_departed_unclosed_links", "Ayrilmis ama hedefte bir saatten uzun suredir kapatilamamis yonetilen hesap baglantisi (ADR-052)", single(s.departed_unclosed)),
        ("opensicil_jobs_needs_intervention", "Mudahale bekleyen is sayisi", single(s.needs_intervention)),
        ("opensicil_oldest_open_job_age_seconds", "En eski acik (kuyrukta ya da calisan) isin yasi", single(s.oldest_open_job_age)),
        ("opensicil_pending_change_sets", "Onay bekleyen rol/departman taslagi (ADR-031)", single(s.pending_change_sets)),
        ("opensicil_hourly_counter_used", "Son bir saatte sayilan kimlik, sinif basina (ADR-050)", used),
        ("opensicil_hourly_counter_limit", "Saatlik fren siniri, sinif basina", limits),
        ("opensicil_awaiting_deletion_accounts", "Saklamasi dolmus, silinmesi onay bekleyen hesap (ADR-024)", single(s.awaiting_deletions)),
        ("opensicil_oldest_awaiting_deletion_age_seconds", "Onay bekleyen en eski hesabin saklama bitisinden bu yana gecen sure", single(s.oldest_awaiting_deletion_age)),
        ("opensicil_target_last_success_timestamp_seconds", "Hedef sistemle son basarili temas (okuma ya da yazma)", per_target(&s.targets, |t| t.last_success)),
        ("opensicil_target_last_reconcile_timestamp_seconds", "Hedef sistemin son basarili mutabakat taramasi", per_target(&s.targets, |t| t.last_reconcile)),
    ];
    if let Some(dry_run) = s.dry_run {
        metrics.push((
            "opensicil_dry_run",
            "Worker kuru calistirmada: hedefe yazmiyor (ADR-054)",
            single(i64::from(dry_run)),
        ));
    }
    if let Some(seen) = s.worker_last_seen {
        metrics.push((
            "opensicil_worker_last_seen_timestamp_seconds",
            "Worker'in durum satirini son yazdigi an (dakikada bir)",
            single(seen),
        ));
    }
    let mut out = String::new();
    for (name, help, rows) in metrics {
        gauge(&mut out, name, help, &rows);
    }
    out
}

fn target_label(name: &str) -> String {
    let escaped = name
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("{{target=\"{escaped}\"}}")
}

fn gauge(out: &mut String, name: &str, help: &str, rows: &[(String, i64)]) {
    out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n"));
    for (labels, value) in rows {
        out.push_str(&format!("{name}{labels} {value}\n"));
    }
}

/// Sabit sureli karsilastirma: token uzunlugu ya da icerigi zamanlamadan sizmasin.
fn token_matches(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

pub async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    // Govdeli 401: hata sayfasi ara katmani govdesiz 4xx'i HTML'e cevirirdi
    if state.metrics_token.is_empty() || !token_matches(bearer, &state.metrics_token) {
        return (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response();
    }
    let limits = match crate::common_settings::CommonSettings::from_env() {
        Ok(c) => (
            c.hourly_destructive_limit,
            c.hourly_grant_limit,
            c.hourly_first_password_limit,
        ),
        Err(_) => (0, 0, 0),
    };
    match snapshot(&state.pool, limits).await {
        Ok(s) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
            render(&s),
        )
            .into_response(),
        Err(e) => {
            eprintln!("metrics: sayaçlar okunamadı: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "unavailable\n").into_response()
        }
    }
}
// --- END FEATURE: metrics ---

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            departed_unclosed: 1,
            needs_intervention: 2,
            oldest_open_job_age: 90,
            pending_change_sets: 0,
            counters: vec![("grant".to_string(), 7)],
            limits: (50, 50, 50),
            awaiting_deletions: 3,
            oldest_awaiting_deletion_age: 86400,
            targets: vec![
                TargetContact {
                    name: "Active Directory".to_string(),
                    last_success: Some(1_700_000_000),
                    last_reconcile: None,
                },
                TargetContact {
                    name: "Zim\"bra".to_string(),
                    last_success: None,
                    last_reconcile: Some(1_700_000_100),
                },
            ],
            dry_run: Some(true),
            worker_last_seen: Some(1_700_000_200),
        }
    }

    #[test]
    fn renders_prometheus_text_with_escaped_labels_and_optional_lines() {
        let text = render(&sample());
        assert!(text.contains("# TYPE opensicil_departed_unclosed_links gauge\n"));
        assert!(text.contains("opensicil_departed_unclosed_links 1\n"));
        assert!(text.contains("opensicil_hourly_counter_used{class=\"grant\"} 7\n"));
        assert!(text.contains("opensicil_hourly_counter_used{class=\"destructive\"} 0\n"));
        assert!(text.contains("opensicil_hourly_counter_limit{class=\"first_password\"} 50\n"));
        assert!(text.contains(
            "opensicil_target_last_success_timestamp_seconds{target=\"Active Directory\"} 1700000000\n"
        ));
        assert!(text.contains(
            "opensicil_target_last_reconcile_timestamp_seconds{target=\"Zim\\\"bra\"} 1700000100\n"
        ));
        assert!(!text.contains("last_reconcile_timestamp_seconds{target=\"Active Directory\"}"));
        assert!(text.contains("opensicil_dry_run 1\n"));
        assert!(text.contains("opensicil_worker_last_seen_timestamp_seconds 1700000200\n"));

        let mut unknown = sample();
        unknown.dry_run = None;
        unknown.worker_last_seen = None;
        let text = render(&unknown);
        assert!(
            !text.contains("opensicil_dry_run"),
            "worker hiç yazmadıysa bayrak yok"
        );
    }

    #[test]
    fn token_comparison_is_exact() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("ab", "abc"));
        assert!(!token_matches("", "abc"));
    }

    /// Sayaclar mevcut tablolardan: ayrilmis ama acik kalan baglanti, mudahale isi,
    /// taslak, onay bekleyen silme, worker durumu; token olmadan 401.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_endpoint_counts_from_the_tables_and_needs_the_token() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        // ids[0]: 2 saat once ayrildi, hesap hala etkin → kapatilamamis (ADR-052)
        sqlx::query("UPDATE identities SET end_at = now() - interval '2 hours' WHERE id = $1")
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, \
             mode, applied_state) VALUES ($1, $2, 'g-0', 'provisioned', 'managed', 'active')",
        )
        .bind(ids[0])
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority, status) \
             VALUES ($1, $2, 1, 'needs_intervention')",
        )
        .bind(ids[1])
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO worker_status (worker_id, dry_run) VALUES ('w-test', TRUE)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO read_jobs (kind, target_system_id, status, finished_at) \
             VALUES ('reconcile', $1, 'succeeded', now())",
        )
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();

        let s = snapshot(&pool, (50, 50, 50)).await.unwrap();
        assert_eq!(s.departed_unclosed, 1);
        assert_eq!(s.needs_intervention, 1);
        assert_eq!(s.pending_change_sets, 0);
        assert_eq!(s.dry_run, Some(true));
        assert!(s.worker_last_seen.is_some());
        let ad = s
            .targets
            .iter()
            .find(|t| t.name == "Active Directory")
            .unwrap();
        assert!(ad.last_success.is_some() && ad.last_reconcile.is_some());

        // Gercek router: /metrics operator kapisinin disinda, token backend'de
        let app =
            crate::server::build_router(crate::web::test_state(pool.clone(), "https://localhost"));
        let get = |auth: &'static str| {
            let app = app.clone();
            async move {
                let mut req = Request::builder().uri("/metrics");
                if !auth.is_empty() {
                    req = req.header(header::AUTHORIZATION, auth);
                }
                app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
            }
        };
        assert_eq!(get("").await.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            get("Bearer yanlis").await.status(),
            StatusCode::UNAUTHORIZED
        );
        let r = get("Bearer metrics-test-token").await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get(header::CONTENT_TYPE).unwrap(),
            PROMETHEUS_CONTENT_TYPE
        );
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            text.contains("opensicil_departed_unclosed_links 1\n"),
            "{text}"
        );
        assert!(text.contains("opensicil_dry_run 1\n"), "{text}");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
