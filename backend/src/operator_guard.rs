// --- START FEATURE: operator-guard ---
// Ayrilmis ya da askidaki operator reddi (ADR-055 madde 4, ADR-059 madde 1):
// her istekte ve oturum acilisinda operatorle eslesen kimligin turetilen
// durumuna bakilir; `ayrildi`, `askida` (ve `silindi`) ise istek reddedilir,
// oturum sonlandirilir. Eslesme ADR-005: preferred_username'in `@` oncesi
// kimligin kullanici adiyla, tamami UPN ile buyuk/kucuk harf duyarsiz.
// Eslesen kimligi olmayan (break-glass) operator etkilenmez.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;

use crate::cookie::{clear_cookie_header, get_cookie, OPERATOR_SESSION_COOKIE_NAME};
use crate::desired_state::{lifecycle_state, Clock, Date, LifecycleState, Timeline};
use crate::web::AppState;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Rejected(LifecycleState),
}

pub fn verdict_for(state: LifecycleState) -> Verdict {
    match state {
        LifecycleState::Departed | LifecycleState::Suspended | LifecycleState::Deleted => {
            Verdict::Rejected(state)
        }
        LifecycleState::Pending | LifecycleState::Active => Verdict::Allowed,
    }
}

// (start_date, end_at, suspension_start, suspension_end, cancelled, emergency,
//  deleted_at, now, today) — tarihler ISO metin, anlar Unix saniyesi; saat
// dilimi cevirisini Postgres yapar (tzdata orada, Rust'ta tz kutuphanesi yok).
pub type TimelineRow = (
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    bool,
    bool,
    Option<i64>,
    i64,
    String,
);

// $2 her zaman kurulum saat dilimi; WHERE kismi cagirana gore degisir
// (operator eslesmesi burada, kisi sayfasi id ile: identity.rs).
#[macro_export]
macro_rules! timeline_sql {
    ($where:literal) => {
        concat!(
            "SELECT to_char(start_date, 'YYYY-MM-DD'), \
             EXTRACT(EPOCH FROM end_at)::bigint, \
             to_char(suspension_start, 'YYYY-MM-DD'), to_char(suspension_end, 'YYYY-MM-DD'), \
             cancelled, emergency_departure, EXTRACT(EPOCH FROM deleted_at)::bigint, \
             EXTRACT(EPOCH FROM now())::bigint, \
             to_char((now() AT TIME ZONE $2)::date, 'YYYY-MM-DD') \
             FROM identities WHERE ",
            $where,
            " LIMIT 1"
        )
    };
}

pub fn timeline_from_row(row: &TimelineRow) -> Result<(Timeline, Clock), sqlx::Error> {
    let date = |s: &str| Date::from_iso(s).ok_or_else(|| sqlx::Error::Decode(s.into()));
    let timeline = Timeline {
        start_date: date(&row.0)?,
        end_at: row.1,
        suspension_start: row.2.as_deref().map(date).transpose()?,
        suspension_end: row.3.as_deref().map(date).transpose()?,
        cancelled: row.4,
        emergency_departure: row.5,
        deleted_at: row.6,
    };
    let clock = Clock {
        now: row.7,
        today: date(&row.8)?,
    };
    Ok((timeline, clock))
}

pub async fn check_operator(
    pool: &PgPool,
    time_zone: &str,
    preferred_username: &str,
) -> Result<Verdict, sqlx::Error> {
    let row: Option<TimelineRow> = sqlx::query_as(timeline_sql!(
        "lower(username) = lower(split_part($1, '@', 1)) OR lower(upn) = lower($1)"
    ))
    .bind(preferred_username)
    .bind(time_zone)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(Verdict::Allowed);
    };
    let (timeline, clock) = timeline_from_row(&row)?;
    Ok(verdict_for(lifecycle_state(&timeline, &clock)))
}

// Her istekte calisan ara katman: gecerli operator oturumu varsa kimligi
// kontrol eder; reddedilirse oturumu siler, cerezi temizler, 403 doner.
pub async fn enforce(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let Some(token) = get_cookie(&headers, OPERATOR_SESSION_COOKIE_NAME) else {
        return next.run(request).await;
    };
    let operator = match crate::operator_session::validate_session(&state.pool, &token).await {
        Ok(Some(op)) => op,
        Ok(None) => return next.run(request).await,
        Err(e) => {
            eprintln!("operator_guard: oturum okunamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match check_operator(&state.pool, &state.time_zone, &operator.username).await {
        Ok(Verdict::Allowed) => next.run(request).await,
        Ok(Verdict::Rejected(reason)) => {
            if let Err(e) = crate::operator_session::delete_session(&state.pool, &token).await {
                eprintln!("operator_guard: oturum silinemedi: {e}");
            }
            rejection_response(&state, &operator, reason).await
        }
        Err(e) => {
            eprintln!("operator_guard: kimlik durumu okunamadı: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// Oturum acilisinda da (ADR-059 madde 1) ayni cevap; cerez zaten yoksa temizleme zararsiz.
pub async fn rejection_response(
    state: &AppState,
    operator: &crate::operator_session::Operator,
    reason: LifecycleState,
) -> Response {
    let actor = crate::audit::Actor {
        subject: Some(&operator.subject),
        username: &operator.username,
    };
    let detail = serde_json::json!({ "state": format!("{reason:?}") });
    if let Err(e) = crate::audit::record(
        &state.pool,
        &actor,
        crate::audit::OPERATOR_REJECTED,
        None,
        detail,
    )
    .await
    {
        eprintln!("operator_guard: denetim kaydı yazılamadı: {e}");
    }
    (
        StatusCode::FORBIDDEN,
        [(
            header::SET_COOKIE,
            clear_cookie_header(OPERATOR_SESSION_COOKIE_NAME),
        )],
        "Erişim reddedildi: kimlik kaydınız ayrılmış ya da askıda.",
    )
        .into_response()
}
// --- END FEATURE: operator-guard ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_departed_suspended_and_deleted_are_rejected() {
        use LifecycleState::*;
        assert_eq!(verdict_for(Active), Verdict::Allowed);
        assert_eq!(verdict_for(Pending), Verdict::Allowed);
        assert_eq!(verdict_for(Departed), Verdict::Rejected(Departed));
        assert_eq!(verdict_for(Suspended), Verdict::Rejected(Suspended));
        assert_eq!(verdict_for(Deleted), Verdict::Rejected(Deleted));
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn matches_username_case_insensitively_and_by_upn_local_part() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        sqlx::query(
            "UPDATE identities SET username = 'ayse.yilmaz', upn = 'ayse.yilmaz@example.local', \
             end_at = now() - interval '1 hour' WHERE id = $1",
        )
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE identities SET username = 'ali.kaya' WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();

        let check = |name: &'static str| check_operator(&pool, "Europe/Istanbul", name);
        assert_eq!(
            check("AYSE.YILMAZ@corp.example").await.unwrap(),
            Verdict::Rejected(LifecycleState::Departed)
        );
        assert_eq!(
            check("ayse.yilmaz@example.local").await.unwrap(),
            Verdict::Rejected(LifecycleState::Departed)
        );
        assert_eq!(check("ali.kaya").await.unwrap(), Verdict::Allowed);
        assert_eq!(
            check("break.glass").await.unwrap(),
            Verdict::Allowed,
            "eşleşen kimlik yoksa serbest"
        );

        sqlx::query("UPDATE identities SET suspension_start = current_date WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            check("ali.kaya").await.unwrap(),
            Verdict::Rejected(LifecycleState::Suspended)
        );
        assert!(
            check_operator(&pool, "Mars/Olympus", "ali.kaya")
                .await
                .is_err(),
            "bilinmeyen saat dilimi hata"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
