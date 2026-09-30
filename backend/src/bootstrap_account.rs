use sqlx::PgPool;

// ADR-068: migration ile seed edilen tek yerel hesabin adi; denetim kaydinda aktor.
pub const BOOTSTRAP_USERNAME: &str = "admin";

struct AccountRow {
    username: String,
    password_hash: String,
    must_change_password: bool,
}

async fn fetch(pool: &PgPool) -> Result<AccountRow, sqlx::Error> {
    let row: (String, String, bool) = sqlx::query_as(
        "SELECT username, password_hash, must_change_password FROM bootstrap_account WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await?;
    Ok(AccountRow {
        username: row.0,
        password_hash: row.1,
        must_change_password: row.2,
    })
}

// ADR-068: kullanici adi + parola dogrulanir; basarisiz denemede hangisinin
// yanlis oldugu ayirt edilmez (kullanici adi taramasina karsi).
pub async fn verify_login(
    pool: &PgPool,
    username: &str,
    password: &str,
) -> Result<bool, sqlx::Error> {
    let account = fetch(pool).await?;
    Ok(account.username == username
        && crate::auth::verify_password(password, &account.password_hash))
}

pub async fn must_change_password(pool: &PgPool) -> Result<bool, sqlx::Error> {
    Ok(fetch(pool).await?.must_change_password)
}

pub async fn set_password(pool: &PgPool, new_password: &str) -> Result<(), String> {
    let hash = crate::auth::hash_password(new_password)?;
    sqlx::query(
        "UPDATE bootstrap_account SET password_hash = $1, must_change_password = FALSE WHERE id = TRUE",
    )
    .bind(hash)
    .execute(pool)
    .await
    .map_err(|e| format!("parola güncellenemedi: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    #[tokio::test]
    async fn verify_login_fails_when_db_unreachable() {
        assert!(verify_login(&lazy_unreachable_pool(), "admin", "admin")
            .await
            .is_err());
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn seeded_admin_must_change_password_on_first_login() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::migrate::seed_bootstrap_account(&pool)
            .await
            .expect("bootstrap hesabı seed edilemedi");

        assert!(verify_login(&pool, "admin", "admin").await.unwrap());
        assert!(!verify_login(&pool, "admin", "yanlis-parola").await.unwrap());
        assert!(!verify_login(&pool, "baska-kullanici", "admin")
            .await
            .unwrap());
        assert!(must_change_password(&pool).await.unwrap());

        set_password(&pool, "yeni-guclu-parola").await.unwrap();
        assert!(!must_change_password(&pool).await.unwrap());
        assert!(verify_login(&pool, "admin", "yeni-guclu-parola")
            .await
            .unwrap());
        assert!(!verify_login(&pool, "admin", "admin").await.unwrap());

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
