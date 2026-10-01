// --- START FEATURE: adoption ---
// Mevcut hesabin sahiplenilmesi (ADR-018/034/042/086): ipucundaki sAMAccountName ile aday
// bulunur, kurallar engine::adopt'ta uygulanir, baglanti gozlem modunda yazilir.

use ldap3::{ldap_escape, Ldap, Scope};
use sqlx::PgPool;

use crate::ad;
use crate::ad_account;
use crate::username::normalize_component;
use crate::writes::WriteError;

pub const ADOPTED_EVENT: &str = "ad.account.adopted";
pub const MANAGED_EVENT: &str = "ad.account.managed";

#[derive(Debug)]
pub struct Candidate {
    pub dn: String,
    pub guid: String,
    pub sam: String,
    pub upn: Option<String>,
    pub mail: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    /// Sicil no'nun eslendigi ozniteligin hedefteki degeri (eslenmemisse None)
    pub employee_value: Option<String>,
    pub admin_count: bool,
    pub member_of: Vec<String>,
}

pub async fn find_by_sam(
    ldap: &mut Ldap,
    base: &str,
    sam: &str,
    employee_attr: Option<&str>,
) -> Result<Option<Candidate>, WriteError> {
    let mut attrs = vec![
        "sAMAccountName",
        "userPrincipalName",
        "mail",
        "givenName",
        "sn",
        "adminCount",
        "memberOf",
    ];
    attrs.extend(employee_attr);
    let filter = format!("(&(objectClass=user)(sAMAccountName={}))", ldap_escape(sam));
    let found = ad::search(ldap, base, Scope::Subtree, &filter, &attrs).await?;
    let Some(entry) = found.into_iter().next() else {
        return Ok(None);
    };
    let text = |name: &str| entry.attrs.get(name).and_then(|v| v.first()).cloned();
    let guid = ad_account::guid_by_dn(ldap, &entry.dn).await?;
    Ok(Some(Candidate {
        guid,
        sam: text("sAMAccountName").unwrap_or_else(|| sam.to_string()),
        upn: text("userPrincipalName"),
        mail: text("mail"),
        given_name: text("givenName"),
        surname: text("sn"),
        employee_value: employee_attr.and_then(text),
        admin_count: text("adminCount").is_some_and(|v| v.trim() != "0"),
        member_of: entry.attrs.get("memberOf").cloned().unwrap_or_default(),
        dn: entry.dn,
    }))
}

// ADR-018 madde 5 / ADR-042: iki taraf da doluysa bastaki sifirlar ve bosluklar atilarak esit olmali.
pub fn employee_number_matches(target: Option<&str>, ours: Option<&str>) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches('0').to_string();
    match (target.map(norm), ours.map(norm)) {
        (Some(t), Some(o)) if !t.is_empty() && !o.is_empty() => t == o,
        _ => true,
    }
}

// ADR-042 madde 4: ADR-011 normallestirmesiyle ad-soyad karsilastirmasi; uyusmazlik uyaridir.
pub fn name_matches(
    target_given: Option<&str>,
    target_surname: Option<&str>,
    given: &str,
    surname: &str,
) -> bool {
    target_given.is_some_and(|g| normalize_component(g) == normalize_component(given))
        && target_surname.is_some_and(|s| normalize_component(s) == normalize_component(surname))
}

pub async fn linked_identity(
    pool: &PgPool,
    target: i64,
    guid: &str,
) -> Result<Option<i64>, String> {
    sqlx::query_scalar(
        "SELECT identity_id FROM account_links WHERE target_system_id = $1 AND external_id = $2",
    )
    .bind(target)
    .bind(guid)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("bağlantı sorgulanamadı: {e}"))
}

// Baglanti gozlem modunda; adlar kimlige yalnizca bossa yazilir (ADR-034); denetim satiri.
pub async fn link_observed(
    pool: &PgPool,
    identity_id: i64,
    target: i64,
    cand: &Candidate,
    name_mismatch: bool,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, \
         name_mismatch) VALUES ($1, $2, $3, 'adopted', 'observed', $4)",
    )
    .bind(identity_id)
    .bind(target)
    .bind(&cand.guid)
    .bind(name_mismatch)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("bağlantı yazılamadı: {e}"))?;
    sqlx::query(
        "UPDATE identities SET username = COALESCE(username, $2), email = COALESCE(email, $3), \
         upn = COALESCE(upn, $4) WHERE id = $1",
    )
    .bind(identity_id)
    .bind(&cand.sam)
    .bind(&cand.mail)
    .bind(&cand.upn)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("kimlik adları yazılamadı: {e}"))?;
    sqlx::query(
        "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
         VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(ADOPTED_EVENT)
    .bind(identity_id)
    .bind(target)
    .bind(format!(
        "{{\"dn\":\"{}\",\"name_mismatch\":{name_mismatch}}}",
        crate::writes::json_quote(&cand.dn)
    ))
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("denetim satırı yazılamadı: {e}"))?;
    tx.commit().await.map_err(|e| e.to_string())
}

// ADR-018/087 yonetime alma: operator farki gorup onaylayinca backend yalnizca
// istek kolonunu yazar, modu worker cevirir (docs/03). Istek tuketilir: ikinci
// is yeniden cevirmeye calismaz. Doner: mod bu cagriyla yonetilene gecti mi.
pub async fn take_over(pool: &PgPool, identity_id: i64, target: i64) -> Result<bool, String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let done = sqlx::query(
        "UPDATE account_links SET mode = 'managed', manage_requested_at = NULL \
         WHERE identity_id = $1 AND target_system_id = $2 AND mode = 'observed' \
         AND manage_requested_at IS NOT NULL",
    )
    .bind(identity_id)
    .bind(target)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("yönetime alma yazılamadı: {e}"))?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
         VALUES ($1, $2, $3, '{}'::jsonb)",
    )
    .bind(MANAGED_EVENT)
    .bind(identity_id)
    .bind(target)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("denetim satırı yazılamadı: {e}"))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(true)
}
// --- END FEATURE: adoption ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn employee_number_ignores_leading_zeros_and_blanks() {
        assert!(employee_number_matches(Some("00123"), Some(" 123 ")));
        assert!(!employee_number_matches(Some("124"), Some("123")));
        assert!(employee_number_matches(None, Some("123")));
        assert!(employee_number_matches(Some(""), Some("123")));
        assert!(employee_number_matches(Some("123"), None));
    }

    // ADR-087: istek tuketilir, mod bir kez cevrilir, denetim satiri yazilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn take_over_flips_mode_once_and_audits() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             VALUES ($1, $2, 'guid-1', 'adopted', 'observed')",
        )
        .bind(seed.identity)
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();

        // istek yoksa cevrilmez
        assert!(!take_over(&pool, seed.identity, seed.ad).await.unwrap());
        sqlx::query("UPDATE account_links SET manage_requested_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        assert!(take_over(&pool, seed.identity, seed.ad).await.unwrap());
        let (mode, consumed): (String, bool) =
            sqlx::query_as("SELECT mode, manage_requested_at IS NULL FROM account_links")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((mode.as_str(), consumed), ("managed", true));
        assert!(
            !take_over(&pool, seed.identity, seed.ad).await.unwrap(),
            "istek tüketildi: ikinci çağrı mod çevirmez"
        );
        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE event_type = $1")
                .bind(MANAGED_EVENT)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(events, 1);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[test]
    fn name_comparison_uses_adr_011_normalization() {
        assert!(name_matches(Some("AYŞE"), Some("Yılmaz"), "Ayşe", "YILMAZ"));
        assert!(!name_matches(
            Some("Ayşe"),
            Some("Yilmaz"),
            "Ayşe",
            "Yılmazoğlu"
        ));
        assert!(!name_matches(None, Some("Yılmaz"), "Ayşe", "Yılmaz"));
    }
}
