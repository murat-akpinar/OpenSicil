use std::time::Instant;

use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;

// Tüm trafik nginx'ten geçer (.claude/rules/docker.md); doğrudan TCP eşi her
// zaman nginx konteyneridir, gerçek istemci IP'si X-Forwarded-For/X-Real-IP'dedir.
pub async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // healthcheck her 10 sn'de bir gelir, log'u anlamsızca doldurur
    if path == "/api/health" {
        return next.run(req).await;
    }

    let client_ip = real_ip(req.headers());
    let start = Instant::now();

    let response = next.run(req).await;

    let status = response.status().as_u16();
    let elapsed_ms = start.elapsed().as_millis();
    println!("{method} {path} {status} {elapsed_ms}ms ip={client_ip}");

    response
}

fn real_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .unwrap_or("-")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn prefers_first_x_forwarded_for_entry() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.5, 10.0.0.2"),
        );
        assert_eq!(real_ip(&headers), "203.0.113.5");
    }

    #[test]
    fn falls_back_to_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.7"));
        assert_eq!(real_ip(&headers), "198.51.100.7");
    }

    #[test]
    fn falls_back_to_dash_when_missing() {
        let headers = HeaderMap::new();
        assert_eq!(real_ip(&headers), "-");
    }
}
