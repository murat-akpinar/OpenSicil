// --- START FEATURE: reconcile ---
// Mutabakat taramasi (ADR-099): yonetilen kullanici OU'larindaki hesaplari okur,
// `account_links` ile karsilastirir ve bulgulari anlik goruntu olarak yazar.
//
// Okuma seridinde calisir (ADR-051): hedefe hicbir sey yazmaz, denetim kaydina
// dokunmaz, fren sayaclarini (ADR-050) harcamaz. Tek yazdigi tablo
// `reconcile_findings` ve oraya yalnizca worker yazabilir (ADR-015).
use crate::ad::DirectoryAccount;
use sqlx::{PgPool, Postgres, Transaction};

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
pub async fn store(
    pool: &PgPool,
    target: i64,
    read_job_id: i64,
    findings: &[Finding],
) -> Result<Counts, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;
    sqlx::query("DELETE FROM reconcile_findings WHERE target_system_id = $1")
        .bind(target)
        .execute(&mut *tx)
        .await?;
    for finding in findings {
        sqlx::query(
            "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
             external_id, account_name, display_name, container, enabled, identity_id, \
             given_name, surname, employee_number, department_name, mail, mobile, \
             telephone_number, when_created) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
             $17::date)",
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
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Counts::of(findings))
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
        }
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
}
