use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use base64::Engine;

pub const SESSION_LIFETIME_HOURS: i64 = 8;

// DB'de token'in kendisi degil hash'i tutulur: veritabani sizarsa cerezler tek basina ise yaramaz.
fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    BASE64_URL.encode(bytes)
}

pub async fn create_session(pool: &PgPool) -> Result<String, sqlx::Error> {
    let token = generate_token();
    sqlx::query(
        "INSERT INTO bootstrap_sessions (token_hash, expires_at) \
         VALUES ($1, now() + make_interval(hours => $2))",
    )
    .bind(hash_token(&token))
    .bind(SESSION_LIFETIME_HOURS as i32)
    .execute(pool)
    .await?;
    Ok(token)
}

pub async fn validate_session(pool: &PgPool, token: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT FROM bootstrap_sessions WHERE token_hash = $1 AND expires_at > now())",
    )
    .bind(hash_token(token))
    .fetch_one(pool)
    .await
}

pub async fn delete_session(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM bootstrap_sessions WHERE token_hash = $1")
        .bind(hash_token(token))
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_is_deterministic_and_distinct() {
        assert_eq!(hash_token("ayni"), hash_token("ayni"));
        assert_ne!(hash_token("bir"), hash_token("iki"));
    }

    #[test]
    fn generate_token_is_not_repeated() {
        assert_ne!(generate_token(), generate_token());
    }

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn create_then_validate_then_delete_round_trip() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;

        let token = create_session(&pool).await.expect("oturum oluşturulamadı");
        assert!(validate_session(&pool, &token).await.unwrap());
        assert!(!validate_session(&pool, "olmayan-token").await.unwrap());

        delete_session(&pool, &token).await.unwrap();
        assert!(!validate_session(&pool, &token).await.unwrap());

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    async fn validate_session_fails_when_db_unreachable() {
        let pool = lazy_unreachable_pool();
        assert!(validate_session(&pool, "herhangi-bir-token").await.is_err());
    }
}
