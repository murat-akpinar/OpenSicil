use askama::Template;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::i18n::Lang;

// --- START FEATURE: error-pages ---

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    lang: Lang,
    code: u16,
    title_key: &'static str,
    hint_key: &'static str,
}

// Govde yalnizca durum kodu + iki hazir metin: yol, SQL ve surum disari cikmaz
// (.claude/rules/security.md "Hata yanitlari ic bilgi sizdirmaz").
fn texts(status: StatusCode) -> (&'static str, &'static str) {
    match status {
        StatusCode::FORBIDDEN => ("err.page.forbidden", "err.no_permission"),
        StatusCode::NOT_FOUND => ("err.page.not_found", "err.page.not_found_hint"),
        StatusCode::METHOD_NOT_ALLOWED => ("err.page.method", "err.page.method_hint"),
        StatusCode::TOO_MANY_REQUESTS => ("err.page.too_many", "err.page.too_many_hint"),
        _ => ("err.page.server", "err.page.server_hint"),
    }
}

pub(crate) fn page(lang: Lang, status: StatusCode) -> Response {
    page_with_hint(lang, status, texts(status).1)
}

// Ayni sayfa, baska bir aciklamayla: ayrilmis operatorun reddi 403'tur ama
// nedeni "yetkin yok" degildir (ADR-059).
pub(crate) fn page_with_hint(lang: Lang, status: StatusCode, hint_key: &'static str) -> Response {
    let body = crate::web::render(&ErrorTemplate {
        lang,
        code: status.as_u16(),
        title_key: texts(status).0,
        hint_key,
    });
    (status, body).into_response()
}

// Govdesiz hata yanitini kabugun icindeki sayfaya cevirir. Cagiran tarafta
// `StatusCode::X.into_response()` yazmak yeterli kalsin diye ara katman:
// kirk cagri yerinde kalir, sayfa tek yerde cizilir. Govdesi olan yanita
// (yonlendirme, kendi metnini yazan 403) dokunulmaz; `/api/*` makine ucudur,
// HTML almaz.
pub(crate) async fn error_page(req: Request, next: Next) -> Response {
    let lang = Lang::from_headers(req.headers());
    let is_api = req.uri().path().starts_with("/api/");
    let res = next.run(req).await;
    let bare = res.headers().get(header::CONTENT_TYPE).is_none();
    if is_api || !bare || !(res.status().is_client_error() || res.status().is_server_error()) {
        return res;
    }
    let status = res.status();
    let (parts, _) = res.into_parts();
    let mut page = page_with_hint(lang, status, texts(status).1);
    // Govde disindaki her sey korunur: cerez temizligi, `allow`, yonlendirme basliklari
    for (name, value) in parts.headers.iter() {
        if name != header::CONTENT_LENGTH {
            page.headers_mut().insert(name, value.clone());
        }
    }
    page
}
// --- END FEATURE: error-pages ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn every_status_has_a_title_and_a_hint_in_both_languages() {
        // Sablon anahtarlari degiskenden geliyor; i18n taramasi bunlari gormez.
        let statuses = [
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::METHOD_NOT_ALLOWED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ];
        for lang in [Lang::Tr, Lang::En] {
            for status in statuses {
                let (title, hint) = texts(status);
                for key in [title, hint, "err.back_home"] {
                    assert_ne!(lang.t(key), key, "{lang:?} dilinde {key} yok");
                }
            }
        }
    }

    #[test]
    fn the_page_carries_the_status_code_and_no_internals() {
        let res = page(Lang::Tr, StatusCode::NOT_FOUND);
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
    }
}
