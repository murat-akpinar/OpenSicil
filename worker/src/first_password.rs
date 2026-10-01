// --- START FEATURE: first-password ---
// Ilk parola teslimi, worker tarafi (ADR-019/036/046/085): bekleyen istek, kullanilmamis
// hesap kurali ve sonucun yazilmasi. LDAP yazmasi engine::issue_first_password'da.

use sqlx::PgPool;

use crate::model::LinkRow;

pub const REJECT_USED: &str = "hesap kullanılmış; parolayı AD'nin kendi süreciyle sıfırlayın";
pub const REJECT_DRY_RUN: &str = "kuru çalıştırma açık, parola yazılmadı (ADR-054)";

pub async fn pending(pool: &PgPool, identity_id: i64, target: i64) -> Result<Option<i64>, String> {
    sqlx::query_scalar(
        "SELECT id FROM first_passwords WHERE identity_id = $1 AND target_system_id = $2 \
         AND issued_at IS NULL AND error IS NULL ORDER BY id LIMIT 1",
    )
    .bind(identity_id)
    .bind(target)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("ilk parola isteği okunamadı: {e}"))
}

// ADR-046: hic kullanilmamis (lastLogonTimestamp bos) ya da ayrilista sifirlanmis; her iki
// yolda pwdLastSet kontrolu: 0 (isaret acik) ya da worker'in son yazdigi degere esit (kapali).
pub fn account_unused(
    last_logon: Option<&str>,
    pwd_last_set: Option<&str>,
    link: &LinkRow,
) -> bool {
    let never_logged_on = last_logon.is_none_or(|v| v == "0");
    let pwd_untouched = pwd_last_set.is_none_or(|v| v == "0")
        || pwd_last_set == link.first_password_pwd_last_set.as_deref();
    (never_logged_on || link.password_reset_at_departure) && pwd_untouched
}

pub async fn reject(pool: &PgPool, id: i64, reason: &str) -> Result<(), String> {
    sqlx::query("UPDATE first_passwords SET error = $2 WHERE id = $1")
        .bind(id)
        .bind(reason)
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|e| format!("ilk parola reddi yazılamadı: {e}"))
}

// Sifreli parola istege, pwdLastSet damgasi baglantiya; ayrilis sifirlama isareti temizlenir.
pub async fn issue(
    pool: &PgPool,
    key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
    password: &str,
    pwd_last_set: Option<&str>,
) -> Result<(), String> {
    let enc = crate::crypto::encrypt_versioned(key, password.as_bytes());
    sqlx::query(
        "WITH r AS (UPDATE first_passwords SET password_enc = $2, issued_at = now() \
                    WHERE id = $1 RETURNING identity_id, target_system_id) \
         UPDATE account_links l SET first_password_pwd_last_set = $3, \
           password_reset_at_departure = FALSE \
         FROM r WHERE l.identity_id = r.identity_id AND l.target_system_id = r.target_system_id",
    )
    .bind(id)
    .bind(enc)
    .bind(pwd_last_set)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| format!("ilk parola yazılamadı: {e}"))
}
// --- END FEATURE: first-password ---

#[cfg(test)]
mod tests {
    use super::*;

    fn link(reset: bool, stored: Option<&str>) -> LinkRow {
        LinkRow {
            external_id: "g".to_string(),
            applied_state: None,
            password_reset_at_departure: reset,
            first_password_pwd_last_set: stored.map(str::to_string),
        }
    }

    #[test]
    fn unused_rule_follows_adr_046() {
        // taze hesap
        assert!(account_unused(None, Some("0"), &link(false, None)));
        // giris yapilmis: pwdLastSet 0 olsa bile (yardim masasi "sonraki giriste degistir")
        assert!(!account_unused(Some("133"), Some("0"), &link(false, None)));
        // ayrilista sifirlanmis, geri alinmis
        assert!(account_unused(Some("133"), Some("0"), &link(true, None)));
        // kapali mod: damga worker'in yazdigina esit
        assert!(account_unused(None, Some("555"), &link(false, Some("555"))));
        // kapali mod: kisi parolasini degistirmis
        assert!(!account_unused(
            None,
            Some("777"),
            &link(false, Some("555"))
        ));
        // ayrilista sifirlanmis ama o zamandan beri parola degismis
        assert!(!account_unused(Some("133"), Some("777"), &link(true, None)));
    }

    // ADR-046/085: verilince ayrilis sifirlama isareti temizlenir, damga baglantiya yazilir,
    // istek bekleyenden cikar; red nedeni istege yazilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn issue_clears_departure_flag_and_reject_records_reason() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, \
             applied_state, password_reset_at_departure) \
             VALUES ($1, $2, 'guid-1', 'provisioned', 'managed', 'active', TRUE)",
        )
        .bind(seed.identity)
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();
        let request = || {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO first_passwords (identity_id, target_system_id, requested_by) \
                     VALUES ($1, $2, 'ik') RETURNING id",
                )
                .bind(seed.identity)
                .bind(seed.ad)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(pending(&pool, seed.identity, seed.ad).await.unwrap(), None);
        let first = request().await;
        assert_eq!(
            pending(&pool, seed.identity, seed.ad).await.unwrap(),
            Some(first)
        );
        let key = [5u8; crate::crypto::KEY_LEN];
        issue(&pool, &key, first, "Kf7m-Rq2x-Wn8d-Tz4p", Some("555"))
            .await
            .unwrap();
        let (enc, issued): (Vec<u8>, bool) = sqlx::query_as(
            "SELECT password_enc, issued_at IS NOT NULL FROM first_passwords WHERE id = $1",
        )
        .bind(first)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(issued);
        assert_eq!(
            crate::crypto::decrypt_versioned(&key, &enc).unwrap(),
            b"Kf7m-Rq2x-Wn8d-Tz4p"
        );
        let (flag, stamp): (bool, Option<String>) = sqlx::query_as(
            "SELECT password_reset_at_departure, first_password_pwd_last_set \
             FROM account_links WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((flag, stamp.as_deref()), (false, Some("555")));
        assert_eq!(pending(&pool, seed.identity, seed.ad).await.unwrap(), None);

        let second = request().await;
        reject(&pool, second, REJECT_USED).await.unwrap();
        let error: Option<String> =
            sqlx::query_scalar("SELECT error FROM first_passwords WHERE id = $1")
                .bind(second)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(error.as_deref(), Some(REJECT_USED));
        assert_eq!(pending(&pool, seed.identity, seed.ad).await.unwrap(), None);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
