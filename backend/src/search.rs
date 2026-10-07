// --- START FEATURE: identity-search ---
// Ust bardaki arama kutusu (ADR-096 madde 4). Kimlik numarasi kapsam disi:
// sifreli ve blind index'li (ADR-010), aramasi `pii_reader` yetkisi ister.
// Bu kutu her operatore acik oldugu icin yalnizca ad/kullanici adi/sicil arar;
// sonuclar kimlik listesiyle ayni satir bicimini kullanir.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::i18n::Lang;
use crate::identity::Listed;
use crate::identity_web::{internal, OperatorSession};
use crate::shell::Shell;
use crate::web::{render, AppState};

/// Tek harf butun kurumu doker; iki harften once aranmaz.
const MIN_QUERY_LEN: usize = 2;

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchTemplate {
    lang: Lang,
    shell: Shell,
    query: String,
    too_short: bool,
    results: Vec<Listed>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/search", get(search))
}

async fn search(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let query = params.q.unwrap_or_default().trim().to_string();
    let shell = Shell::of(&op).with_query(query.clone());
    if query.chars().count() < MIN_QUERY_LEN {
        return render(&SearchTemplate {
            lang: op.lang,
            shell,
            query,
            too_short: true,
            results: Vec::new(),
        });
    }
    match crate::identity::search(&state.pool, &time_zone, &query).await {
        Ok(results) => render(&SearchTemplate {
            lang: op.lang,
            shell,
            query,
            too_short: false,
            results,
        }),
        Err(e) => internal("arama başarısız", e),
    }
}
// --- END FEATURE: identity-search ---

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    async fn body_of(response: axum::response::Response) -> String {
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn search_matches_name_username_and_employee_number_but_not_national_id() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        // Ayşe Yılmaz'a kullanıcı adı ve sicil ver; Ali Kaya'yı sil.
        sqlx::query(
            "UPDATE identities SET username = 'ayse.yilmaz', employee_number = 'SC-4071' \
             WHERE id = $1",
        )
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE identities SET deleted_at = now() WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();

        let found = |q: &'static str| {
            let pool = pool.clone();
            async move {
                crate::identity::search(&pool, "Europe/Istanbul", q)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|l| l.name)
                    .collect::<Vec<_>>()
            }
        };

        assert_eq!(found("yılmaz").await, vec!["Ayşe Yılmaz"], "soyad");
        assert_eq!(found("AYŞE").await, vec!["Ayşe Yılmaz"], "büyük/küçük harf");
        assert_eq!(
            found("ayse.yil").await,
            vec!["Ayşe Yılmaz"],
            "kullanıcı adı"
        );
        assert_eq!(found("sc-40").await, vec!["Ayşe Yılmaz"], "sicil");
        assert_eq!(found("şe Yıl").await, vec!["Ayşe Yılmaz"], "tam ad ortası");
        assert!(
            found("kaya").await.is_empty(),
            "silinmiş kayıt aramaya girmez"
        );
        // Joker karakter harf sayılır: tek başına her şeyi dökmemeli
        assert!(found("%").await.is_empty(), "% harf olarak aranır");
        assert!(found("_").await.is_empty(), "_ harf olarak aranır");

        // Avatar baş harfleri liste satırında hazır gelir
        let row = &crate::identity::search(&pool, "Europe/Istanbul", "yılmaz")
            .await
            .unwrap()[0];
        assert_eq!(row.initials, "AY");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_box_needs_two_letters_and_every_operator_may_use_it() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let operator = crate::operator_session::Operator {
            subject: "sub".to_string(),
            username: "deneyimli.denetci".to_string(),
            email: "dd@example.org".to_string(),
            // En dar yetki: auditor yalnızca okur
            authorities: vec!["auditor".to_string()],
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(&pool, &operator)
            .await
            .unwrap();
        let cookie = format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME);
        let get = |uri: &str| {
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::COOKIE, cookie.clone())
                .body(Body::empty())
                .unwrap()
        };

        let short = app.clone().oneshot(get("/search?q=a")).await.unwrap();
        assert_eq!(short.status(), StatusCode::OK);
        let short = body_of(short).await;
        assert!(short.contains("En az iki harf"), "{short}");

        let hit = app.clone().oneshot(get("/search?q=kaya")).await.unwrap();
        assert_eq!(hit.status(), StatusCode::OK);
        let hit = body_of(hit).await;
        assert!(hit.contains("Ali Kaya"), "{hit}");
        // Sorgu üst bardaki kutuda duruyor
        assert!(hit.contains(r#"value="kaya""#), "{hit}");

        let miss = app.clone().oneshot(get("/search?q=zzzz")).await.unwrap();
        let miss = body_of(miss).await;
        assert!(miss.contains("Eşleşen kimlik yok"), "{miss}");

        // Oturumsuz istek girişe düşer
        let anon = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"))
            .oneshot(
                Request::builder()
                    .uri("/search?q=kaya")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anon.status(), StatusCode::SEE_OTHER);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
