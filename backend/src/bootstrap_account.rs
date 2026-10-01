use sqlx::PgPool;

// ADR-068: migration ile seed edilen tek yerel hesabin adi; denetim kaydinda aktor.
pub const BOOTSTRAP_USERNAME: &str = "admin";

// ADR-095 madde 3: yerel kapinin kaba kuvvet korumasi. AD kapisinda kilitleme
// AD'nin kendi politikasi; buradaki sayac yalnizca bu hesap icin.
pub const MAX_FAILED_ATTEMPTS: i32 = 5;
pub const LOCK_MINUTES: i64 = 15;

#[derive(Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    Ok,
    BadCredentials,
    /// Hesap kilitli; kalan dakika (yukari yuvarlanmis).
    Locked(i64),
}

struct AccountRow {
    username: String,
    password_hash: String,
    failed_attempts: i32,
    lock_seconds_left: i64,
}

async fn fetch(pool: &PgPool) -> Result<AccountRow, sqlx::Error> {
    let row: (String, String, i32, i64) = sqlx::query_as(
        "SELECT username, password_hash, failed_attempts, \
         GREATEST(0, EXTRACT(EPOCH FROM COALESCE(locked_until, now()) - now()))::bigint \
         FROM bootstrap_account WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await?;
    Ok(AccountRow {
        username: row.0,
        password_hash: row.1,
        failed_attempts: row.2,
        lock_seconds_left: row.3,
    })
}

/// Kalan kilit suresini dakikaya yuvarlar: 1 saniye kalsa da "1 dakika" denir.
pub fn lock_minutes_left(seconds: i64) -> i64 {
    seconds.div_euclid(60) + i64::from(seconds.rem_euclid(60) > 0)
}

// ADR-068: kullanici adi + parola dogrulanir; basarisiz denemede hangisinin
// yanlis oldugu ayirt edilmez (kullanici adi taramasina karsi). Yanlis kullanici
// adi da deneme sayilir, yoksa sayac bos bir adla atlatilabilirdi.
pub async fn verify_login(
    pool: &PgPool,
    username: &str,
    password: &str,
) -> Result<LoginOutcome, sqlx::Error> {
    let account = fetch(pool).await?;
    if account.lock_seconds_left > 0 {
        return Ok(LoginOutcome::Locked(lock_minutes_left(
            account.lock_seconds_left,
        )));
    }
    if account.username == username
        && crate::auth::verify_password(password, &account.password_hash)
    {
        clear_failures(pool).await?;
        return Ok(LoginOutcome::Ok);
    }
    register_failure(pool).await?;
    if account.failed_attempts + 1 >= MAX_FAILED_ATTEMPTS {
        return Ok(LoginOutcome::Locked(LOCK_MINUTES));
    }
    Ok(LoginOutcome::BadCredentials)
}

async fn clear_failures(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE bootstrap_account SET failed_attempts = 0, locked_until = NULL WHERE id = TRUE",
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Sayac veritabaninda artirilir: iki es zamanli deneme birbirinin artisini yemez.
async fn register_failure(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE bootstrap_account SET failed_attempts = failed_attempts + 1, \
         locked_until = CASE WHEN failed_attempts + 1 >= $1 \
         THEN now() + make_interval(mins => $2) ELSE NULL END \
         WHERE id = TRUE",
    )
    .bind(MAX_FAILED_ATTEMPTS)
    .bind(LOCK_MINUTES as i32)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn must_change_password(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let row: (bool,) =
        sqlx::query_as("SELECT must_change_password FROM bootstrap_account WHERE id = TRUE")
            .fetch_one(pool)
            .await?;
    Ok(row.0)
}

pub async fn set_password(pool: &PgPool, new_password: &str) -> Result<(), String> {
    let hash = crate::auth::hash_password(new_password)?;
    sqlx::query(
        "UPDATE bootstrap_account SET password_hash = $1, must_change_password = FALSE, \
         failed_attempts = 0, locked_until = NULL WHERE id = TRUE",
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

        let check = |u: &'static str, p: &'static str| verify_login(&pool, u, p);
        assert_eq!(check("admin", "admin").await.unwrap(), LoginOutcome::Ok);
        assert_eq!(
            check("admin", "yanlis-parola").await.unwrap(),
            LoginOutcome::BadCredentials
        );
        assert_eq!(
            check("baska-kullanici", "admin").await.unwrap(),
            LoginOutcome::BadCredentials
        );
        assert!(must_change_password(&pool).await.unwrap());

        set_password(&pool, "yeni-guclu-parola").await.unwrap();
        assert!(!must_change_password(&pool).await.unwrap());
        assert_eq!(
            check("admin", "yeni-guclu-parola").await.unwrap(),
            LoginOutcome::Ok
        );
        assert_eq!(
            check("admin", "admin").await.unwrap(),
            LoginOutcome::BadCredentials
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[test]
    fn lock_minutes_left_rounds_any_remaining_second_up() {
        assert_eq!(lock_minutes_left(0), 0);
        assert_eq!(lock_minutes_left(1), 1);
        assert_eq!(lock_minutes_left(60), 1);
        assert_eq!(lock_minutes_left(61), 2);
        assert_eq!(lock_minutes_left(15 * 60), 15);
    }

    // ADR-095 madde 3: besinci basarisiz denemede kilit, dogru parola bile
    // gecmez; kilit suresi gecince ayni parola kabul edilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn five_failed_attempts_lock_the_account_for_fifteen_minutes() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::migrate::seed_bootstrap_account(&pool)
            .await
            .expect("bootstrap hesabı seed edilemedi");

        for attempt in 1..MAX_FAILED_ATTEMPTS {
            assert_eq!(
                verify_login(&pool, "admin", "yanlis").await.unwrap(),
                LoginOutcome::BadCredentials,
                "{attempt}. deneme henüz kilitlememeli"
            );
        }
        assert_eq!(
            verify_login(&pool, "admin", "yanlis").await.unwrap(),
            LoginOutcome::Locked(LOCK_MINUTES)
        );
        assert_eq!(
            verify_login(&pool, "admin", "admin").await.unwrap(),
            LoginOutcome::Locked(LOCK_MINUTES),
            "kilitliyken doğru parola da girmez"
        );

        sqlx::query("UPDATE bootstrap_account SET locked_until = now() - interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            verify_login(&pool, "admin", "admin").await.unwrap(),
            LoginOutcome::Ok,
            "kilit süresi dolunca doğru parola geçer"
        );
        // Basarili girise sayac sifirlanir: tek yanlis deneme yeniden kilitlemez.
        assert_eq!(
            verify_login(&pool, "admin", "yanlis").await.unwrap(),
            LoginOutcome::BadCredentials
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
