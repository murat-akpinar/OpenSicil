// --- START FEATURE: operator-login ---
// Operator oturumu; uc kapi (AD bind, yerel break-glass, OIDC) ayni satiri
// uretir ve hangi kapidan acildigi `auth_source`'ta durur (ADR-095 madde 5).

use sqlx::PgPool;

use crate::i18n::Lang;
use crate::token::{generate_token, hash_token};

pub const SESSION_LIFETIME_HOURS: i64 = 8;

/// Oturumun acildigi kapi (ADR-095). Veritabaninda CHECK ile ayni uc deger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    Ad,
    Local,
    Oidc,
}

impl AuthSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthSource::Ad => "ad",
            AuthSource::Local => "local",
            AuthSource::Oidc => "oidc",
        }
    }

    fn parse(value: &str) -> Result<Self, sqlx::Error> {
        match value {
            "ad" => Ok(AuthSource::Ad),
            "local" => Ok(AuthSource::Local),
            "oidc" => Ok(AuthSource::Oidc),
            other => Err(sqlx::Error::Decode(
                format!("bilinmeyen auth_source: {other}").into(),
            )),
        }
    }
}

pub struct Operator {
    pub subject: String,
    pub username: String,
    pub email: String,
    pub authorities: Vec<String>,
    pub auth_source: AuthSource,
    /// Arayuz dili tercihi; oturum satirinda saklanir (ADR-089)
    pub lang: Lang,
}

// Cikis yapmadan birakilan oturumun satiri kendiliginden gitmez; suresi gecmis
// satirlar (operatorun adi, e-postasi, yetkileri ve token hash'i) suresiz
// durmasin diye her yeni oturumda temizlenir. Tablo yalnizca burada buyuyor.
async fn purge_expired_sessions(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM operator_sessions WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn create_session(pool: &PgPool, operator: &Operator) -> Result<String, sqlx::Error> {
    purge_expired_sessions(pool).await?;
    let token = generate_token();
    sqlx::query(
        "INSERT INTO operator_sessions \
         (token_hash, subject, username, email, authorities, auth_source, lang, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(hours => $8))",
    )
    .bind(hash_token(&token))
    .bind(&operator.subject)
    .bind(&operator.username)
    .bind(&operator.email)
    .bind(&operator.authorities)
    .bind(operator.auth_source.as_str())
    .bind(operator.lang.code())
    .bind(SESSION_LIFETIME_HOURS as i32)
    .execute(pool)
    .await?;
    Ok(token)
}

type SessionRow = (String, String, String, Vec<String>, String, String);

pub async fn validate_session(pool: &PgPool, token: &str) -> Result<Option<Operator>, sqlx::Error> {
    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT subject, username, email, authorities, auth_source, lang FROM operator_sessions \
         WHERE token_hash = $1 AND expires_at > now()",
    )
    .bind(hash_token(token))
    .fetch_optional(pool)
    .await?;
    let Some((subject, username, email, authorities, auth_source, lang)) = row else {
        return Ok(None);
    };
    Ok(Some(Operator {
        subject,
        username,
        email,
        authorities,
        auth_source: AuthSource::parse(&auth_source)?,
        lang: Lang::from_code(&lang),
    }))
}

/// Dil secicisi: tercih bu oturum icin kalicidir (ADR-089).
pub async fn set_lang(pool: &PgPool, token: &str, lang: Lang) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE operator_sessions SET lang = $2 WHERE token_hash = $1")
        .bind(hash_token(token))
        .bind(lang.code())
        .execute(pool)
        .await?;
    Ok(())
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
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
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

        // Cikis yapilmadan birakilan oturum: suresi gecince sonraki girisin
        // temizligine takilir, satir birikmez.
        let stale = create_session(&pool, &sample_operator()).await.unwrap();
        sqlx::query("UPDATE operator_sessions SET expires_at = now() - interval '1 minute'")
            .execute(&pool)
            .await
            .unwrap();
        create_session(&pool, &sample_operator()).await.unwrap();
        assert!(validate_session(&pool, &stale).await.unwrap().is_none());
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operator_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1, "yalnızca yeni oturum kalmalı");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
// --- END FEATURE: operator-login ---
