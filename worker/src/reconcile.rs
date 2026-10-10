// --- START FEATURE: reconcile ---
// Mutabakat taramasi (ADR-099): yonetilen kullanici OU'larindaki hesaplari okur,
// `account_links` ile karsilastirir ve bulgulari anlik goruntu olarak yazar.
//
// Okuma seridinde calisir (ADR-051): hedefe hicbir sey yazmaz, denetim kaydina
// dokunmaz, fren sayaclarini (ADR-050) harcamaz. Tek yazdigi tablo
// `reconcile_findings` ve oraya yalnizca worker yazabilir (ADR-015).
use std::collections::{HashMap, HashSet};

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
    /// AD'deki unvan (`title`); rol dolumu ve fark listesi bunu `roles.title`
    /// ile eslestirir (ADR-120 madde 5)
    pub title: Option<String>,
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
    /// Yoneticinin **hesabinin** objectGUID'i (ADR-129): `manager` DN'i ayni
    /// taramanin hesap listesinden cozulur. Yonetici kapsam disindaysa bos.
    pub manager_external_id: Option<String>,
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
    // ADR-129: `manager` DN'i kimlige ancak hesabin GUID'i uzerinden baglanir.
    // Harita ayni taramanin hesap listesinden kurulur — ikinci bir LDAP sorgusu
    // yok. DN karsilastirmasi harf buyuklugune duyarsiz (AD de boyle davranir).
    let by_dn: HashMap<String, &str> = accounts
        .iter()
        .map(|a| (a.dn.to_lowercase(), a.guid.as_str()))
        .collect();
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
                title: account.title.clone(),
                mail: account.mail.clone(),
                mobile: account.mobile.clone(),
                telephone: account.telephone.clone(),
                when_created: account.when_created.clone(),
                national_id: account.national_id.clone(),
                manager_external_id: account
                    .manager_dn
                    .as_deref()
                    .and_then(|dn| by_dn.get(&dn.to_lowercase()))
                    .map(|guid| (*guid).to_string()),
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
        title: None,
        mail: None,
        mobile: None,
        telephone: None,
        when_created: None,
        national_id: None,
        manager_external_id: None,
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
             given_name, surname, employee_number, department_name, title, mail, mobile, \
             telephone_number, when_created, national_id_enc, manager_external_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
             $17, $18::date, $19, $20)",
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
        .bind(&finding.title)
        .bind(&finding.mail)
        .bind(&finding.mobile)
        .bind(&finding.telephone)
        .bind(&finding.when_created)
        .bind(national_id_enc)
        .bind(&finding.manager_external_id)
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
    i.username, i.email, i.mobile_phone, i.employee_number, i.manager_id, \
    mi.id AS ad_manager_id \
    FROM reconcile_findings f JOIN identities i ON i.id = f.identity_id \
    LEFT JOIN reconcile_findings mf ON mf.target_system_id = f.target_system_id \
      AND mf.external_id = f.manager_external_id \
    LEFT JOIN identities mi ON mi.id = mf.identity_id AND mi.deleted_at IS NULL \
      AND mi.id <> f.identity_id \
    WHERE f.target_system_id = $1 AND i.deleted_at IS NULL \
      AND (i.username IS NULL OR i.email IS NULL OR i.mobile_phone IS NULL \
           OR i.employee_number IS NULL OR i.manager_id IS NULL) \
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
        ("manager", v.manager_id.is_some()),
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
/// Yonetici (ADR-129): bulgudaki `manager_external_id` ayni taramadaki yonetici
/// bulgusuna, oradan onun kimligine cevrilir; yoneticinin hesabi bir kimlige
/// bagli degilse (henuz sahiplenilmemis) alan bos kalir ve sonraki tarama dener.
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
    let mut filled_ids: Vec<i64> = Vec::new();
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
        let have_manager: Option<i64> = row.get("manager_id");
        let ad_manager: Option<i64> = row.get("ad_manager_id");

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
            // ADR-129: yonetici de bos alan; dolu olan degismez. Yoneticinin
            // hesabi henuz sahiplenilmemisse `ad_manager` bostur, sonraki gece
            // taramasi yeniden dener.
            manager_id: have_manager.is_none().then_some(ad_manager).flatten(),
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
        filled_ids.push(identity);
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    for identity in &filled_ids {
        crate::log::audit(FILLED_EVENT, None, Some(*identity), Some(target), None);
    }
    Ok(filled_ids.len())
}

// Yer tutucu rolun dolumu (ADR-120 madde 5): AD'nin `title` degeri agacta bir
// birincil rolun unvaniysa yazilir. Eslestirme rol **adi** uzerinden degil
// `roles.title` kolonu uzerinden yapilir — o kolon rolun AD karsiligidir.
// Unvan iki rolde duruyorsa (`title` tekil degil) `HAVING count(*) = 1`
// eslesmeyi yok sayar, hangisi oldugu tahmin edilmez.
//
// `fill_linked_identities` ile ayni seritte (ADR-051): hedefe yazilmaz, is
// acilmaz. Dolan rol yer tutucunun yerine gectigi icin AD'deki `title` ile
// zaten uyumludur; hak seti farki ilk isin hesabina girer.
const FILL_ROLE_SQL: &str = "WITH filled AS (     UPDATE identities i SET primary_role_id = m.role_id FROM (         SELECT f.identity_id,                (SELECT max(r.id) FROM roles r                   WHERE r.kind = 'primary' AND NOT r.placeholder                     AND lower(btrim(r.title)) = lower(btrim(f.title))                   HAVING count(*) = 1) AS role_id           FROM reconcile_findings f          WHERE f.target_system_id = $1 AND btrim(coalesce(f.title, '')) <> ''     ) m     WHERE i.id = m.identity_id AND m.role_id IS NOT NULL AND i.deleted_at IS NULL       AND EXISTS (SELECT 1 FROM roles p WHERE p.id = i.primary_role_id AND p.placeholder)     RETURNING i.id)     INSERT INTO audit_log (event_type, identity_id, target_system_id, detail)     SELECT $2, id, $1, '{\"fields\":[\"primary_role\"]}'::jsonb FROM filled RETURNING identity_id";

/// Birincil rolu yer tutucu (`Tanimsiz`) olan bagli kimliklerin rolu, AD'deki
/// unvanin agactaki karsiligiyla dolar. Yer tutucu "bos" sayilir (ADR-112
/// madde 1); gercek bir rol duruyorsa dokunulmaz — ikisi de doluysa karar
/// operatorun, satir fark listesine girer. Doner: rolu dolan kimlik sayisi.
pub async fn fill_placeholder_roles(pool: &PgPool, target: i64) -> Result<u64, String> {
    let filled: Vec<i64> = sqlx::query_scalar(FILL_ROLE_SQL)
        .bind(target)
        .bind(FILLED_EVENT)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("rol dolumu yapılamadı: {e}"))?;
    for identity in &filled {
        crate::log::audit(FILLED_EVENT, None, Some(*identity), Some(target), None);
    }
    Ok(filled.len() as u64)
}
// --- END FEATURE: reconcile ---

// --- START FEATURE: ad-change-sync ---
// ADR-138: AD'de yapilan degisiklik kendiliginden gelir. Taban onceki taramanin
// anlik goruntusudur — ayri bir "son esitlenen deger" tablosu yok. Alan yalnizca
// AD'de degistiyse (bizdeki deger AD'nin eski haline esitse) kimlige yazilir;
// iki taraf da degistiyse satir "AD'de farkli" listesinde kalir (ADR-112 madde 2).

/// Bir hesabin kisi alanlari: AD tarafi (eski/yeni tarama) ya da kimlik tarafi.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersonFields {
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub employee_number: Option<String>,
    pub phone: Option<String>,
    pub department: Option<String>,
    pub title: Option<String>,
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim() == b.trim()
}

// Departman ve unvan agacta buyuk/kucuk harfe bakmadan cozulur (ad_diff ile ayni).
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn present(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// ADR-138 madde 1: yeni AD degeri yalnizca AD degistiyse ve biz eski AD
/// degerinde duruyorsak alinir. Taban yoksa, AD bosaldiysa ya da bizde bossa
/// (onu dolum yapar, ADR-112 madde 1) hicbir sey alinmaz.
fn ad_only_change<'a>(
    old: Option<&str>,
    new: Option<&'a str>,
    ours: Option<&str>,
    same: fn(&str, &str) -> bool,
) -> Option<&'a str> {
    let (old, new, ours) = (old?, new?, ours?);
    (!same(old, new) && same(ours, old)).then_some(new)
}

/// (alan, alanin okunusu, esitlik kurali)
type FieldRule = (
    &'static str,
    fn(&PersonFields) -> &Option<String>,
    fn(&str, &str) -> bool,
);

/// Alinacak alanlar: (alan, AD'deki yeni deger, bizdeki eski deger).
pub fn ad_changes<'a>(
    old: &PersonFields,
    new: &'a PersonFields,
    ours: &'a PersonFields,
) -> Vec<(&'static str, &'a str, &'a str)> {
    let fields: [FieldRule; 6] = [
        ("given_name", |p| &p.given_name, same_text),
        ("surname", |p| &p.surname, same_text),
        (
            // Yazim birebir: `00000000009` ile `9` esitlemede farktir (ADR-138 ek);
            // bastaki sifiri yok sayan kural yalnizca sahiplenme eslestirmesinde
            "employee_number",
            |p| &p.employee_number,
            same_text,
        ),
        ("mobile_phone", |p| &p.phone, same_text),
        ("department", |p| &p.department, same_name),
        ("role", |p| &p.title, same_name),
    ];
    fields
        .into_iter()
        .filter_map(|(field, get, same)| {
            let to = ad_only_change(
                present(get(old)),
                present(get(new)),
                present(get(ours)),
                same,
            )?;
            Some((field, to, present(get(ours))?))
        })
        .collect()
}

fn ad_side(row: &sqlx::postgres::PgRow, prefix: &str) -> PersonFields {
    let get = |c: &str| row.get::<Option<String>, _>(format!("{prefix}{c}").as_str());
    PersonFields {
        given_name: get("given_name"),
        surname: get("surname"),
        employee_number: get("employee_number"),
        phone: adoption::writable_phone(get("mobile").or(get("telephone_number")).as_deref())
            .map(str::to_string),
        department: get("department_name"),
        title: get("title"),
    }
}

/// Taramadan once cagrilir: bagli hesaplarin onceki AD hali (taban).
pub async fn load_previous(
    pool: &PgPool,
    target: i64,
) -> Result<HashMap<String, PersonFields>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT external_id, given_name, surname, employee_number, mobile, telephone_number, \
         department_name, title FROM reconcile_findings \
         WHERE target_system_id = $1 AND identity_id IS NOT NULL",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get("external_id"), ad_side(row, "")))
        .collect())
}

// Yeni bulgu + kimligin bugunku hali + AD metninin agactaki karsiligi. Yer tutucu
// rolun unvani "bos" sayilir: onu `fill_placeholder_roles` doldurur.
const CHANGES_SQL: &str = "SELECT f.identity_id, f.external_id, \
    f.given_name, f.surname, f.employee_number, f.mobile, f.telephone_number, \
    f.department_name, f.title, \
    (SELECT max(d.id) FROM departments d \
       WHERE lower(d.name) = lower(f.department_name) HAVING count(*) = 1) AS department_id, \
    (SELECT max(r.id) FROM roles r WHERE r.kind = 'primary' AND NOT r.placeholder \
       AND lower(btrim(r.title)) = lower(btrim(f.title)) HAVING count(*) = 1) AS role_id, \
    i.given_name AS i_given_name, i.surname AS i_surname, \
    i.employee_number AS i_employee_number, i.mobile_phone AS i_mobile, \
    NULL::text AS i_telephone_number, dep.name AS i_department_name, \
    CASE WHEN rol.placeholder THEN NULL ELSE rol.title END AS i_title \
    FROM reconcile_findings f JOIN identities i ON i.id = f.identity_id \
    JOIN departments dep ON dep.id = i.department_id \
    JOIN roles rol ON rol.id = i.primary_role_id \
    WHERE f.target_system_id = $1 AND i.deleted_at IS NULL ORDER BY f.identity_id";

// Sicil tekil: baska kimlikte duruyorsa yazilmaz (ADR-112 madde 1 ile ayni kural).
fn update_sql(field: &str) -> &'static str {
    match field {
        "given_name" => "UPDATE identities SET given_name = $1 WHERE id = $2",
        "surname" => "UPDATE identities SET surname = $1 WHERE id = $2",
        "employee_number" => {
            "UPDATE identities SET employee_number = $1 WHERE id = $2 AND NOT EXISTS \
             (SELECT 1 FROM identities o WHERE o.employee_number = $1 AND o.id <> $2)"
        }
        _ => "UPDATE identities SET mobile_phone = $1 WHERE id = $2",
    }
}

/// Okuma seridi alani, onu besleyen AD oznitelikleri ve alanin kendi esleme kaynagi.
const READ_LANE_SOURCES: [(&str, &[&str], &str); 6] = [
    ("given_name", &["givenName"], "given_name"),
    ("surname", &["sn"], "surname"),
    (
        "employee_number",
        &["employeeID", "employeeNumber"],
        "employee_number",
    ),
    (
        "mobile_phone",
        &["mobile", "telephoneNumber"],
        "mobile_phone",
    ),
    ("department", &["department"], "department_name"),
    ("role", &["title"], "title"),
];

/// ADR-138: AD'deki deger ancak eslememiz o ozniteligi kimligin ayni alanindan,
/// degistirmeden yaziyorsa "AD'de yapilmis degisiklik" olabilir. Sabit, sablon,
/// baska alan ya da harf/ascii donusumu bizim yankimizdir; alinirsa role_admin
/// eslemeyle IK alanlarini degistirir, `{employee_number}9` her taramada uzar
/// (guvenlik denetimi OS-05). Telefon bicim donusumu yazimdan sonra ayni numaraya
/// cozulur (`writable_phone`), yanki degildir.
/// Satir: (hedef oznitelik, kaynak, donusum).
pub fn echoed_fields(mappings: &[(String, String, String)]) -> Vec<&'static str> {
    READ_LANE_SOURCES
        .iter()
        .filter(|(field, attributes, own)| {
            mappings.iter().any(|(attribute, kind, transform)| {
                let faithful = kind == own
                    && (transform == "none"
                        || (*field == "mobile_phone" && transform.starts_with("phone_")));
                attributes.contains(&attribute.as_str()) && !faithful
            })
        })
        .map(|(field, _, _)| *field)
        .collect()
}

/// Bulgular yazildiktan sonra cagrilir. Doner: AD'den degisiklik alinan kimlik sayisi.
pub async fn take_ad_changes(
    pool: &PgPool,
    target: i64,
    previous: &HashMap<String, PersonFields>,
) -> Result<usize, String> {
    let rows = sqlx::query(CHANGES_SQL)
        .bind(target)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("AD değişiklik adayları okunamadı: {e}"))?;
    let mappings: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT target_attribute, source_kind, transform FROM attribute_mappings \
         WHERE target_system_id = $1",
    )
    .bind(target)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("eşlemeler okunamadı: {e}"))?;
    let echoed = echoed_fields(&mappings);
    let mut tx: Transaction<'_, Postgres> = pool.begin().await.map_err(|e| e.to_string())?;
    let mut changed_ids: Vec<i64> = Vec::new();
    for row in &rows {
        let external_id: String = row.get("external_id");
        let Some(old) = previous.get(&external_id) else {
            continue; // taban yok: ilk tarama ya da yeni baglanti
        };
        let identity: i64 = row.get("identity_id");
        let (new, ours) = (ad_side(row, ""), ad_side(row, "i_"));
        let mut taken = 0;
        for (field, to, from) in ad_changes(old, &new, &ours) {
            if echoed.contains(&field) {
                continue;
            }
            let node = match field {
                "department" => Some(row.get::<Option<i64>, _>("department_id")),
                "role" => Some(row.get::<Option<i64>, _>("role_id")),
                _ => None,
            };
            let query = match (field, node) {
                // agacta tek dugume cozulmuyor: listeye kalir, tahmin edilmez
                (_, Some(None)) => continue,
                ("department", Some(Some(id))) => {
                    sqlx::query("UPDATE identities SET department_id = $1 WHERE id = $2").bind(id)
                }
                (_, Some(Some(id))) => {
                    sqlx::query("UPDATE identities SET primary_role_id = $1 WHERE id = $2").bind(id)
                }
                (_, None) => sqlx::query(update_sql(field)).bind(to),
            };
            let done = query
                .bind(identity)
                .execute(&mut *tx)
                .await
                .map_err(|e| format!("AD değişikliği yazılamadı: {e}"))?
                .rows_affected();
            if done == 0 {
                continue;
            }
            sqlx::query(
                "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
                 VALUES ($1, $2, $3, jsonb_build_object('source', 'ad_auto', \
                 'target_system_id', $3, 'field', $4::text, 'from', $5::text, 'to', $6::text))",
            )
            .bind(TAKEN_EVENT)
            .bind(identity)
            .bind(target)
            .bind(field)
            .bind(from)
            .bind(to)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("AD değişikliği denetime yazılamadı: {e}"))?;
            taken += 1;
        }
        if taken == 0 {
            continue;
        }
        // Departman/rol OU'yu ve gruplari, ad turemis oznitelikleri belirler: tek is.
        sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority) VALUES ($1, $2, 1) \
             ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' DO NOTHING",
        )
        .bind(identity)
        .bind(target)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("AD değişikliği sonrası iş açılamadı: {e}"))?;
        changed_ids.push(identity);
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    for identity in &changed_ids {
        crate::log::audit(TAKEN_EVENT, None, Some(*identity), Some(target), None);
    }
    Ok(changed_ids.len())
}

/// backend `audit::IDENTITY_FIELD_TAKEN` ile ayni olay; kaynak `ad_auto`.
pub const TAKEN_EVENT: &str = "identity.field_taken";
// --- END FEATURE: ad-change-sync ---

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
            title: None,
            mail: Some(format!("{sam}@hogwarts.local")),
            mobile: None,
            telephone: Some("01632 960001".to_string()),
            when_created: Some("2024-09-01".to_string()),
            national_id: None,
            manager_dn: None,
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

    /// ADR-129: `manager` DN'i ayni taramanin hesap listesinden GUID'e cevrilir
    /// (harf buyuklugune duyarsiz); kapsam disindaki yonetici bos kalir.
    #[test]
    fn the_scan_resolves_the_manager_dn_to_the_managers_account_guid() {
        let head = account("g-1", "adumbledore", true);
        let mut teacher = account("g-2", "ssnape", true);
        // AD DN'i buyuk harfle dondurebilir; eslesme duyarsiz olmali
        teacher.manager_dn = Some(head.dn.to_uppercase());
        let mut outsider = account("g-3", "afilch", true);
        outsider.manager_dn = Some("CN=Kapsam Disi,OU=Baska,DC=hogwarts,DC=local".to_string());
        let head_guid = head.guid.clone();

        let findings = compare(&[head, teacher, outsider], &[]);
        let of = |sam: &str| {
            findings
                .iter()
                .find(|f| f.account_name == sam)
                .expect("bulgu yok")
        };
        assert_eq!(
            of("ssnape").manager_external_id,
            Some(head_guid),
            "yöneticinin hesabının GUID'i bulguya yazılır"
        );
        assert_eq!(
            of("afilch").manager_external_id,
            None,
            "kapsam dışındaki yönetici çözülmez"
        );
        assert_eq!(
            of("adumbledore").manager_external_id,
            None,
            "`manager` özniteliği boş olan hesap boş kalır"
        );
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

    // ADR-120 madde 5: yer tutucu rol "bos" sayilir ve AD'deki unvanin agactaki
    // karsiligindan dolar; gercek bir rol duruyorsa dokunulmaz, unvan iki role
    // karsilik geliyorsa eslesme yok sayilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_scan_fills_a_placeholder_role_from_the_directory_job_title() {
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
        let placeholder: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE placeholder")
            .fetch_one(&pool)
            .await
            .unwrap();
        let expected: i64 =
            sqlx::query_scalar("SELECT id FROM roles WHERE title = 'Sistem Uzmanı'")
                .fetch_one(&pool)
                .await
                .unwrap();
        // Ayse AD'den sahiplenilmis gibi yer tutucu rolde; Ali'nin rolu gercek
        let set_placeholder = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query("UPDATE identities SET primary_role_id = $1 WHERE id = $2")
                    .bind(id)
                    .bind(placeholder)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        set_placeholder(seed.identity).await;

        // Ayse'nin unvani yazim farkiyla ayni role isaret ediyor, Ali'nin
        // unvaninin agacta karsiligi yok (zaten gercek rolu de var)
        let mut ayse = account("g-1", "ayse.yilmaz", true);
        ayse.title = Some("  sistem uzmanı ".to_string());
        let mut ali = account("g-2", "ali.kaya", true);
        ali.title = Some("Olmayan Unvan".to_string());
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
            &[9u8; crate::crypto::KEY_LEN],
        )
        .await
        .unwrap();

        let role_of = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT primary_role_id FROM identities WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(fill_placeholder_roles(&pool, seed.ad).await.unwrap(), 1);
        assert_eq!(role_of(seed.identity).await, expected);
        assert_eq!(
            role_of(seed.other_identity).await,
            expected,
            "gerçek rol AD'deki unvandan değişmez"
        );
        let detail: String = sqlx::query_scalar(
            "SELECT detail::text FROM audit_log WHERE event_type = $1 AND identity_id = $2",
        )
        .bind(FILLED_EVENT)
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(detail.contains("primary_role"), "{detail}");

        assert_eq!(
            fill_placeholder_roles(&pool, seed.ad).await.unwrap(),
            0,
            "yer tutucu kalmadi, ikinci tarama dolum yapmaz"
        );

        // Unvan iki role karsilik geliyorsa eslesme yok sayilir (ad tekil,
        // unvan degil): yer tutucu yerinde kalir
        sqlx::query(
            "INSERT INTO roles (kind, name, title) VALUES ('primary', 'İkinci', 'Sistem Uzmanı')",
        )
        .execute(&pool)
        .await
        .unwrap();
        set_placeholder(seed.identity).await;
        assert_eq!(fill_placeholder_roles(&pool, seed.ad).await.unwrap(), 0);
        assert_eq!(role_of(seed.identity).await, placeholder);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
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
        // ADR-129: Ayse kendi kendisinin yoneticisi gosterilmis (koruma denenir)
        ayse.manager_dn = Some(ayse.dn.clone());
        // Ali'nin cebi yok (sabit hat E.164 degil) ve sicili Ayse'nin sicili
        let mut ali = account("g-2", "ali.kaya", true);
        ali.employee_number = Some("7788".to_string());
        // ADR-129: Ali'nin AD'deki yoneticisi Ayse'nin hesabi
        ali.manager_dn = Some(ayse.dn.clone());
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
        assert!(
            detail.contains("manager"),
            "yönetici de dolan alan: {detail}"
        );

        // ADR-129: yonetici AD'den dolar; kendi kendisini yonetici gosteren
        // hesapta alan bos kalir.
        let manager_of = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT manager_id FROM identities WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(
            manager_of(seed.other_identity).await,
            Some(seed.identity),
            "Ali'nin yöneticisi AD'deki DN'den çözülüp kimliğe yazılır"
        );
        assert_eq!(
            manager_of(seed.identity).await,
            None,
            "kendi kendisinin yöneticisi olan hesapta alan boş kalır"
        );

        assert_eq!(
            fill_linked_identities(&pool, seed.ad).await.unwrap(),
            0,
            "ikinci tarama dolacak alan bulmaz"
        );

        // ADR-024 kisisel veri temizligi geri doldurulmaz. (Sicil serbest kaldigi
        // icin ayni taramada Ali'nin sicili dolar; bakilan sey silinmis kimlik.)
        sqlx::query(
            "UPDATE identities SET deleted_at = now(), username = NULL, email = NULL, \
             mobile_phone = NULL, employee_number = NULL, manager_id = NULL WHERE id = $1",
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

    fn person(given: &str, employee: &str, department: &str) -> PersonFields {
        PersonFields {
            given_name: Some(given.to_string()),
            employee_number: Some(employee.to_string()),
            department: Some(department.to_string()),
            ..PersonFields::default()
        }
    }

    // ADR-138 madde 1: yalnizca AD'de degisen alan alinir.
    #[test]
    fn only_a_directory_side_change_is_taken() {
        let old = person("Draco", "9", "Slytherin");
        let ours = person("Draco", "9", "Slytherin");

        // AD degismedi: hicbir sey
        assert!(ad_changes(&old, &old, &ours).is_empty());

        // AD'de ad ve sicilin yazimi degisti; departmanda yalnizca harf buyuklugu
        let new = person("Drako", "00000000009", "slytherin ");
        assert_eq!(
            ad_changes(&old, &new, &ours),
            vec![
                ("given_name", "Drako", "Draco"),
                ("employee_number", "00000000009", "9")
            ]
        );

        // sitede de degismis: cakisma, alinmaz (listeye kalir)
        let ours_changed = person("Dray", "9", "Slytherin");
        // ad sitede de degismis: cakisma, alinmaz; sicil yalnizca AD'de degisti
        assert_eq!(
            ad_changes(&old, &new, &ours_changed),
            vec![("employee_number", "00000000009", "9")]
        );

        // taban/AD/biz bos: alinmaz (bosaltma siteye tasinmaz, bos alani dolum doldurur)
        let empty = PersonFields::default();
        assert!(ad_changes(&empty, &new, &ours).is_empty());
        assert!(ad_changes(&old, &empty, &ours).is_empty());
        assert!(ad_changes(&old, &new, &empty).is_empty());

        // gercek sicil degisikligi alinir
        let new = person("Draco", "10", "Slytherin");
        assert_eq!(
            ad_changes(&old, &new, &ours),
            vec![("employee_number", "10", "9")]
        );
    }

    #[test]
    fn only_a_faithful_mapping_lets_the_read_lane_take_a_field() {
        let row = |a: &str, k: &str, t: &str| (a.to_string(), k.to_string(), t.to_string());
        assert!(echoed_fields(&[]).is_empty(), "eşleme yok: AD'nin değeri");
        let defaults = [
            row("givenName", "given_name", "none"),
            row("sn", "surname", "none"),
            row("displayName", "template", "none"),
            row("department", "department_name", "none"),
            row("title", "title", "none"),
            row("employeeID", "employee_number", "none"),
            row("mobile", "mobile_phone", "phone_national"),
        ];
        assert!(echoed_fields(&defaults).is_empty(), "varsayılan eşlemeler");
        let echoes = [
            row("title", "constant", "none"),
            row("department", "template", "none"),
            row("employeeNumber", "template", "none"),
            row("mobile", "constant", "none"),
            row("givenName", "given_name", "lower"),
            row("sn", "given_name", "none"),
        ];
        assert_eq!(
            echoed_fields(&echoes),
            [
                "given_name",
                "surname",
                "employee_number",
                "mobile_phone",
                "department",
                "role"
            ]
        );
    }

    // ADR-138 uctan uca: ilk taramada taban yok; ikinci taramada yalnizca AD'de
    // degisen alan kimlige yazilir, iki tarafta degisen alana dokunulmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn a_directory_side_change_reaches_the_identity_on_the_next_scan() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let seed = crate::test_support::seed_example_model(&pool).await;
        let key = [7u8; crate::crypto::KEY_LEN];
        for (id, given, surname, employee) in [
            (seed.identity, "Ayşe", "Yılmaz", "100"),
            (seed.other_identity, "Ali", "Kaya", "200"),
        ] {
            sqlx::query(
                "UPDATE identities SET given_name = $2, surname = $3, employee_number = $4, \
                 mobile_phone = '+905321111111' WHERE id = $1",
            )
            .bind(id)
            .bind(given)
            .bind(surname)
            .bind(employee)
            .execute(&pool)
            .await
            .unwrap();
        }
        let links = [
            link("g-1", MANAGED, seed.identity, "?"),
            link("g-2", OBSERVED, seed.other_identity, "?"),
        ];
        let accounts = |ayse_given: &str, ayse_mobile: &str, ayse_employee: &str, ali_sn: &str| {
            let mut ayse = account("g-1", "ayse.yilmaz", true);
            ayse.given_name = Some(ayse_given.to_string());
            ayse.surname = Some("Yılmaz".to_string());
            ayse.employee_number = Some(ayse_employee.to_string());
            ayse.mobile = Some(ayse_mobile.to_string());
            let mut ali = account("g-2", "ali.kaya", true);
            ali.given_name = Some("Ali".to_string());
            ali.surname = Some(ali_sn.to_string());
            ali.employee_number = Some("200".to_string());
            ali.mobile = Some("+905321111111".to_string());
            vec![ayse, ali]
        };
        let scan = |accs: Vec<DirectoryAccount>| {
            let pool = pool.clone();
            let links = links.clone();
            async move {
                let job: i64 = sqlx::query_scalar(
                    "INSERT INTO read_jobs (kind, target_system_id, requested_by, status) \
                     VALUES ('reconcile', $1, 'test', 'succeeded') RETURNING id",
                )
                .bind(seed.ad)
                .fetch_one(&pool)
                .await
                .unwrap();
                let previous = load_previous(&pool, seed.ad).await.unwrap();
                store(&pool, seed.ad, job, &compare(&accs, &links), &key)
                    .await
                    .unwrap();
                take_ad_changes(&pool, seed.ad, &previous).await.unwrap()
            }
        };

        assert_eq!(
            scan(accounts("Ayşe", "+905321111111", "100", "Kaya")).await,
            0,
            "ilk taramada taban yok"
        );
        // sitede Ali'nin soyadi degisti (gozlem modu, AD'ye gitmedi)
        sqlx::query("UPDATE identities SET surname = 'Kaya-Site' WHERE id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        // AD'de: Ayse'nin adi ve cebi degisti, sicilin yazimi degisti (sifirlar);
        // Ali'nin soyadi AD'de de degisti (cakisma)
        assert_eq!(
            scan(accounts(
                "Ayşegül",
                "+905322222222",
                "00000000100",
                "Kaya-AD"
            ))
            .await,
            1
        );
        let fields = |id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
                    "SELECT given_name, surname, employee_number, mobile_phone \
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
                "Ayşegül".into(),
                "Yılmaz".into(),
                Some("00000000100".into()),
                Some("+905322222222".into())
            )
        );
        assert_eq!(
            fields(seed.other_identity).await.1,
            "Kaya-Site",
            "iki tarafta değişen alan listeye kalır"
        );
        let audit: Vec<String> = sqlx::query_scalar(
            "SELECT detail->>'field' FROM audit_log WHERE event_type = $1 \
             AND detail->>'source' = 'ad_auto' AND identity_id = $2 ORDER BY id",
        )
        .bind(TAKEN_EVENT)
        .bind(seed.identity)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(audit, ["given_name", "employee_number", "mobile_phone"]);
        let jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE identity_id = $1 AND target_system_id = $2",
        )
        .bind(seed.identity)
        .bind(seed.ad)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(jobs, 1, "kimlik için tek iş");

        assert_eq!(
            scan(accounts(
                "Ayşegül",
                "+905322222222",
                "00000000100",
                "Kaya-AD"
            ))
            .await,
            0,
            "AD değişmedi: tekrar alınmaz"
        );

        // Guvenlik denetimi OS-05: eslememizin AD'ye yazdigi deger yanki, alinmaz
        sqlx::query(
            "INSERT INTO attribute_mappings (target_system_id, target_attribute, source_kind, source_text) \
             VALUES ($1, 'employeeID', 'template', '{employee_number}9'), \
                    ($1, 'givenName', 'constant', 'Mallory') \
             ON CONFLICT (target_system_id, target_attribute) DO UPDATE \
             SET source_kind = EXCLUDED.source_kind, source_text = EXCLUDED.source_text",
        )
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            scan(accounts(
                "Mallory",
                "+905322222222",
                "000000001009",
                "Kaya-AD"
            ))
            .await,
            0,
            "sabit ve şablon yankısı alınmaz"
        );
        assert_eq!(
            fields(seed.identity).await,
            (
                "Ayşegül".into(),
                "Yılmaz".into(),
                Some("00000000100".into()),
                Some("+905322222222".into())
            )
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
