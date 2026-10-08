use axum::extract::State;
use axum::http::StatusCode;
use sqlx::PgPool;

// Bağımlılık (veritabanı) kontrol edilir; sürüm ve yapılandırma sızdırmaz (.claude/rules/docker.md).
pub async fn health(State(pool): State<PgPool>) -> StatusCode {
    match sqlx::query("SELECT 1").execute(&pool).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            log_error!("health: veritabanı kontrolü başarısız: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_returns_503_when_db_unreachable() {
        // 127.0.0.1:1 hicbir servis dinlemez, baglanti hemen reddedilir; test
        // suitin 30 sn'lik varsayilan zaman asimini beklememesi icin kisaltilir
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı");

        let status = health(State(pool)).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
