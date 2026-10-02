// --- START FEATURE: reconcile ---
// Mutabakat taramasi (ADR-099): yonetilen kullanici OU'larindaki hesaplari okur,
// `account_links` ile karsilastirir ve bulgulari anlik goruntu olarak yazar.
//
// Okuma seridinde calisir (ADR-051): hedefe hicbir sey yazmaz, denetim kaydina
// dokunmaz, fren sayaclarini (ADR-050) harcamaz. Tek yazdigi tablo
// `reconcile_findings` ve oraya yalnizca worker yazabilir (ADR-015).
use std::collections::HashSet;

use crate::ad::DirectoryAccount;
use crate::adoption::{self, PersonValues};
use sqlx::{PgPool, Postgres, Row, Transaction};

pub const MANAGED: &str = "managed";
pub const OBSERVED: &str = "observed";
pub const UNMANAGED: &str = "unmanaged";
pub const MISSING: &str = "missing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub external_id: String,
    pub mode: String,
    pub identity_id: i64,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: &'static str,
    pub external_id: String,
    pub account_name: String,
    pub display_name: Option<String>,
    pub container: Option<String>,
    pub enabled: Option<bool>,
    pub identity_id: Option<i64>,
    /// Toplu sahiplenmenin kimlik satirini kurdugu kisi oznitelikleri (ADR-102).
    /// `missing` bulgusunda hesap dizinde yok, dordu de bos.
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub employee_number: Option<String>,
    pub department_name: Option<String>,
    /// AD'deki iletisim alanlari (ADR-106): ekran "hangi hesapta hangi alan
    /// geldi" sorusunu cevaplar, toplu sahiplenme telefonu buradan alir.
    pub mail: Option<String>,
    pub mobile: Option<String>,
    pub telephone: Option<String>,
    /// Hesabin acilis gunu `YYYY-MM-DD` (ADR-103 madde 6); toplu sahiplenmenin
    /// baslangic tarihi, bossa formdaki tarih
    pub when_created: Option<String>,
    /// AD'den okunan TC kimlik no (ayar doluysa, ADR-106 madde 5). Yalnizca
    /// bellekte duz; `store` AEAD ile sifreleyip yazar (ADR-010)
    pub national_id: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub managed: usize,
    pub observed: usize,
    pub unmanaged: usize,
    pub missing: usize,
}

impl Counts {
    fn of(findings: &[Finding]) -> Counts {
        let count = |kind| findings.iter().filter(|f| f.kind == kind).count();
        Counts {
            managed: count(MANAGED),
            observed: count(OBSERVED),
            unmanaged: count(UNMANAGED),
            missing: count(MISSING),
        }
    }
}

/// Saf karsilastirma: AD'de goruleni bagli olanla esler (ADR-099 madde 2).
/// `silindi` isaretli baglanti (worker'in kendi sildigi hesap) kayip sayilmaz —
/// onu zaten biz sildik, dizinde olmamasi beklenen durumdur.
pub fn compare(accounts: &[DirectoryAccount], links: &[Link]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = accounts
        .iter()
        .map(|account| {
            let link = links.iter().find(|l| l.external_id == account.guid);
            Finding {
                kind: match link.map(|l| l.mode.as_str()) {
                    Some(OBSERVED) => OBSERVED,
                    Some(_) => MANAGED,
                    None => UNMANAGED,
                },
                external_id: account.guid.clone(),
                account_name: account.sam.clone(),
                display_name: account.display_name.clone(),
                container: Some(account.container.clone()),
                enabled: Some(account.enabled),
                identity_id: link.map(|l| l.identity_id),
                given_name: account.given_name.clone(),
                surname: account.surname.clone(),
                employee_number: account.employee_number.clone(),
                department_name: account.department.clone(),
                mail: account.mail.clone(),
                mobile: account.mobile.clone(),
                telephone: account.telephone.clone(),
                when_created: account.when_created.clone(),
                national_id: account.national_id.clone(),
            }
        })
        .collect();

    // ADR-040 "kayip hesap": OpenSicil bagli sayiyor, dizinde yok.
    let seen = |link: &&Link| accounts.iter().any(|a| a.guid == link.external_id);
    findings.extend(links.iter().filter(|l| !seen(l)).map(missing_finding));
    findings.sort_by(|a, b| a.account_name.cmp(&b.account_name));
    findings
}

fn missing_finding(link: &Link) -> Finding {
    Finding {
        kind: MISSING,
        external_id: link.external_id.clone(),
        account_name: link.username.clone().unwrap_or_else(|| "?".to_string()),
        display_name: None,
        container: None,
        enabled: None,
        identity_id: Some(link.identity_id),
        given_name: None,
        surname: None,
        employee_number: None,
        department_name: None,
        mail: None,
        mobile: None,
        telephone: None,
        when_created: None,
        national_id: None,
    }
}

const LINKS_SQL: &str = "SELECT l.external_id, l.mode, l.identity_id, i.username \
    FROM account_links l JOIN identities i ON i.id = l.identity_id \
    WHERE l.target_system_id = $1 AND l.deleted_by_us_at IS NULL";

pub async fn load_links(pool: &PgPool, target: i64) -> Result<Vec<Link>, sqlx::Error> {
    let rows: Vec<(String, String, i64, Option<String>)> = sqlx::query_as(LINKS_SQL)
        .bind(target)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(external_id, mode, identity_id, username)| Link {
            external_id,
            mode,
            identity_id,
            username,
        })
        .collect())
}

/// Anlik goruntu: hedefin onceki bulgulari silinir, yenileri tek transaction'da
/// yazilir (ADR-099 madde 3). Yarim kalmis bir tarama ekrani bosaltmaz.
/// Anlik goruntuyu tek transaction'da degistirir. TC kimlik no sifreli yazilir
/// (`crypto::encrypt_versioned`, backend `national_id::decrypt` ile ayni bicim).
pub async fn store(
    pool: &PgPool,
    target: i64,
    read_job_id: i64,
    findings: &[Finding],
    aead_key: &[u8; crate::crypto::KEY_LEN],
) -> Result<Counts, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;
    sqlx::query("DELETE FROM reconcile_findings WHERE target_system_id = $1")
        .bind(target)
        .execute(&mut *tx)
        .await?;
    for finding in findings {
        let national_id_enc = finding
            .national_id
            .as_deref()
            .map(|v| crate::crypto::encrypt_versioned(aead_key, v.as_bytes()));
        sqlx::query(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
             external_id, account_name, display_name, container, enabled, identity_id, \
             given_name, surname, employee_number, department_name, mail, mobile, \
             telephone_number, when_created, national_id_enc) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
             $17::date, $18)",
        )
        .bind(target)
        .bind(read_job_id)
        .bind(finding.kind)
        .bind(&finding.external_id)
        .bind(&finding.account_name)
        .bind(&finding.display_name)
        .bind(&finding.container)
        .bind(finding.enabled)
        .bind(finding.identity_id)
        .bind(&finding.given_name)
        .bind(&finding.surname)
        .bind(&finding.employee_number)
        .bind(&finding.department_name)
        .bind(&finding.mail)
        .bind(&finding.mobile)
        .bind(&finding.telephone)
        .bind(&finding.when_created)
        .bind(national_id_enc)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Counts::of(findings))
}
// Denetim: dolum bir kimlik olayidir (hangi alan doldu), hedefe yazma niyeti
// degil — okuma seridinin "denetime dokunma" kurali (ADR-099) worker'in hedefe
// yazma niyet satirlarini kapsar, model degisikligi kayitsiz kalmaz.
pub const FILLED_EVENT: &str = "identity.fields_filled";

// Dolum adaylari: bagli bulgu + alani eksik, silinmemis kimlik. Sicilin baska
// kimlikte durup durmadigi ayni sorguda sorulur.
const FILL_SQL: &str = "SELECT f.identity_id, f.account_name, f.mail AS ad_mail, \
    f.mobile AS ad_mobile, f.telephone_number AS ad_telephone, \
    f.employee_number AS ad_employee_number, \
    NOT EXISTS (SELECT 1 FROM identities o WHERE o.employee_number = f.employee_number) \
      AS employee_number_free, \
    i.username, i.email, i.mobile_phone, i.employee_number \
    FROM reconcile_findings f JOIN identities i ON i.id = f.identity_id \
    WHERE f.target_system_id = $1 AND i.deleted_at IS NULL \
      AND (i.username IS NULL OR i.email IS NULL OR i.mobile_phone IS NULL \
           OR i.employee_number IS NULL) \
    ORDER BY f.identity_id";

/// ADR-112 madde 1: alan kimlikte doluysa dokunulmaz, bossa AD'deki deger yazilir.
fn fillable<'a>(have: Option<&str>, found: Option<&'a str>) -> Option<&'a str> {
    if have.is_some() {
        None
    } else {
        found
    }
}

/// Denetim satirina girecek alan adlari; bos liste = yazilacak sey yok.
fn filled_field_names(v: &PersonValues<'_>) -> Vec<&'static str> {
    [
        ("username", v.username.is_some()),
        ("email", v.email.is_some()),
        ("mobile_phone", v.phone.is_some()),
        ("employee_number", v.employee_number.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, set)| set.then_some(name))
    .collect()
}

/// Taramadan sonra bagli (yonetilen + gozlemdeki) kimliklerin **bos** alanlari
/// AD'de okunan degerden dolar (ADR-112 madde 1). Dolu alan degismez, hedefe
/// yazilmaz, fren sayaci harcanmaz (ADR-051); degisen tek tablo `identities`,
/// yanina kimlik olayi dusulur. Silinmis kimlik atlanir — kisisel veri
/// temizligi (ADR-024) geri doldurulmaz. UPN bulguda tasinmiyor, dolmuyor.
/// Doner: alani dolan kimlik sayisi.
pub async fn fill_linked_identities(pool: &PgPool, target: i64) -> Result<usize, String> {
    let rows = sqlx::query(FILL_SQL)
        .bind(target)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("dolum adayları okunamadı: {e}"))?;
    let mut tx: Transaction<'_, Postgres> = pool.begin().await.map_err(|e| e.to_string())?;
    // Ayni sicil iki bulguda duruyorsa ikincisini SQL'in tekillik kontrolu
    // zaten atlar; denetim satiri "doldu" demesin diye burada da atlanir.
    let mut taken: HashSet<String> = HashSet::new();
    let mut filled = 0;
    for row in &rows {
        let identity: i64 = row.get("identity_id");
        let account_name: String = row.get("account_name");
        let ad_mail: Option<String> = row.get("ad_mail");
        let ad_mobile: Option<String> = row.get("ad_mobile");
        let ad_telephone: Option<String> = row.get("ad_telephone");
        let ad_employee: Option<String> = row.get("ad_employee_number");
        let employee_free: bool = row.get("employee_number_free");
        let have_username: Option<String> = row.get("username");
        let have_email: Option<String> = row.get("email");
        let have_phone: Option<String> = row.get("mobile_phone");
        let have_employee: Option<String> = row.get("employee_number");

        let phone = adoption::writable_phone(ad_mobile.as_deref().or(ad_telephone.as_deref()));
        let free_employee = ad_employee
            .as_deref()
            .filter(|v| employee_free && !taken.contains(*v));
        let values = PersonValues {
            username: fillable(have_username.as_deref(), Some(account_name.as_str())),
            email: fillable(have_email.as_deref(), ad_mail.as_deref()),
            upn: None,
            phone: fillable(have_phone.as_deref(), phone),
            employee_number: fillable(have_employee.as_deref(), free_employee),
        };
        let names = filled_field_names(&values);
        if names.is_empty() {
            continue;
        }
        if let Some(value) = values.employee_number {
            taken.insert(value.to_string());
        }
        adoption::fill_person_fields(&mut tx, identity, &values).await?;
        sqlx::query(
            "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
             VALUES ($1, $2, $3, $4::jsonb)",
        )
        .bind(FILLED_EVENT)
        .bind(identity)
        .bind(target)
        .bind(format!("{{\"fields\":[\"{}\"]}}", names.join("\",\"")))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("dolum denetim satırı yazılamadı: {e}"))?;
        filled += 1;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(filled)
}
// --- END FEATURE: reconcile ---

#[cfg(test)]
mod tests {
    use super::*;

    fn account(guid: &str, sam: &str, enabled: bool) -> DirectoryAccount {
        DirectoryAccount {
            guid: guid.to_string(),
            sam: sam.to_string(),
            display_name: Some(sam.to_uppercase()),
            dn: format!("CN={sam},OU=Users,OU=Hogwarts,DC=hogwarts,DC=local"),
            container: "OU=Users,OU=Hogwarts,DC=hogwarts,DC=local".to_string(),
            enabled,
            given_name: Some(sam.to_string()),
            surname: Some("Hogwarts".to_string()),
            employee_number: None,
            department: Some("Teachers".to_string()),
            mail: Some(format!("{sam}@hogwarts.local")),
            mobile: None,
            telephone: Some("01632 960001".to_string()),
            when_created: Some("2024-09-01".to_string()),
            national_id: None,
        }
    }

    /// ADR-010/106: TC kimlik no bulguya yalnizca sifreli girer; ayni anahtar
    /// surum bayti biciminde cozulur, bos olan satirda kolon NULL kalir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_national_id_is_stored_encrypted_or_not_at_all() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        let key = [5u8; crate::crypto::KEY_LEN];
        let mut with_id = account("g-1", "hpotter", true);
        with_id.national_id = Some("10000000146".to_string());
        let findings = compare(&[with_id, account("g-2", "hgranger", true)], &[]);
        store(&pool, target, read_job, &findings, &key)
            .await
            .unwrap();

        let rows: Vec<(String, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT account_name, national_id_enc FROM reconcile_findings ORDER BY account_name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows[0].0, "hgranger");
        assert_eq!(rows[0].1, None, "değer yoksa kolon boş");
        let enc = rows[1]
            .1
            .as_deref()
            .expect("hpotter şifreli değer taşımalı");
        assert_ne!(enc, b"10000000146", "düz metin yazılmaz");
        let plain = crate::crypto::decrypt_versioned(&key, enc).unwrap();
        assert_eq!(plain, b"10000000146");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    fn link(guid: &str, mode: &str, identity_id: i64, username: &str) -> Link {
        Link {
            external_id: guid.to_string(),
            mode: mode.to_string(),
            identity_id,
            username: Some(username.to_string()),
        }
    }

    #[test]
    fn classifies_every_account_against_the_links() {
        let accounts = vec![
            account("g-1", "hpotter", true),
            account("g-2", "hgranger", true),
            account("g-3", "dmalfoy", false),
        ];
        let links = vec![
            link("g-1", MANAGED, 11, "hpotter"),
            link("g-2", OBSERVED, 12, "hgranger"),
            // AD'de karsiligi yok: kayip hesap (ADR-040)
            link("g-9", MANAGED, 19, "triddle"),
        ];

        let findings = compare(&accounts, &links);
        let of = |sam: &str| {
            findings
                .iter()
                .find(|f| f.account_name == sam)
                .unwrap_or_else(|| panic!("{sam} bulunamadı"))
        };

        assert_eq!(of("hpotter").kind, MANAGED);
        assert_eq!(of("hpotter").identity_id, Some(11));
        assert_eq!(of("hgranger").kind, OBSERVED);
        // Hicbir kimlige bagli olmayan hesap: toplu sahiplenmenin girdisi
        assert_eq!(of("dmalfoy").kind, UNMANAGED);
        assert_eq!(of("dmalfoy").identity_id, None);
        assert_eq!(of("dmalfoy").enabled, Some(false));
        assert_eq!(of("triddle").kind, MISSING);
        assert_eq!(of("triddle").enabled, None);

        assert_eq!(
            Counts::of(&findings),
            Counts {
                managed: 1,
                observed: 1,
                unmanaged: 1,
                missing: 1
            }
        );
        // Ad sirasi: ekranda liste sabit kalsin
        let names: Vec<&str> = findings.iter().map(|f| f.account_name.as_str()).collect();
        assert_eq!(names, ["dmalfoy", "hgranger", "hpotter", "triddle"]);
    }

    // Hic kimlik kaydedilmemis kurumun ilk taramasi: hepsi yonetilmeyen.
    #[test]
    fn first_scan_of_an_empty_install_reports_every_account_as_unmanaged() {
        let accounts: Vec<DirectoryAccount> = (1..=29)
            .map(|n| account(&format!("g-{n}"), &format!("user{n:02}"), true))
            .collect();
        let findings = compare(&accounts, &[]);
        assert_eq!(findings.len(), 29);
        assert!(findings.iter().all(|f| f.kind == UNMANAGED));
        assert_eq!(Counts::of(&findings).unmanaged, 29);
    }

    // ADR-112 madde 1 saf kural: dolu alana dokunulmaz, degeri olmayan alan dolmaz.
    #[test]
    fn only_an_empty_field_with_a_value_is_filled() {
        assert_eq!(fillable(None, Some("deger")), Some("deger"));
        assert_eq!(fillable(Some("elde var"), Some("deger")), None);
        assert_eq!(fillable(None, None), None);
        let values = PersonValues {
            username: Some("ali.kaya"),
            employee_number: Some("7788"),
            ..PersonValues::default()
        };
        assert_eq!(
            filled_field_names(&values),
            vec!["username", "employee_number"]
        );
        assert!(filled_field_names(&PersonValues::default()).is_empty());
    }

    // ADR-112 madde 1 uctan uca: tarama bulgusundan bos alanlar dolar; dolu alan,
    // E.164 olmayan numara, mukerrer sicil ve silinmis kimlik atlanir, denetim yazilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_scan_fills_empty_identity_fields_from_the_directory() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        let read_job: i64 = sqlx::query_scalar(
            "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
             VALUES ('reconcile', $1, 'test') RETURNING id",
        )
        .bind(seed.ad)
        .fetch_one(&pool)
        .await
        .unwrap();
        // ikinci kimligin e-postasi elle girilmis: dolum onu ezmemeli
        sqlx::query("UPDATE identities SET email = 'elle@girildi' WHERE id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();

        let mut ayse = account("g-1", "ayse.yilmaz", true);
        ayse.mobile = Some("+905321234567".to_string());
        ayse.employee_number = Some("7788".to_string());
        // Ali'nin cebi yok (sabit hat E.164 degil) ve sicili Ayse'nin sicili
        let mut ali = account("g-2", "ali.kaya", true);
        ali.employee_number = Some("7788".to_string());
        let links = [
            link("g-1", MANAGED, seed.identity, "?"),
            link("g-2", OBSERVED, seed.other_identity, "?"),
        ];
        let findings = compare(&[ayse, ali], &links);
        store(
            &pool,
            seed.ad,
            read_job,
            &findings,
            &[7u8; crate::crypto::KEY_LEN],
        )
        .await
        .unwrap();

        assert_eq!(fill_linked_identities(&pool, seed.ad).await.unwrap(), 2);
        let fields = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<
                    _,
                    (
                        Option<String>,
                        Option<String>,
                        Option<String>,
                        Option<String>,
                    ),
                >(
                    "SELECT username, email, mobile_phone, employee_number \
                     FROM identities WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(
            fields(seed.identity).await,
            (
                Some("ayse.yilmaz".into()),
                Some("ayse.yilmaz@hogwarts.local".into()),
                Some("+905321234567".into()),
                Some("7788".into())
            )
        );
        assert_eq!(
            fields(seed.other_identity).await,
            (
                Some("ali.kaya".into()),
                Some("elle@girildi".into()),
                None,
                None
            ),
            "dolu e-posta ezilmez, sabit hat ceple yazılmaz, mükerrer sicil atlanır"
        );
        let detail: String = sqlx::query_scalar(
            "SELECT detail::text FROM audit_log WHERE event_type = $1 AND identity_id = $2",
        )
        .bind(FILLED_EVENT)
        .bind(seed.other_identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(detail.contains("username"), "{detail}");
        assert!(!detail.contains("email"), "{detail}");

        assert_eq!(
            fill_linked_identities(&pool, seed.ad).await.unwrap(),
            0,
            "ikinci tarama dolacak alan bulmaz"
        );

        // ADR-024 kisisel veri temizligi geri doldurulmaz. (Sicil serbest kaldigi
        // icin ayni taramada Ali'nin sicili dolar; bakilan sey silinmis kimlik.)
        sqlx::query(
            "UPDATE identities SET deleted_at = now(), username = NULL, email = NULL, \
             mobile_phone = NULL, employee_number = NULL WHERE id = $1",
        )
        .bind(seed.identity)
        .execute(&pool)
        .await
        .unwrap();
        fill_linked_identities(&pool, seed.ad).await.unwrap();
        assert_eq!(
            fields(seed.identity).await,
            (None, None, None, None),
            "silinmiş kimliğin alanları yeniden dolmaz"
        );
        let again: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE event_type = $1 AND identity_id = $2",
        )
        .bind(FILLED_EVENT)
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(again, 1, "silinmiş kimliğe ikinci dolum satırı yazılmaz");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
