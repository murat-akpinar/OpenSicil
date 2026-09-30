// --- START FEATURE: oidc-login ---
// OIDC ile girmis operatorun oturumu; bootstrap_sessions'tan bagimsiz (ADR-073).

use sqlx::PgPool;

use crate::token::{generate_token, hash_token};

pub const SESSION_LIFETIME_HOURS: i64 = 8;

pub struct Operator {
    pub subject: String,
    pub username: String,
    pub email: String,
    pub authorities: Vec<String>,
}

pub async fn create_session(pool: &PgPool, operator: &Operator) -> Result<String, sqlx::Error> {
    let token = generate_token();
    sqlx::query(
        "INSERT INTO operator_sessions (token_hash, subject, username, email, authorities, expires_at) \
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(hours => $6))",
    )
    .bind(hash_token(&token))
    .bind(&operator.subject)
    .bind(&operator.username)
    .bind(&operator.email)
    .bind(&operator.authorities)
    .bind(SESSION_LIFETIME_HOURS as i32)
    .execute(pool)
    .await?;
    Ok(token)
}

pub async fn validate_session(pool: &PgPool, token: &str) -> Result<Option<Operator>, sqlx::Error> {
    let row: Option<(String, String, String, Vec<String>)> = sqlx::query_as(
        "SELECT subject, username, email, authorities FROM operator_sessions \
         WHERE token_hash = $1 AND expires_at > now()",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(subject, username, email, authorities)| Operator {
        subject,
        username,
        email,
        authorities,
    }))
}

pub async fn delete_session(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM operator_sessions WHERE token_hash = $1")
        .bind(hash_token(token))
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_operator() -> Operator {
        Operator {
            subject: "abc-123".to_string(),
            username: "test-admin".to_string(),
            email: "test-admin@example.org".to_string(),
            authorities: vec!["admin".to_string(), "hr".to_string()],
        }
    }

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    #[tokio::test]
    async fn validate_session_fails_when_db_unreachable() {
        let pool = lazy_unreachable_pool();
        assert!(validate_session(&pool, "herhangi-bir-token").await.is_err());
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn create_then_validate_then_delete_round_trip() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;

        let token = create_session(&pool, &sample_operator())
            .await
            .expect("oturum oluşturulamadı");

        let loaded = validate_session(&pool, &token)
            .await
            .unwrap()
            .expect("oturum bulunamadı");
        assert_eq!(loaded.username, "test-admin");
        assert_eq!(
            loaded.authorities,
            vec!["admin".to_string(), "hr".to_string()]
        );

        assert!(validate_session(&pool, "olmayan-token")
            .await
            .unwrap()
            .is_none());

        delete_session(&pool, &token).await.unwrap();
        assert!(validate_session(&pool, &token).await.unwrap().is_none());

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
// --- END FEATURE: oidc-login ---
