// --- START FEATURE: ad-field-diff ---
// "AD'de farkli" listesi ve toplu "AD'dekini al" (ADR-112 madde 2).
//
// Bos alan dolumu otomatiktir ve worker'da durur (`reconcile::fill_linked_identities`).
// Ikisi de doluysa karar insanda kalir: bu modul farki hesaplar, operator satir
// secer, secilen alan AD'deki degerle degisir.
//
// Backend AD'ye baglanmaz (ADR-004): AD'deki deger son mutabakat taramasinin
// `reconcile_findings` satirinda duruyor. Hedefe yazma burada da yok.
//
// Alan kumesi uc alandir: sicil, cep ve departman (ADR-120).
// `username`/`email` OpenSicil'in uretimidir (ADR-011/022/035/042) ve silinmede
// `used_names`'te yakilir; AD'dekini "almak" o kaydi atlardi, bu baska bir is.
//
// Sicil ve cepte is acilmaz: deger artik AD'dekiyle ayni, motorun gorecegi fark
// yok. Departmanda **acilir** (ADR-120 madde 3) — departman konteyneri (OU) ve
// hak setini (grup) belirler, is acilmazsa kimlik yeni departmani soyler ama
// hesap eski OU'da eski gruplariyla kalir.
use sqlx::PgPool;

use crate::identity;

/// "AD'dekini al" yetkisi: kimligin kisi alanini degistiren islem, kayit
/// yetkisiyle ayni kapi. `auditor` listeyi gorur, eylemi goremez.
pub const AUTHORITIES: &[&str] = &["hr", "admin"];

/// Alinabilir tek bir fark: kisi · alan · AD'deki deger · OpenSicil'deki deger.
pub struct Diff {
    pub identity_id: i64,
    pub person: String,
    pub account_name: String,
    /// `employee_number` | `mobile_phone` | `department`
    pub field: &'static str,
    pub ad: String,
    pub ours: String,
    /// Departmanda: AD'nin serbest metninin agacta karsilik geldigi dugum.
    /// `None` ise satir listelenir ama alinamaz (ADR-120 madde 2). Diger
    /// alanlarda anlamsizdir ve hep `None` durur.
    pub department_id: Option<i64>,
}

impl Diff {
    /// Onay kutusunun degeri: `<kimlik>.<alan>`. Geri gelen deger degil anahtar
    /// tasir — alinacak deger formdan degil veritabanindan okunur.
    pub fn key(&self) -> String {
        format!("{}.{}", self.identity_id, self.field)
    }

    /// Alan etiketinin i18n anahtari
    pub fn label(&self) -> &'static str {
        match self.field {
            "employee_number" => "field.employee_number",
            "department" => "field.department",
            _ => "field.mobile_phone",
        }
    }
}

/// Bir bagli bulgunun kimlikle yan yana hali; `diffs` bunun saf girdisi.
pub struct Pair {
    pub identity_id: i64,
    pub person: String,
    pub account_name: String,
    pub ad_employee_number: Option<String>,
    pub ad_mobile: Option<String>,
    pub ad_telephone: Option<String>,
    pub ad_department: Option<String>,
    /// `ad_department`'in agactaki karsiligi; eslesme yoksa `None`
    pub ad_department_id: Option<i64>,
    pub employee_number: Option<String>,
    pub mobile_phone: Option<String>,
    /// Kimligin departman adi; `department_id` `NOT NULL` oldugundan hep dolu
    pub department_name: String,
}

/// ADR-018 madde 5 / ADR-042: bastaki sifirlar ve bosluklar atilarak esit mi.
/// `worker/src/adoption.rs:employee_number_matches` ikizi — crate'ler bagimsiz
/// (ADR-070), fonksiyon paylasilamiyor.
fn same_employee_number(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches('0').to_string();
    norm(a) == norm(b)
}

fn filled(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// AD'de duran telefon: once cep, cep bossa sabit hat. Kimlikteki alan E.164
/// cep numarasidir (docs/03); sabit hat biciminde bir deger oraya yazilmaz, o
/// yuzden listeye de girmez — alinabilir olmayan satir operatore secenek
/// gostermez (`writable_phone` kurali, ADR-106).
fn writable_phone(p: &Pair) -> Option<&str> {
    filled(&p.ad_mobile)
        .or_else(|| filled(&p.ad_telephone))
        .filter(|v| identity::valid_e164(v.to_string()).is_ok())
}

/// Departman adlari ayni mi: AD serbest metin yazar, bosluk ve buyuk/kucuk
/// harf fark sayilmaz. `to_lowercase` Unicode'a gore katlar — `Ş`/`ş`, `Ö`/`ö`
/// gibi harfleri `eq_ignore_ascii_case` kacirirdi. Turkce'nin noktali/noktasiz
/// i'si ne burada ne SQL `lower()`'da cozulur; o durumda satir "farkli" diye
/// listelenir ve agacta eslesmedigi icin alinamaz — sessiz yanlis eslesme yok.
fn same_department(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Yalnizca **ikisi de dolu ve farkli** olan alanlar. Bos alan burada cikmaz:
/// onu gece taramasi onaysiz doldurur (ADR-112 madde 1). Departman bizde hic
/// bos olamaz (`department_id` `NOT NULL`), o yuzden tek kurali bu liste
/// (ADR-120): AD tarafi dolu ve ad farkliysa satir cikar.
pub fn diffs(p: &Pair) -> Vec<Diff> {
    let row = |field: &'static str, ad: &str, ours: &str| Diff {
        identity_id: p.identity_id,
        person: p.person.clone(),
        account_name: p.account_name.clone(),
        field,
        ad: ad.to_string(),
        ours: ours.to_string(),
        department_id: None,
    };
    let mut out = Vec::new();
    if let (Some(ad), Some(ours)) = (filled(&p.ad_employee_number), filled(&p.employee_number)) {
        if !same_employee_number(ad, ours) {
            out.push(row("employee_number", ad, ours));
        }
    }
    if let (Some(ad), Some(ours)) = (writable_phone(p), filled(&p.mobile_phone)) {
        if ad != ours {
            out.push(row("mobile_phone", ad, ours));
        }
    }
    if let Some(ad) = filled(&p.ad_department) {
        if !same_department(ad, &p.department_name) {
            out.push(Diff {
                department_id: p.ad_department_id,
                ..row("department", ad, &p.department_name)
            });
        }
    }
    out
}

// AD'nin serbest metin departmani agacta ada gore cozulur — `bulk_adopt`'un
// sahiplenme adaylarinda kullandigi kuralin ayni. `departments.name` tekil
// degil (yalnizca `code` ve `slug` tekil): ad iki dugumde duruyorsa
// `HAVING count(*) = 1` eslesmeyi yok sayar, hangisi oldugu tahmin edilmez.
const PAIRS_SQL: &str = "SELECT f.identity_id, i.given_name || ' ' || i.surname, \
    f.account_name, f.employee_number, f.mobile, f.telephone_number, \
    f.department_name, \
    (SELECT max(d.id) FROM departments d \
       WHERE lower(d.name) = lower(f.department_name) HAVING count(*) = 1), \
    i.employee_number, i.mobile_phone, dep.name \
    FROM reconcile_findings f JOIN identities i ON i.id = f.identity_id \
    JOIN departments dep ON dep.id = i.department_id \
    WHERE f.target_system_id = $1 AND i.deleted_at IS NULL \
    ORDER BY i.given_name, i.surname, f.identity_id";

/// identity_id, kisi, hesap adi, AD sicil, AD cep, AD sabit hat, AD departman,
/// AD departmaninin agactaki id'si, sicil, cep, departman adi
type PairRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
);

pub async fn list(pool: &PgPool, target: i64) -> Result<Vec<Diff>, sqlx::Error> {
    let rows: Vec<PairRow> = sqlx::query_as(PAIRS_SQL)
        .bind(target)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .flat_map(
            |(
                identity_id,
                person,
                account_name,
                ad_employee_number,
                ad_mobile,
                ad_telephone,
                ad_department,
                ad_department_id,
                employee_number,
                mobile_phone,
                department_name,
            )| {
                diffs(&Pair {
                    identity_id,
                    person,
                    account_name,
                    ad_employee_number,
                    ad_mobile,
                    ad_telephone,
                    ad_department,
                    ad_department_id,
                    employee_number,
                    mobile_phone,
                    department_name,
                })
            },
        )
        .collect())
}

/// Alinan tek alan; denetim satiri bundan yazilir.
pub struct Taken {
    pub identity_id: i64,
    pub field: &'static str,
    pub from: String,
    pub to: String,
}

#[derive(Default)]
pub struct Outcome {
    pub taken: Vec<Taken>,
    /// Yazilamayan satirlar: "kisi (alan)" — bugun tek neden mukerrer sicil
    pub skipped: Vec<String>,
}

// Sicil tekil kolondur: ayni deger baska bir kimlikte duruyorsa yazilmaz
// (ADR-112 madde 1 ile ayni kural). Kosul SQL'de duruyor, kontrol-sonra-yaz
// yarisi yok; `rows_affected() == 0` "atlandi" demektir.
const TAKE_EMPLOYEE_SQL: &str = "UPDATE identities SET employee_number = $2 WHERE id = $1 \
    AND NOT EXISTS (SELECT 1 FROM identities o WHERE o.employee_number = $2 AND o.id <> $1)";
const TAKE_PHONE_SQL: &str = "UPDATE identities SET mobile_phone = $2 WHERE id = $1";
// Departman id'si `list`'te cozuldu; burada ad eslestirmesi tekrarlanmaz.
const TAKE_DEPARTMENT_SQL: &str = "UPDATE identities SET department_id = $2 WHERE id = $1";

/// Secilen satirlarda AD'deki degeri kimlige yazar. Secim yalnizca anahtar
/// tasir: deger ekranda gorulen degil, su an veritabaninda duran AD degeridir.
/// Departman alinirsa hedefe is acilir (ADR-120 madde 3); sicil ve cep hak
/// seti degistirmedigi icin is acmaz.
pub async fn take(pool: &PgPool, target: i64, selected: &[&str]) -> Result<Outcome, sqlx::Error> {
    let mut outcome = Outcome::default();
    for diff in list(pool, target).await? {
        if !selected.contains(&diff.key().as_str()) {
            continue;
        }
        let affected = match (diff.field, diff.department_id) {
            // AD'nin metni agacta bir departman adi degil: satir atlanir,
            // yeni departman acilmaz, en yakin ad tahmin edilmez
            ("department", None) => 0,
            ("department", Some(department)) => sqlx::query(TAKE_DEPARTMENT_SQL)
                .bind(diff.identity_id)
                .bind(department)
                .execute(pool)
                .await?
                .rows_affected(),
            ("employee_number", _) => sqlx::query(TAKE_EMPLOYEE_SQL)
                .bind(diff.identity_id)
                .bind(&diff.ad)
                .execute(pool)
                .await?
                .rows_affected(),
            _ => sqlx::query(TAKE_PHONE_SQL)
                .bind(diff.identity_id)
                .bind(&diff.ad)
                .execute(pool)
                .await?
                .rows_affected(),
        };
        if affected == 0 {
            outcome
                .skipped
                .push(format!("{} ({})", diff.person, diff.ad));
            continue;
        }
        if diff.field == "department" {
            crate::jobs::enqueue(
                pool,
                diff.identity_id,
                target,
                crate::jobs::Priority::Single,
            )
            .await?;
        }
        outcome.taken.push(Taken {
            identity_id: diff.identity_id,
            field: diff.field,
            from: diff.ours,
            to: diff.ad,
        });
    }
    Ok(outcome)
}
// --- END FEATURE: ad-field-diff ---

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> Pair {
        Pair {
            identity_id: 1,
            person: "Draco Malfoy".into(),
            account_name: "draco.malfoy".into(),
            ad_employee_number: None,
            ad_mobile: None,
            ad_telephone: None,
            ad_department: None,
            ad_department_id: None,
            employee_number: None,
            mobile_phone: None,
            department_name: "Hogwarts".into(),
        }
    }

    #[test]
    fn department_compares_names_and_carries_the_resolved_node() {
        let mut p = pair();

        // AD tarafi bos: satir yok (bizdeki departman `NOT NULL`, hep dolu)
        assert!(diffs(&p).is_empty());

        // ayni ad, farkli yazim: fark degil
        p.ad_department = Some("  hogwarts ".into());
        assert!(diffs(&p).is_empty());

        // ASCII disi harf de katlanir
        p.department_name = "Şifa Bölümü".into();
        p.ad_department = Some("ŞIFA BÖLÜMÜ".into());
        assert!(diffs(&p).is_empty());

        // gercek fark, agacta eslesiyor: alinabilir
        p.department_name = "Hogwarts".into();
        p.ad_department = Some("Slytherin".into());
        p.ad_department_id = Some(8);
        let rows = diffs(&p);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "department");
        assert_eq!(rows[0].label(), "field.department");
        assert_eq!(rows[0].key(), "1.department");
        assert_eq!(
            (rows[0].ad.as_str(), rows[0].ours.as_str()),
            ("Slytherin", "Hogwarts")
        );
        assert_eq!(rows[0].department_id, Some(8));

        // agacta eslesmeyen ad: satir yine listelenir (operator farki gorur)
        // ama `department_id` bos, `take` onu atlar
        p.ad_department = Some("Slytherin Evi".into());
        p.ad_department_id = None;
        assert_eq!(diffs(&p)[0].department_id, None);
    }

    #[test]
    fn only_both_filled_and_really_different_rows_are_listed() {
        // bos alan listede cikmaz: onu dolum halleder
        let mut p = pair();
        p.ad_employee_number = Some("00000000009".into());
        assert!(diffs(&p).is_empty());

        // bastaki sifir fark degil
        p.employee_number = Some("9".into());
        assert!(diffs(&p).is_empty());

        // gercek fark
        p.employee_number = Some("10".into());
        let rows = diffs(&p);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "employee_number");
        assert_eq!(
            (rows[0].ad.as_str(), rows[0].ours.as_str()),
            ("00000000009", "10")
        );
        assert_eq!(rows[0].key(), "1.employee_number");
    }

    #[test]
    fn phone_falls_back_to_landline_but_only_in_e164() {
        let mut p = pair();
        p.mobile_phone = Some("+905000000000".into());

        // sabit hat bicimi kimligin cep alanina yazilamaz: satir hic cikmaz
        p.ad_telephone = Some("01632 960001".into());
        assert!(diffs(&p).is_empty());

        // E.164 sabit hat yedege gecer
        p.ad_telephone = Some("+441632960001".into());
        assert_eq!(diffs(&p).len(), 1);

        // cep varsa sabit hat okunmaz
        p.ad_mobile = Some("+447700900009".into());
        let rows = diffs(&p);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ad, "+447700900009");
        assert_eq!(rows[0].label(), "field.mobile_phone");

        // ayni deger fark degil
        p.ad_mobile = Some("+905000000000".into());
        assert!(diffs(&p).is_empty());
    }
}
