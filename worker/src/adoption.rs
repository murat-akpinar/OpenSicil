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
/// ADR-122: dizinde bulunamayan hesabin baglantisi kaldirildi
pub const UNLINKED_EVENT: &str = "ad.account.unlinked";

/// ADR-122: operatorun istedigi kaldirma. Cagiran, hesabin dizinde GERCEKTEN
/// olmadigini dogrulamis olmali — burada yalnizca satir silinir ve denetime
/// olu `external_id` yazilir. Baglantinin tasidigi bayraklar (ilk parola,
/// ayrilis parolasi, iptal dogrulamasi) o hesaba aitti, onunla birlikte gider.
pub async fn remove_link(
    pool: &sqlx::PgPool,
    identity_id: i64,
    target: i64,
    external_id: &str,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM account_links WHERE identity_id = $1 AND target_system_id = $2")
        .bind(identity_id)
        .bind(target)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("bağlantı kaldırılamadı: {e}"))?;
    sqlx::query(
        "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
         VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(UNLINKED_EVENT)
    .bind(identity_id)
    .bind(target)
    .bind(format!(
        "{{\"external_id\":\"{}\"}}",
        crate::writes::json_quote(external_id)
    ))
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("denetim satırı yazılamadı: {e}"))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    crate::log::audit(UNLINKED_EVENT, None, Some(identity_id), Some(target), None);
    Ok(())
}

/// ADR-122: hesap dizinde duruyormus — istek dusurulur, baglanti korunur.
pub async fn clear_unlink_request(
    pool: &sqlx::PgPool,
    identity_id: i64,
    target: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE account_links SET unlink_requested_at = NULL \
         WHERE identity_id = $1 AND target_system_id = $2",
    )
    .bind(identity_id)
    .bind(target)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| format!("kaldırma isteği düşürülemedi: {e}"))
}

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
    /// Kimlige yazilacak sicil: `employeeID`, bossa `employeeNumber` — ikisi AD'de
    /// ayri ozniteliktir ve kurumlar birini ya da otekini doldurur (ADR-106 madde 2).
    pub employee_number: Option<String>,
    /// Kimlige yazilacak telefon adayi: cep (`mobile`), cep bossa sabit hat
    /// (`telephoneNumber`) — mutabakat taramasiyla ayni sira (ADR-106 madde 3).
    pub phone: Option<String>,
    pub admin_count: bool,
    pub member_of: Vec<String>,
}

// E.164: '+' ve 8-15 rakam, ilk rakam 0 degil (docs/03 cep telefonu). Kural
// backend'deki `identity::valid_e164` ile ayni; tek fonksiyon icin ikiz dosya
// acilmadi (ADR-070 ikizleri modul boyu paylasim icin), kopya bu uc satirdir.
const E164_MIN_DIGITS: usize = 8;
const E164_MAX_DIGITS: usize = 15;

fn valid_e164(value: &str) -> bool {
    let digits = value.strip_prefix('+').unwrap_or("");
    (E164_MIN_DIGITS..=E164_MAX_DIGITS).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.starts_with('0')
}

impl Candidate {
    /// Kimligin cep alanina yazilabilecek numara. Kimlikteki alan E.164 ceptir
    /// (docs/03); sabit hat biciminde bir deger ("01632 960001") oraya yazilmaz
    /// ve uydurulmaz — bos kalir, is sonucuna uyari girer (ADR-106 madde 3).
    pub fn writable_phone(&self) -> Option<&str> {
        writable_phone(self.phone.as_deref())
    }

    pub fn phone_rejected(&self) -> bool {
        self.phone.is_some() && self.writable_phone().is_none()
    }
}

/// Ayni kural mutabakat dolumunda da gecerli (ADR-112 madde 1): aday numara
/// (cep, bossa sabit hat) E.164 degilse kimligin cep alanina yazilmaz.
pub fn writable_phone(phone: Option<&str>) -> Option<&str> {
    phone.filter(|p| valid_e164(p))
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
        "employeeID",
        "employeeNumber",
        "mobile",
        "telephoneNumber",
    ];
    attrs.extend(employee_attr.filter(|a| !attrs.contains(a)));
    let filter = format!("(&(objectClass=user)(sAMAccountName={}))", ldap_escape(sam));
    let found = ad::search(ldap, base, Scope::Subtree, &filter, &attrs).await?;
    let Some(entry) = found.into_iter().next() else {
        return Ok(None);
    };
    let text = |name: &str| {
        entry
            .attrs
            .get(name)
            .and_then(|v| v.first())
            .filter(|v| !v.trim().is_empty())
            .cloned()
    };
    let guid = ad_account::guid_by_dn(ldap, &entry.dn).await?;
    Ok(Some(Candidate {
        guid,
        sam: text("sAMAccountName").unwrap_or_else(|| sam.to_string()),
        upn: text("userPrincipalName"),
        mail: text("mail"),
        given_name: text("givenName"),
        surname: text("sn"),
        employee_value: employee_attr.and_then(text),
        employee_number: text("employeeID").or_else(|| text("employeeNumber")),
        phone: text("mobile").or_else(|| text("telephoneNumber")),
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
    fill_person_fields(&mut tx, identity_id, &PersonValues::of(cand)).await?;
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
    tx.commit().await.map_err(|e| e.to_string())?;
    crate::log::audit(ADOPTED_EVENT, None, Some(identity_id), Some(target), None);
    Ok(())
}

/// Kimlige yazilabilecek kisi alanlari. `None` = bu alan icin degerimiz yok.
/// `phone` E.164 kontrolunden gecmis olmali (`writable_phone`).
#[derive(Debug, Default)]
pub struct PersonValues<'a> {
    pub username: Option<&'a str>,
    pub email: Option<&'a str>,
    pub upn: Option<&'a str>,
    pub phone: Option<&'a str>,
    pub employee_number: Option<&'a str>,
    /// Yoneticinin kimlik id'si (ADR-129). Sahiplenme aninda bos: tek hesap
    /// okunurken yoneticinin DN'i kimlige cevrilemez, gece taramasi doldurur.
    pub manager_id: Option<i64>,
}

impl<'a> PersonValues<'a> {
    fn of(cand: &'a Candidate) -> PersonValues<'a> {
        PersonValues {
            username: Some(&cand.sam),
            email: cand.mail.as_deref(),
            upn: cand.upn.as_deref(),
            phone: cand.writable_phone(),
            employee_number: cand.employee_number.as_deref(),
            manager_id: None,
        }
    }
}

// Ad, e-posta, UPN, sicil, cep ve yonetici: hepsi yalnizca kimlikte bosken
// yazilir (ADR-129 yoneticiyi listeye ekledi), dolu
// alana dokunulmaz (ADR-034/086/106). Sicil tekil kolondur: ayni deger baska bir
// kimlikte duruyorsa yazilmaz — yoksa tek mukerrer numara butun sahiplenmeyi
// (baglanti + denetim satiri) geri alirdi. Sahiplenme ani ve gece mutabakati
// (ADR-112 madde 1) ayni fonksiyondan geçer.
pub async fn fill_person_fields(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    identity_id: i64,
    v: &PersonValues<'_>,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE identities SET username = COALESCE(username, $2), email = COALESCE(email, $3), \
         upn = COALESCE(upn, $4), mobile_phone = COALESCE(mobile_phone, $5), \
         employee_number = COALESCE(employee_number, \
           (SELECT $6::text WHERE NOT EXISTS \
              (SELECT 1 FROM identities o WHERE o.employee_number = $6))), \
         manager_id = COALESCE(manager_id, $7) \
         WHERE id = $1",
    )
    .bind(identity_id)
    .bind(v.username)
    .bind(v.email)
    .bind(v.upn)
    .bind(v.phone)
    .bind(v.employee_number)
    .bind(v.manager_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| format!("kimlik alanları yazılamadı: {e}"))
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
    crate::log::audit(MANAGED_EVENT, None, Some(identity_id), Some(target), None);
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

    fn candidate(employee: Option<&str>, phone: Option<&str>) -> Candidate {
        Candidate {
            dn: "CN=Ali Kaya,OU=Personel,DC=opensicil,DC=lab".to_string(),
            guid: "guid-1".to_string(),
            sam: "ali.kaya".to_string(),
            upn: Some("ali.kaya@opensicil.lab".to_string()),
            mail: Some("ali.kaya@opensicil.lab".to_string()),
            given_name: Some("Ali".to_string()),
            surname: Some("Kaya".to_string()),
            employee_value: None,
            employee_number: employee.map(str::to_string),
            phone: phone.map(str::to_string),
            admin_count: false,
            member_of: Vec::new(),
        }
    }

    // ADR-106 madde 3: sabit hat bicimi kimligin cep alanina yazilmaz, uydurulmaz.
    #[test]
    fn only_an_e164_number_reaches_the_identity() {
        let ok = candidate(None, Some("+905321234567"));
        assert_eq!(ok.writable_phone(), Some("+905321234567"));
        assert!(!ok.phone_rejected());
        for bad in ["01632 960001", "+0532123456", "+90532", "905321234567"] {
            let c = candidate(None, Some(bad));
            assert_eq!(c.writable_phone(), None, "{bad}");
            assert!(c.phone_rejected(), "{bad}");
        }
        let none = candidate(None, None);
        assert_eq!(none.writable_phone(), None);
        assert!(!none.phone_rejected(), "AD'de telefon yoksa uyarı da yok");
    }

    // ADR-086/106: bos alan dolar, dolu alan ezilmez, baskasinin sicili yazilmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn adoption_fills_only_the_empty_person_fields() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        let fields = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (Option<String>, Option<String>)>(
                    "SELECT employee_number, mobile_phone FROM identities WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };

        let cand = candidate(Some("7788"), Some("+905321234567"));
        link_observed(&pool, seed.identity, seed.ad, &cand, false)
            .await
            .unwrap();
        assert_eq!(
            fields(seed.identity).await,
            (Some("7788".into()), Some("+905321234567".into()))
        );

        // ikinci kimligin cebi dolu, sicili bos ama ayni sicil birincide duruyor
        sqlx::query("UPDATE identities SET mobile_phone = '+905000000000' WHERE id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        // ayri hesap: sAMAccountName AD'de tekildir, iki aday ayni adi tasiyamaz
        let mut other = candidate(Some("7788"), Some("+905321234567"));
        other.guid = "guid-2".to_string();
        other.sam = "veli.demir".to_string();
        other.upn = Some("veli.demir@opensicil.lab".to_string());
        other.mail = Some("veli.demir@opensicil.lab".to_string());
        link_observed(&pool, seed.other_identity, seed.ad, &other, false)
            .await
            .unwrap();
        assert_eq!(
            fields(seed.other_identity).await,
            (None, Some("+905000000000".into())),
            "mükerrer sicil yazılmaz, dolu cep ezilmez"
        );

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
