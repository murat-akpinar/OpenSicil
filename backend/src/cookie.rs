use axum::http::HeaderMap;

// Tek oturum cerezi: uc giris kapisi da ayni cerezi kullanir (ADR-095 madde 5).
pub const OPERATOR_SESSION_COOKIE_NAME: &str = "opensicil_operator_session";

// HttpOnly: JS/XSS okuyamaz. Secure: yalnizca TLS uzerinden gider (ADR-066, nginx'te sonlanir).
// SameSite=Strict: cross-site istekte hic gonderilmez, ayri bir CSRF token'ina gerek birakmaz.
// Token URL'e, log'a (logging.rs yalnizca method/path/status/ip yazar) ya da body'ye hicbir zaman konmaz.
pub fn set_cookie_header(name: &str, value: &str, max_age_secs: i64) -> String {
    format!("{name}={value}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={max_age_secs}")
}

pub fn clear_cookie_header(name: &str) -> String {
    format!("{name}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0")
}

pub fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn set_cookie_header_carries_all_required_flags() {
        let header = set_cookie_header("s", "tok", 3600);
        for flag in [
            "HttpOnly",
            "Secure",
            "SameSite=Strict",
            "Path=/",
            "Max-Age=3600",
        ] {
            assert!(header.contains(flag), "eksik bayrak: {flag} ({header})");
        }
    }

    #[test]
    fn get_cookie_finds_named_value_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("a=1; opensicil_operator_session=abc123; b=2"),
        );
        assert_eq!(
            get_cookie(&headers, OPERATOR_SESSION_COOKIE_NAME),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn get_cookie_returns_none_when_missing() {
        let headers = HeaderMap::new();
        assert_eq!(get_cookie(&headers, OPERATOR_SESSION_COOKIE_NAME), None);
    }
}
