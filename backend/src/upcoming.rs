// --- START FEATURE: upcoming-ends ---
// Yaklasan bitisler (F-36, ADR-053): onumuzdeki N gun icinde bitis tarihi olan
// kimlikler, suresi dolacak ek roller, aski baslangiclari ve donusler. Her
// operator okur; IK sozlesme yenilemeyi buradan gorur. N sorgu parametresi,
// varsayilan 30 (ayri bir kurulum ayari acilmadi: ekran basina deger yeter).

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity_web::{internal, OperatorSession};
use crate::shell::Shell;
use crate::web::{render, AppState};

const DEFAULT_DAYS: i32 = 30;
const MAX_DAYS: i32 = 365;

pub struct Upcoming {
    /// Olay turu anahtari; ekran karsiligi i18n'de (upcomingkind.<anahtar>)
    pub kind: String,
    /// Tura bagli ek bilgi (ek rolde rolun adi), yoksa bos
    pub detail: String,
    pub identity_id: i64,
    pub name: String,
    pub day: String,
}

// Dort kaynak tek sorguda; gunler kurulum saat diliminde (ADR-039).
pub async fn list(pool: &PgPool, time_zone: &str, days: i32) -> Result<Vec<Upcoming>, sqlx::Error> {
    // Tur ekranda cevrilir (ADR-089): sorgu metin degil anahtar ve ek bilgi dondurur.
    let rows: Vec<(String, String, i64, String, String)> = sqlx::query_as(
        "WITH today AS (SELECT (now() AT TIME ZONE $1)::date AS d), \
         ends AS ( \
           SELECT 'end' AS kind, ''::text AS detail, i.id, \
                  i.given_name || ' ' || i.surname AS name, \
                  ((i.end_at AT TIME ZONE $1) - interval '1 day')::date AS day \
           FROM identities i WHERE i.deleted_at IS NULL AND i.end_at > now() \
           UNION ALL \
           SELECT 'role_end', r.name, i.id, i.given_name || ' ' || i.surname, a.ends_on \
           FROM identity_additional_roles a JOIN identities i ON i.id = a.identity_id \
           JOIN roles r ON r.id = a.role_id WHERE i.deleted_at IS NULL AND a.ends_on IS NOT NULL \
           UNION ALL \
           SELECT 'suspension_start', '', i.id, i.given_name || ' ' || i.surname, \
                  i.suspension_start \
           FROM identities i WHERE i.deleted_at IS NULL AND i.suspension_start IS NOT NULL \
           UNION ALL \
           SELECT 'suspension_return', '', i.id, i.given_name || ' ' || i.surname, \
                  i.suspension_end + 1 \
           FROM identities i WHERE i.deleted_at IS NULL AND i.suspension_end IS NOT NULL) \
         SELECT e.kind, e.detail, e.id, e.name, to_char(e.day, 'YYYY-MM-DD') FROM ends e, today \
         WHERE e.day >= today.d AND e.day <= today.d + $2 ORDER BY e.day, e.name, e.kind",
    )
    .bind(time_zone)
    .bind(days)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, detail, identity_id, name, day)| Upcoming {
            kind,
            detail,
            identity_id,
            name,
            day,
        })
        .collect())
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/upcoming", get(page))
}

#[derive(Deserialize)]
struct DaysQuery {
    days: Option<i32>,
}

#[derive(Template)]
#[template(path = "upcoming.html")]
struct UpcomingTemplate {
    shell: Shell,
    lang: Lang,
    days: i32,
    rows: Vec<Upcoming>,
}

async fn page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Query(q): Query<DaysQuery>,
) -> Response {
    let days = q.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
    match list(&state.pool, &state.time_zone, days).await {
        Ok(rows) => render(&UpcomingTemplate {
            lang: op.lang,
            shell: Shell::of(&op),
            days,
            rows,
        }),
        Err(e) => internal("yaklaşan bitişler okunamadı", e),
    }
}
// --- END FEATURE: upcoming-ends ---

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn lists_ends_roles_and_suspensions_within_window_in_date_order() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let tz = "Europe/Istanbul";
        sqlx::query(
            "UPDATE identities SET end_at = ((now() AT TIME ZONE $2)::date + 11)::timestamp AT TIME ZONE $2, \
             suspension_start = (now() AT TIME ZONE $2)::date + 3, \
             suspension_end = (now() AT TIME ZONE $2)::date + 19 WHERE id = $1",
        )
        .bind(ids[0])
        .bind(tz)
        .execute(&pool)
        .await
        .unwrap();
        let role: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name) VALUES ('additional', 'Nöbet') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) \
             VALUES ($1, $2, (now() AT TIME ZONE $3)::date + 5)",
        )
        .bind(ids[1])
        .bind(role)
        .bind(tz)
        .execute(&pool)
        .await
        .unwrap();

        let rows = list(&pool, tz, 30).await.unwrap();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["suspension_start", "role_end", "end", "suspension_return"],
            "{rows:?}",
        );
        // Ek rolde tur anahtari yaninda rolun adi (ADR-089: metin ekranda kurulur)
        assert_eq!(rows[1].detail, "Nöbet");
        assert!(rows[0].detail.is_empty());
        assert_eq!(rows[2].identity_id, ids[0]);
        assert_eq!(
            list(&pool, tz, 4).await.unwrap().len(),
            1,
            "yalnızca 3. gün askı"
        );
        assert_eq!(list(&pool, tz, 1).await.unwrap().len(), 0);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    impl std::fmt::Debug for Upcoming {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} {} {}", self.kind, self.name, self.day)
        }
    }
}
