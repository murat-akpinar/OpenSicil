// --- START FEATURE: ui-shell ---
// Statik varlıklar binary'ye gömülür (ADR-088): dosya sistemi ve yeni crate (tower-http
// `fs`) gerekmez, runtime imajı tek binary kalır. `app.css` kaynaktan üretilir
// (`sh scripts/build-css.sh`), fontlar self-host (ADR-067).
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

const CSS: &[u8] = include_bytes!("../../frontend/static/app.css");
const JS: &[u8] = include_bytes!("../../frontend/static/app.js");
const FONT_REGULAR: &[u8] =
    include_bytes!("../../frontend/static/CaskaydiaMonoNerdFont-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../frontend/static/CaskaydiaMonoNerdFont-Bold.ttf");

// Varlık adı derlemede sabit; istenen ad listede yoksa 404 (dizin gezinmesi imkânsız).
const ASSETS: [(&str, &str, &[u8]); 4] = [
    ("app.css", "text/css; charset=utf-8", CSS),
    ("app.js", "text/javascript; charset=utf-8", JS),
    (
        "CaskaydiaMonoNerdFont-Regular.ttf",
        "font/ttf",
        FONT_REGULAR,
    ),
    ("CaskaydiaMonoNerdFont-Bold.ttf", "font/ttf", FONT_BOLD),
];

// Varlıklar sürüm başına değişmez; adları da sabit olduğu için bir yıl cache'lenir.
const CACHE: &str = "public, max-age=31536000, immutable";

pub fn routes() -> Router {
    Router::new().route("/static/{name}", get(serve))
}

async fn serve(Path(name): Path<String>) -> Response {
    match ASSETS.iter().find(|(asset, _, _)| *asset == name) {
        Some((_, content_type, body)) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
                (header::CACHE_CONTROL, HeaderValue::from_static(CACHE)),
            ],
            *body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
// --- END FEATURE: ui-shell ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get_asset(name: &str) -> Response {
        routes()
            .oneshot(
                Request::builder()
                    .uri(format!("/static/{name}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn serves_css_and_rejects_unknown_names() {
        let css = get_asset("app.css").await;
        assert_eq!(css.status(), StatusCode::OK);
        assert_eq!(
            css.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert_eq!(get_asset("app.js").await.status(), StatusCode::OK);
        assert_eq!(
            get_asset("CaskaydiaMonoNerdFont-Regular.ttf")
                .await
                .status(),
            StatusCode::OK
        );
        for unknown in ["yok.css", "../Cargo.toml", "app.css.map"] {
            assert_eq!(
                get_asset(unknown).await.status(),
                StatusCode::NOT_FOUND,
                "{unknown}"
            );
        }
    }

    // Uretilen CSS commit'li (ADR-088): bos ya da eski kalirsa arayuz stilsiz acilir.
    #[test]
    fn compiled_css_has_tokens_and_components() {
        let css = std::str::from_utf8(CSS).expect("CSS utf-8");
        for needle in ["--ctp-bg", "data-theme=dark", ".btn", ".tbl", ".badge"] {
            assert!(css.contains(needle), "derlenmiş CSS'te {needle} yok");
        }
    }
}
