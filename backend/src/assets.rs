// --- START FEATURE: ui-shell ---
// Statik varlıklar binary'ye gömülür (ADR-088): dosya sistemi ve yeni crate (tower-http
// `fs`) gerekmez, runtime imajı tek binary kalır. `app.css` kaynaktan üretilir
// (`sh scripts/build-css.sh`), fontlar self-host (ADR-067).
use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

const CSS: &[u8] = include_bytes!("../../frontend/static/app.css");
const JS: &[u8] = include_bytes!("../../frontend/static/app.js");
const FONT_REGULAR: &[u8] =
    include_bytes!("../../frontend/static/CaskaydiaMonoNerdFont-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../../frontend/static/CaskaydiaMonoNerdFont-Bold.ttf");
const FAVICON: &[u8] = include_bytes!("../../frontend/static/favicon.svg");

// Fontun adı sürümünü taşır ve içeriği değişmez: bir yıl, sorulmadan.
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
// CSS ve JS her derlemede değişir, adları değişmez. `immutable` verilirse tarayıcı
// yeni arayüzü bir yıl boyunca eski CSS'le çizer (ADR-098); saklanır ama her
// kullanımdan önce ETag ile doğrulanır.
const CACHE_REVALIDATE: &str = "public, no-cache";

// Varlık adı derlemede sabit; istenen ad listede yoksa 404 (dizin gezinmesi imkânsız).
const ASSETS: [(&str, &str, &[u8], &str); 5] = [
    ("app.css", "text/css; charset=utf-8", CSS, CACHE_REVALIDATE),
    (
        "app.js",
        "text/javascript; charset=utf-8",
        JS,
        CACHE_REVALIDATE,
    ),
    (
        "CaskaydiaMonoNerdFont-Regular.ttf",
        "font/ttf",
        FONT_REGULAR,
        CACHE_IMMUTABLE,
    ),
    (
        "CaskaydiaMonoNerdFont-Bold.ttf",
        "font/ttf",
        FONT_BOLD,
        CACHE_IMMUTABLE,
    ),
    ("favicon.svg", "image/svg+xml", FAVICON, CACHE_REVALIDATE),
];

// İçeriğin özeti: yalnızca dosya gerçekten değişince değişir (ADR-098 madde 2).
// Gömülü baytlar süreç ömründe sabit, bir kez hesaplanır.
static ETAGS: LazyLock<[String; ASSETS.len()]> =
    LazyLock::new(|| ASSETS.map(|(_, _, body, _)| etag(body)));

fn etag(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity(2 + 16);
    out.push('"');
    for byte in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out.push('"');
    out
}

pub fn routes() -> Router {
    Router::new().route("/static/{name}", get(serve))
}

async fn serve(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let Some(index) = ASSETS.iter().position(|(asset, ..)| *asset == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (_, content_type, body, cache) = ASSETS[index];
    let tag = &ETAGS[index];

    // Zayıf karşılaştırma: araya giren bir proxy ETag'i `W/"..."` yapabilir.
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|candidate| candidate.trim().trim_start_matches("W/") == tag)
        });

    let common = [
        (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        (
            header::ETAG,
            HeaderValue::from_str(tag).expect("ETag ascii hex"),
        ),
    ];

    if known {
        return (StatusCode::NOT_MODIFIED, common).into_response();
    }
    (
        common,
        [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
        body,
    )
        .into_response()
}
// --- END FEATURE: ui-shell ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get_asset(name: &str) -> Response {
        request(name, None).await
    }

    async fn request(name: &str, if_none_match: Option<&str>) -> Response {
        let mut builder = Request::builder().uri(format!("/static/{name}"));
        if let Some(value) = if_none_match {
            builder = builder.header(header::IF_NONE_MATCH, value);
        }
        routes()
            .oneshot(builder.body(Body::empty()).unwrap())
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
        // Favicon olmayinca tarayici /favicon.ico isteyip 404 aliyordu
        let icon = get_asset("favicon.svg").await;
        assert_eq!(icon.status(), StatusCode::OK);
        assert_eq!(icon.headers()[header::CONTENT_TYPE], "image/svg+xml");
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

    // ADR-098: sabit adli CSS `immutable` verilirse tarayici yeni arayuzu eski
    // CSS'le cizer. Degisen varlik dogrulanir, degismeyen font bir yil cache'lenir.
    #[tokio::test]
    async fn changing_assets_revalidate_and_fonts_stay_immutable() {
        let css = get_asset("app.css").await;
        assert_eq!(css.headers()[header::CACHE_CONTROL], "public, no-cache");
        let tag = css.headers()[header::ETAG].to_str().unwrap().to_owned();
        assert!(tag.starts_with('"') && tag.len() == 18, "ETag: {tag}");

        // Ayni icerik: govde gitmez.
        let cached = request("app.css", Some(&tag)).await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        // Proxy ETag'i zayiflatmis olabilir.
        let weak = request("app.css", Some(&format!("W/{tag}"))).await;
        assert_eq!(weak.status(), StatusCode::NOT_MODIFIED);
        // Eski icerigin ETag'i: tam yanit.
        let stale = request("app.css", Some("\"0000000000000000\"")).await;
        assert_eq!(stale.status(), StatusCode::OK);

        // Her varligin ETag'i kendine ait.
        let js = get_asset("app.js").await;
        assert_ne!(js.headers()[header::ETAG], css.headers()[header::ETAG]);

        let font = get_asset("CaskaydiaMonoNerdFont-Regular.ttf").await;
        assert_eq!(
            font.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
    }

    // Uretilen CSS commit'li (ADR-088): bos ya da eski kalirsa arayuz stilsiz acilir.
    #[test]
    fn compiled_css_has_tokens_and_components() {
        let css = std::str::from_utf8(CSS).expect("CSS utf-8");
        for needle in [
            "--ctp-bg",
            "data-theme=dark",
            ".btn",
            ".tbl",
            ".badge",
            // ADR-116: govde fontu CaskaydiaMono kalir
            "CaskaydiaMono Nerd Font",
        ] {
            assert!(css.contains(needle), "derlenmiş CSS'te {needle} yok");
        }
    }
}
