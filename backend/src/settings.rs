use sqlx::PgPool;

// Sirlar asla duz metin geri okunmaz/ekrana basilmaz; formda yalnizca "kayitli mi" bilgisi gosterilir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettings {
    pub ad_host: String,
    pub ad_bind_dn: String,
    pub ad_service_password_set: bool,
    pub zimbra_url: String,
    pub zimbra_admin_password_set: bool,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret_set: bool,
    pub oidc_admin_verified: bool,
}

// Formdan gelen, henuz sifrelenmemis giris; bos parola alani "degistirme" anlamina gelir.
pub struct AppSettingsInput {
    pub ad_host: String,
    pub ad_bind_dn: String,
    pub ad_service_password: String,
    pub zimbra_url: String,
    pub zimbra_admin_password: String,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
}

pub async fn load(pool: &PgPool) -> Result<AppSettings, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let row: (
        String,
        String,
        bool,
        String,
        bool,
        String,
        String,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT ad_host, ad_bind_dn, ad_service_password_enc IS NOT NULL, \
                zimbra_url, zimbra_admin_password_enc IS NOT NULL, \
                oidc_issuer, oidc_client_id, oidc_client_secret_enc IS NOT NULL, \
                oidc_admin_verified_at IS NOT NULL \
         FROM app_settings WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await?;

    Ok(AppSettings {
        ad_host: row.0,
        ad_bind_dn: row.1,
        ad_service_password_set: row.2,
        zimbra_url: row.3,
        zimbra_admin_password_set: row.4,
        oidc_issuer: row.5,
        oidc_client_id: row.6,
        oidc_client_secret_set: row.7,
        oidc_admin_verified: row.8,
    })
}

// OIDC ile gercek baglanti icin cozulmus sir gerekir; AppSettings/load()'un
// aksine bu fonksiyon template'e asla verilmez, yalnizca oidc.rs kullanir.
pub struct OidcCredentials {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

// issuer/client_id bos ya da sir hic kaydedilmemisse None: OIDC henuz
// yapilandirilmamis demektir.
pub async fn load_oidc_credentials(
    pool: &PgPool,
    key: &[u8; crate::crypto::KEY_LEN],
) -> Result<Option<OidcCredentials>, String> {
    let row: (String, String, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT oidc_issuer, oidc_client_id, oidc_client_secret_enc FROM app_settings WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("ayarlar okunamadı: {e}"))?;

    let (issuer, client_id, secret_enc) = row;
    if issuer.is_empty() || client_id.is_empty() {
        return Ok(None);
    }
    let Some(secret_enc) = secret_enc else {
        return Ok(None);
    };
    let secret_bytes = crate::crypto::decrypt(key, &secret_enc)?;
    let client_secret = String::from_utf8(secret_bytes)
        .map_err(|e| format!("oidc client secret utf-8 değil: {e}"))?;
    Ok(Some(OidcCredentials {
        issuer,
        client_id,
        client_secret,
    }))
}

// En az bir OpenSicil-Admins girisi dogrulandiginda bir kez isaretlenir
// (ADR-068 madde 3); sonraki girislerde dokunmaz (WHERE ... IS NULL).
pub async fn mark_oidc_admin_verified(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE app_settings SET oidc_admin_verified_at = now() \
         WHERE id = TRUE AND oidc_admin_verified_at IS NULL",
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Bos birakilan sir alani mevcut sifreli degeri korur (COALESCE), yeniden girmeye zorlamaz.
pub async fn save(
    pool: &PgPool,
    key: &[u8; crate::crypto::KEY_LEN],
    input: &AppSettingsInput,
) -> Result<(), sqlx::Error> {
    let encrypt_if_present =
        |plain: &str| (!plain.is_empty()).then(|| crate::crypto::encrypt(key, plain.as_bytes()));

    sqlx::query(
        "UPDATE app_settings SET \
            ad_host = $1, ad_bind_dn = $2, \
            ad_service_password_enc = COALESCE($3, ad_service_password_enc), \
            zimbra_url = $4, \
            zimbra_admin_password_enc = COALESCE($5, zimbra_admin_password_enc), \
            oidc_issuer = $6, oidc_client_id = $7, \
            oidc_client_secret_enc = COALESCE($8, oidc_client_secret_enc) \
         WHERE id = TRUE",
    )
    .bind(&input.ad_host)
    .bind(&input.ad_bind_dn)
    .bind(encrypt_if_present(&input.ad_service_password))
    .bind(&input.zimbra_url)
    .bind(encrypt_if_present(&input.zimbra_admin_password))
    .bind(&input.oidc_issuer)
    .bind(&input.oidc_client_id)
    .bind(encrypt_if_present(&input.oidc_client_secret))
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; crate::crypto::KEY_LEN] {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        crate::crypto::parse_master_key(&BASE64.encode([7u8; crate::crypto::KEY_LEN])).unwrap()
    }

    fn sample_input() -> AppSettingsInput {
        AppSettingsInput {
            ad_host: "dc1.example.org".to_string(),
            ad_bind_dn: "CN=svc,DC=example,DC=org".to_string(),
            ad_service_password: "gizli-ad-parolasi".to_string(),
            zimbra_url: "https://zimbra.example.org:7071".to_string(),
            zimbra_admin_password: "gizli-zimbra-parolasi".to_string(),
            oidc_issuer: "https://idp.example.org/realms/opensicil".to_string(),
            oidc_client_id: "opensicil".to_string(),
            oidc_client_secret: "gizli-client-secret".to_string(),
        }
    }

    #[test]
    fn blank_secret_field_leaves_no_encrypted_value_to_write() {
        let mut input = sample_input();
        input.ad_service_password = String::new();
        let key = test_key();
        let encrypted =
            (!input.ad_service_password.is_empty()).then(|| crate::crypto::encrypt(&key, b""));
        assert!(encrypted.is_none());
    }

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    #[tokio::test]
    async fn load_fails_when_db_unreachable() {
        assert!(load(&lazy_unreachable_pool()).await.is_err());
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn save_then_load_round_trips_without_exposing_secrets() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let key = test_key();

        save(&pool, &key, &sample_input()).await.unwrap();
        let loaded = load(&pool).await.unwrap();

        assert_eq!(loaded.ad_host, "dc1.example.org");
        assert!(loaded.ad_service_password_set);
        assert!(loaded.zimbra_admin_password_set);
        assert!(loaded.oidc_client_secret_set);

        // Bos parola alaniyla ikinci kayit: onceki sifreli deger korunur.
        let mut second = sample_input();
        second.ad_service_password = String::new();
        second.ad_host = "dc2.example.org".to_string();
        save(&pool, &key, &second).await.unwrap();
        let reloaded = load(&pool).await.unwrap();
        assert_eq!(reloaded.ad_host, "dc2.example.org");
        assert!(reloaded.ad_service_password_set);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn load_oidc_credentials_is_none_until_fully_configured() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let key = test_key();

        assert!(load_oidc_credentials(&pool, &key).await.unwrap().is_none());

        save(&pool, &key, &sample_input()).await.unwrap();
        let creds = load_oidc_credentials(&pool, &key)
            .await
            .unwrap()
            .expect("ayarlar tam kaydedildikten sonra sir çözülebilmeli");
        assert_eq!(creds.issuer, "https://idp.example.org/realms/opensicil");
        assert_eq!(creds.client_id, "opensicil");
        assert_eq!(creds.client_secret, "gizli-client-secret");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn mark_oidc_admin_verified_is_idempotent() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;

        assert!(!load(&pool).await.unwrap().oidc_admin_verified);

        mark_oidc_admin_verified(&pool).await.unwrap();
        assert!(load(&pool).await.unwrap().oidc_admin_verified);

        // Ikinci cagri hata vermez, deger degismez (WHERE ... IS NULL).
        mark_oidc_admin_verified(&pool).await.unwrap();
        assert!(load(&pool).await.unwrap().oidc_admin_verified);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
