// --- START FEATURE: operator-guard ---
// Ayrilmis ya da askidaki operator reddi (ADR-055 madde 4, ADR-059 madde 1):
// her istekte ve oturum acilisinda operatorle eslesen kimligin turetilen
// durumuna bakilir; `ayrildi`, `askida` (ve `silindi`) ise istek reddedilir,
// oturum sonlandirilir. Eslesme ADR-005: preferred_username'in `@` oncesi
// kimligin kullanici adiyla, tamami UPN ile buyuk/kucuk harf duyarsiz.
// Yerel break-glass hesabi hic etkilenmez: adi (`admin`) kimlik tablosunda
// bir kayitla eslesebilir ve o kaydi askiya almak tek acil durum kapisini
// kilitlerdi (guvenlik denetimi OS-11).

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;

use crate::cookie::{clear_cookie_header, get_cookie, OPERATOR_SESSION_COOKIE_NAME};
use crate::desired_state::{lifecycle_state, Clock, Date, LifecycleState, Timeline};
use crate::operator_session::AuthSource;
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
// (operator eslesmesi burada, kisi sayfasi id ile: identity.rs). Satir sayisini
// cagiran belirler: id ile tek satir, ad eslesmesiyle birden fazla olabilir.
// ADR-005/034: operatorun kendi kaydi. Oturum reddi ve kendi kaydi kurallari
// (CSV, ayrilis, rol) ayni eslesmeyi kullanir; ayri yazilinca biri dar kaliyordu
// (guvenlik denetimi OS-04). $1 her zaman operatorun adi.
macro_rules! own_record_match {
    () => {
        "(lower(username) = lower(split_part($1, '@', 1)) OR lower(upn) = lower($1))"
    };
}

/// Kimlik `id` bu operatorun kendi kaydi mi (ADR-005).
pub async fn is_own_record(
    pool: &PgPool,
    id: i64,
    preferred_username: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(concat!(
        "SELECT EXISTS (SELECT 1 FROM identities WHERE id = $2 AND ",
        own_record_match!(),
        ")"
    ))
    .bind(preferred_username)
    .bind(id)
    .fetch_one(pool)
    .await
}

#[macro_export]
macro_rules! timeline_sql {
    ($where:expr) => {
        concat!(
            "SELECT to_char(start_date, 'YYYY-MM-DD'), \
             EXTRACT(EPOCH FROM end_at)::bigint, \
             to_char(suspension_start, 'YYYY-MM-DD'), to_char(suspension_end, 'YYYY-MM-DD'), \
             cancelled, emergency_departure, EXTRACT(EPOCH FROM deleted_at)::bigint, \
             EXTRACT(EPOCH FROM now())::bigint, \
             to_char((now() AT TIME ZONE $2)::date, 'YYYY-MM-DD') \
             FROM identities WHERE ",
            $where
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

// Ad eslesmesi birden fazla kimlige denk gelebilir: `username` tekil ama
// buyuk/kucuk harf duyarli, ayrica bir satir `username`le baska bir satir
// `upn`le eslesebilir. Tek satir cekip `LIMIT 1` demek hangisinin gelecegini
// Postgres'e birakirdi; eslesen her satira bakilir ve biri bile reddediliyorsa
// istek reddedilir (guvenli taraf).
pub async fn check_operator(
    pool: &PgPool,
    time_zone: &str,
    preferred_username: &str,
    auth_source: AuthSource,
) -> Result<Verdict, sqlx::Error> {
    if auth_source == AuthSource::Local {
        return Ok(Verdict::Allowed);
    }
    let rows: Vec<TimelineRow> = sqlx::query_as(timeline_sql!(own_record_match!()))
        .bind(preferred_username)
        .bind(time_zone)
        .fetch_all(pool)
        .await?;
    for row in &rows {
        let (timeline, clock) = timeline_from_row(row)?;
        if let Verdict::Rejected(state) = verdict_for(lifecycle_state(&timeline, &clock)) {
            return Ok(Verdict::Rejected(state));
        }
    }
    Ok(Verdict::Allowed)
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
            log_error!("operator_guard: oturum okunamadı: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if let Some(redirect) = password_change_due(&state, &operator, request.uri().path()).await {
        return redirect;
    }
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    match check_operator(
        &state.pool,
        &time_zone,
        &operator.username,
        operator.auth_source,
    )
    .await
    {
        Ok(Verdict::Allowed) => next.run(request).await,
        Ok(Verdict::Rejected(reason)) => {
            if let Err(e) = crate::operator_session::delete_session(&state.pool, &token).await {
                log_error!("operator_guard: oturum silinemedi: {e}");
            }
            rejection_response(&state, &operator, reason).await
        }
        Err(e) => {
            log_error!("operator_guard: kimlik durumu okunamadı: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// Parola degisimine izin verilen yollar: formun kendisi ve cikis.
const PASSWORD_CHANGE_PATHS: &[&str] = &["/change-password", "/logout"];

// ADR-095 madde 3: yerel hesap ilk girisinde parolasini degistirmeden baska
// hicbir ekrani goremez. Yalnizca yerel oturumda sorulur (AD/OIDC parolasi
// bizde degil), bu yuzden ek sorgu diger kapilari yavaslatmaz.
async fn password_change_due(
    state: &AppState,
    operator: &crate::operator_session::Operator,
    path: &str,
) -> Option<Response> {
    if operator.auth_source != crate::operator_session::AuthSource::Local
        || PASSWORD_CHANGE_PATHS.contains(&path)
    {
        return None;
    }
    match crate::bootstrap_account::must_change_password(&state.pool).await {
        Ok(true) => Some(axum::response::Redirect::to("/change-password").into_response()),
        Ok(false) => None,
        Err(e) => {
            log_error!("operator_guard: yerel hesap durumu okunamadı: {e}");
            Some(StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
        log_error!("operator_guard: denetim kaydı yazılamadı: {e}");
    }
    (
        [(
            header::SET_COOKIE,
            clear_cookie_header(OPERATOR_SESSION_COOKIE_NAME),
        )],
        crate::errors::page_with_hint(
            operator.lang,
            StatusCode::FORBIDDEN,
            "err.access_denied_departed",
        ),
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

        let check =
            |name: &'static str| check_operator(&pool, "Europe/Istanbul", name, AuthSource::Ad);
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

        // Ayni operator adi iki kimlige denk gelebilir: Ayse `username`le, Ali
        // `upn`le eslesir. Satirlarin hangi sirayla dondugu Postgres'in bilecegi
        // is; ikisi de denenir, ayrilmis olan hangisiyse istek reddedilir.
        // (Tek satir cekilseydi bu iki iddiadan biri mutlaka duserdi.)
        sqlx::query("UPDATE identities SET upn = 'ayse.yilmaz@corp.example' WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        let depart = |id: i64, departed: bool| {
            sqlx::query(
                "UPDATE identities SET end_at = CASE WHEN $2 THEN now() - interval '1 hour' END \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(departed)
            .execute(&pool)
        };
        for departed_one in [ids[0], ids[1]] {
            depart(ids[0], departed_one == ids[0]).await.unwrap();
            depart(ids[1], departed_one == ids[1]).await.unwrap();
            assert_eq!(
                check("AYSE.YILMAZ@corp.example").await.unwrap(),
                Verdict::Rejected(LifecycleState::Departed),
                "eşleşenlerden biri ayrılmışsa istek reddedilir"
            );
        }
        depart(ids[1], false).await.unwrap();
        sqlx::query("UPDATE identities SET upn = NULL WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("UPDATE identities SET suspension_start = current_date WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            check("ali.kaya").await.unwrap(),
            Verdict::Rejected(LifecycleState::Suspended)
        );
        assert_eq!(
            check_operator(&pool, "Europe/Istanbul", "ali.kaya", AuthSource::Local)
                .await
                .unwrap(),
            Verdict::Allowed,
            "yerel break-glass oturumu kimlik durumuna bakmaz (OS-11)"
        );
        assert!(
            check_operator(&pool, "Mars/Olympus", "ali.kaya", AuthSource::Ad)
                .await
                .is_err(),
            "bilinmeyen saat dilimi hata"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
