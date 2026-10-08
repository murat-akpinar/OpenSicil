// --- START FEATURE: target-readable-urls ---
//! ADR-137: `/targets/active-directory/…` okunur adres. Anahtar router'dan once
//! sayisal id'ye cevrilir, handler'lar `Path<i64>` olarak kalir; sayisal GET adresi
//! okunur adrese yonlenir.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use sqlx::PgPool;

const PREFIX: &str = "/targets/";

#[derive(Debug, PartialEq)]
pub enum Route {
    Pass,
    Rewrite(String),
    Redirect(String),
    /// Hedef adina benzemeyen anahtar: `Path<i64>` 400 donmesin, sayfa yok
    NotFound,
}

/// `targets`: (id, slug). Yalnizca GET/HEAD yonlenir: tarayici POST'u 303'te GET'e cevirir.
pub fn decide(is_get: bool, path_and_query: &str, targets: &[(i64, String)]) -> Route {
    let Some(tail) = path_and_query.strip_prefix(PREFIX) else {
        return Route::Pass;
    };
    let (key, rest) = tail.split_at(tail.find(['/', '?']).unwrap_or(tail.len()));
    match key.parse::<i64>() {
        Ok(_) if !is_get => Route::Pass,
        Ok(id) => targets
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(Route::Pass, |(_, slug)| {
                Route::Redirect(format!("{PREFIX}{slug}{rest}"))
            }),
        Err(_) => targets
            .iter()
            .find(|(_, slug)| slug == key)
            .map_or(Route::NotFound, |(id, _)| {
                Route::Rewrite(format!("{PREFIX}{id}{rest}"))
            }),
    }
}

async fn targets(pool: &PgPool) -> Result<Vec<(i64, String)>, sqlx::Error> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM target_systems")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, name)| crate::org::slug_for(&name).map(|slug| (id, slug)))
        .collect())
}

pub async fn resolve(State(pool): State<PgPool>, mut req: Request, next: Next) -> Response {
    let Some(path) = req.uri().path_and_query().map(|p| p.as_str().to_string()) else {
        return next.run(req).await;
    };
    if !path.starts_with(PREFIX) {
        return next.run(req).await;
    }
    // Okuma hatasinda sayisal adres yine calisir; okunur adres 404'e duser.
    let targets = targets(&pool).await.unwrap_or_default();
    let is_get = matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    match decide(is_get, &path, &targets) {
        Route::Pass => next.run(req).await,
        Route::Redirect(to) => Redirect::to(&to).into_response(),
        Route::NotFound => axum::http::StatusCode::NOT_FOUND.into_response(),
        Route::Rewrite(to) => match to.parse() {
            Ok(uri) => {
                *req.uri_mut() = uri;
                next.run(req).await
            }
            Err(_) => next.run(req).await,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Vec<(i64, String)> {
        vec![(1, "active-directory".into()), (2, "zimbra".into())]
    }

    #[test]
    fn numeric_get_redirects_with_sub_path_and_query() {
        assert_eq!(
            decide(true, "/targets/1/reconcile?page=2", &targets()),
            Route::Redirect("/targets/active-directory/reconcile?page=2".into())
        );
        assert_eq!(
            decide(true, "/targets/2", &targets()),
            Route::Redirect("/targets/zimbra".into())
        );
    }

    #[test]
    fn numeric_post_and_unknown_pass_through() {
        assert_eq!(
            decide(false, "/targets/1/reconcile/scan", &targets()),
            Route::Pass
        );
        assert_eq!(decide(true, "/targets/9/manage", &targets()), Route::Pass);
        assert_eq!(decide(true, "/targets", &targets()), Route::Pass);
        assert_eq!(decide(true, "/identities/1", &targets()), Route::Pass);
    }

    #[test]
    fn slug_rewrites_to_id_for_any_method_unknown_slug_is_not_found() {
        for is_get in [true, false] {
            assert_eq!(
                decide(
                    is_get,
                    "/targets/active-directory/manage/3/approve",
                    &targets()
                ),
                Route::Rewrite("/targets/1/manage/3/approve".into())
            );
        }
        assert_eq!(
            decide(false, "/targets/yok/manage", &targets()),
            Route::NotFound
        );
        assert_eq!(
            decide(true, "/targets/zimbra?tab=catalog", &targets()),
            Route::Rewrite("/targets/2?tab=catalog".into())
        );
    }
}
// --- END FEATURE: target-readable-urls ---
