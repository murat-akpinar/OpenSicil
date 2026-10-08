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
// Alan kumesi dort alandir: sicil, cep, departman ve rol (ADR-120).
// `username`/`email` OpenSicil'in uretimidir (ADR-011/022/035/042) ve silinmede
// `used_names`'te yakilir; AD'dekini "almak" o kaydi atlardi, bu baska bir is.
//
// Sicil ve cepte is acilmaz: deger artik AD'dekiyle ayni, motorun gorecegi fark
// yok. Departmanda ve rolde **acilir** (ADR-120 madde 3) — ikisi de konteyneri
// (OU) ve hak setini (grup) belirler, is acilmazsa kimlik yeni degeri soyler ama
// hesap eski OU'da eski gruplariyla kalir.
//
// Rolde karsilastirilan deger rolun **adi** degil `roles.title` kolonu: o kolon
// rolun AD'deki `title` karsiligidir (ADR-120 madde 5). Yer tutucu rol burada
// "dolu" sayilmaz — onu gece taramasi onaysiz doldurur
// (`worker/src/reconcile.rs::fill_placeholder_roles`).
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
    /// `employee_number` | `mobile_phone` | `department` | `role`
    pub field: &'static str,
    pub ad: String,
    pub ours: String,
    /// Departman ve rolde: AD'nin serbest metninin agacta karsilik geldigi
    /// dugum. `None` ise satir listelenir ama alinamaz (ADR-120 madde 2).
    /// Diger alanlarda anlamsizdir ve hep `None` durur.
    pub node_id: Option<i64>,
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
            "given_name" => "field.given_name",
            "surname" => "field.surname",
            "employee_number" => "field.employee_number",
            "department" => "field.department",
            "role" => "field.role",
            _ => "field.mobile_phone",
        }
    }
}

/// Bir bagli bulgunun kimlikle yan yana hali; `diffs` bunun saf girdisi.
pub struct Pair {
    pub identity_id: i64,
    pub person: String,
    pub account_name: String,
    /// AD'deki ad/soyad (ADR-138): iki tarafta da degismisse listeye duser
    pub ad_given_name: Option<String>,
    pub ad_surname: Option<String>,
    pub ad_employee_number: Option<String>,
    pub ad_mobile: Option<String>,
    pub ad_telephone: Option<String>,
    pub ad_department: Option<String>,
    /// `ad_department`'in agactaki karsiligi; eslesme yoksa `None`
    pub ad_department_id: Option<i64>,
    /// AD'deki unvan (`title`)
    pub ad_title: Option<String>,
    /// `ad_title`'in unvani olan birincil rol; eslesme yoksa `None`
    pub ad_role_id: Option<i64>,
    pub given_name: String,
    pub surname: String,
    pub employee_number: Option<String>,
    pub mobile_phone: Option<String>,
    /// Kimligin departman adi; `department_id` `NOT NULL` oldugundan hep dolu
    pub department_name: String,
    /// Kimligin birincil rolunun unvani; yer tutucu rolde ve unvan girilmemis
    /// rolde `None` — karsilastirilacak degerimiz yok, satir cikmaz
    pub role_title: Option<String>,
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

/// Agac dugumunun adi ile AD'nin serbest metni ayni mi: bosluk ve buyuk/kucuk
/// harf fark sayilmaz. `to_lowercase` Unicode'a gore katlar — `Ş`/`ş`, `Ö`/`ö`
/// gibi harfleri `eq_ignore_ascii_case` kacirirdi. Turkce'nin noktali/noktasiz
/// i'si ne burada ne SQL `lower()`'da cozulur; o durumda satir "farkli" diye
/// listelenir ve agacta eslesmedigi icin alinamaz — sessiz yanlis eslesme yok.
fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Yalnizca **ikisi de dolu ve farkli** olan alanlar. Bos alan burada cikmaz:
/// onu gece taramasi onaysiz doldurur (ADR-112 madde 1). Departman bizde hic
/// bos olamaz (`department_id` `NOT NULL`), o yuzden tek kurali bu liste
/// (ADR-120): AD tarafi dolu ve ad farkliysa satir cikar. Rolde "bos" yer
/// tutucu roldur ve `role_title` `None` gelir — o satir da burada cikmaz.
pub fn diffs(p: &Pair) -> Vec<Diff> {
    let row = |field: &'static str, ad: &str, ours: &str| Diff {
        identity_id: p.identity_id,
        person: p.person.clone(),
        account_name: p.account_name.clone(),
        field,
        ad: ad.to_string(),
        ours: ours.to_string(),
        node_id: None,
    };
    let mut out = Vec::new();
    for (field, ad, ours) in [
        ("given_name", &p.ad_given_name, &p.given_name),
        ("surname", &p.ad_surname, &p.surname),
    ] {
        if let Some(ad) = filled(ad) {
            if ad != ours.trim() {
                out.push(row(field, ad, ours));
            }
        }
    }
    if let (Some(ad), Some(ours)) = (filled(&p.ad_employee_number), filled(&p.employee_number)) {
        // Yazim birebir (ADR-138 ek): `00000000009` ile `9` farktir; bastaki
        // sifiri yok sayan kural yalnizca sahiplenme eslestirmesinde
        if ad != ours {
            out.push(row("employee_number", ad, ours));
        }
    }
    if let (Some(ad), Some(ours)) = (writable_phone(p), filled(&p.mobile_phone)) {
        if ad != ours {
            out.push(row("mobile_phone", ad, ours));
        }
    }
    if let Some(ad) = filled(&p.ad_department) {
        if !same_name(ad, &p.department_name) {
            out.push(Diff {
                node_id: p.ad_department_id,
                ..row("department", ad, &p.department_name)
            });
        }
    }
    if let (Some(ad), Some(ours)) = (filled(&p.ad_title), filled(&p.role_title)) {
        if !same_name(ad, ours) {
            out.push(Diff {
                node_id: p.ad_role_id,
                ..row("role", ad, ours)
            });
        }
    }
    out
}

// AD'nin serbest metin departmani agacta ada gore cozulur — `bulk_adopt`'un
// sahiplenme adaylarinda kullandigi kuralin ayni. `departments.name` tekil
// degil (yalnizca `code` ve `slug` tekil): ad iki dugumde duruyorsa
// `HAVING count(*) = 1` eslesmeyi yok sayar, hangisi oldugu tahmin edilmez.
// Rolde ayni kural `roles.title` uzerinde kosar (`roles.name` tekil, unvan
// degil) ve yer tutucu rol iki yerde de disarida kalir: unvani AD'ye yazilmaz,
// dolumu gece taramasinin isidir.
const PAIRS_SQL: &str = "SELECT f.identity_id, i.given_name || ' ' || i.surname, \
    f.account_name, f.given_name, f.surname, f.employee_number, f.mobile, f.telephone_number, \
    f.department_name, \
    (SELECT max(d.id) FROM departments d \
       WHERE lower(d.name) = lower(f.department_name) HAVING count(*) = 1), \
    f.title, \
    (SELECT max(r.id) FROM roles r \
       WHERE r.kind = 'primary' AND NOT r.placeholder \
         AND lower(btrim(r.title)) = lower(btrim(f.title)) HAVING count(*) = 1), \
    i.given_name, i.surname, i.employee_number, i.mobile_phone, dep.name, \
    CASE WHEN rol.placeholder THEN NULL ELSE rol.title END \
    FROM reconcile_findings f JOIN identities i ON i.id = f.identity_id \
    JOIN departments dep ON dep.id = i.department_id \
    JOIN roles rol ON rol.id = i.primary_role_id \
    WHERE f.target_system_id = $1 AND i.deleted_at IS NULL \
    ORDER BY i.given_name, i.surname, f.identity_id";

pub async fn list(pool: &PgPool, target: i64) -> Result<Vec<Diff>, sqlx::Error> {
    use sqlx::Row;
    let rows = sqlx::query(PAIRS_SQL).bind(target).fetch_all(pool).await?;
    Ok(rows
        .iter()
        .flat_map(|r| {
            diffs(&Pair {
                identity_id: r.get(0),
                person: r.get(1),
                account_name: r.get(2),
                ad_given_name: r.get(3),
                ad_surname: r.get(4),
                ad_employee_number: r.get(5),
                ad_mobile: r.get(6),
                ad_telephone: r.get(7),
                ad_department: r.get(8),
                ad_department_id: r.get(9),
                ad_title: r.get(10),
                ad_role_id: r.get(11),
                given_name: r.get(12),
                surname: r.get(13),
                employee_number: r.get(14),
                mobile_phone: r.get(15),
                department_name: r.get(16),
                role_title: r.get(17),
            })
        })
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
const TAKE_GIVEN_NAME_SQL: &str = "UPDATE identities SET given_name = $2 WHERE id = $1";
const TAKE_SURNAME_SQL: &str = "UPDATE identities SET surname = $2 WHERE id = $1";
const TAKE_PHONE_SQL: &str = "UPDATE identities SET mobile_phone = $2 WHERE id = $1";
// Departman ve rol id'si `list`'te cozuldu; burada ad eslestirmesi tekrarlanmaz.
const TAKE_DEPARTMENT_SQL: &str = "UPDATE identities SET department_id = $2 WHERE id = $1";
const TAKE_ROLE_SQL: &str = "UPDATE identities SET primary_role_id = $2 WHERE id = $1";

/// Secilen satirlarda AD'deki degeri kimlige yazar. Secim yalnizca anahtar
/// tasir: deger ekranda gorulen degil, su an veritabaninda duran AD degeridir.
/// Departman ya da rol alinirsa hedefe is acilir (ADR-120 madde 3); sicil ve
/// cep hak seti degistirmedigi icin is acmaz.
pub async fn take(pool: &PgPool, target: i64, selected: &[&str]) -> Result<Outcome, sqlx::Error> {
    let mut outcome = Outcome::default();
    for diff in list(pool, target).await? {
        if !selected.contains(&diff.key().as_str()) {
            continue;
        }
        let affected = match (diff.field, diff.node_id) {
            // AD'nin metni agacta bir departman adi / rol unvani degil: satir
            // atlanir, yeni dugum acilmaz, en yakin ad tahmin edilmez
            ("department" | "role", None) => 0,
            ("department", Some(node)) => sqlx::query(TAKE_DEPARTMENT_SQL)
                .bind(diff.identity_id)
                .bind(node)
                .execute(pool)
                .await?
                .rows_affected(),
            ("role", Some(node)) => sqlx::query(TAKE_ROLE_SQL)
                .bind(diff.identity_id)
                .bind(node)
                .execute(pool)
                .await?
                .rows_affected(),
            ("given_name" | "surname", _) => sqlx::query(match diff.field {
                "given_name" => TAKE_GIVEN_NAME_SQL,
                _ => TAKE_SURNAME_SQL,
            })
            .bind(diff.identity_id)
            .bind(&diff.ad)
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
        // Ad degisikligi turemis oznitelikleri (displayName) de degistirir (ADR-138)
        if matches!(diff.field, "department" | "role" | "given_name" | "surname") {
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
            ad_given_name: None,
            ad_surname: None,
            ad_employee_number: None,
            ad_mobile: None,
            ad_telephone: None,
            ad_department: None,
            ad_department_id: None,
            ad_title: None,
            ad_role_id: None,
            given_name: "Draco".into(),
            surname: "Malfoy".into(),
            employee_number: None,
            mobile_phone: None,
            department_name: "Hogwarts".into(),
            role_title: None,
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
        assert_eq!(rows[0].node_id, Some(8));

        // agacta eslesmeyen ad: satir yine listelenir (operator farki gorur)
        // ama `department_id` bos, `take` onu atlar
        p.ad_department = Some("Slytherin Evi".into());
        p.ad_department_id = None;
        assert_eq!(diffs(&p)[0].node_id, None);
    }

    /// ADR-120 madde 5: karsilastirilan deger rolun adi degil unvani
    /// (`roles.title`). Yer tutucu rolde unvanimiz `None` gelir ve satir
    /// cikmaz — onu gece taramasi onaysiz doldurur.
    #[test]
    fn the_role_row_compares_job_titles_and_skips_the_placeholder() {
        let mut p = pair();
        p.ad_title = Some("Student".into());

        // bizdeki rol yer tutucu (ya da unvani girilmemis): satir cikmaz
        assert!(diffs(&p).is_empty());

        // ayni unvan, farkli yazim: fark degil
        p.role_title = Some(" student ".into());
        assert!(diffs(&p).is_empty());

        // gercek fark, unvan agacta tek bir rolde: alinabilir
        p.role_title = Some("Professor".into());
        p.ad_role_id = Some(3);
        let rows = diffs(&p);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "role");
        assert_eq!(rows[0].label(), "field.role");
        assert_eq!(rows[0].key(), "1.role");
        assert_eq!(
            (rows[0].ad.as_str(), rows[0].ours.as_str()),
            ("Student", "Professor")
        );
        assert_eq!(rows[0].node_id, Some(3));

        // unvan hicbir role (ya da birden fazla role) karsilik gelmiyor: satir
        // listelenir, `take` atlar
        p.ad_role_id = None;
        assert_eq!(diffs(&p)[0].node_id, None);

        // AD tarafi bos: satir cikmaz
        p.ad_title = None;
        assert!(diffs(&p).is_empty());
    }

    #[test]
    fn only_both_filled_and_really_different_rows_are_listed() {
        // bos alan listede cikmaz: onu dolum halleder
        let mut p = pair();
        p.ad_employee_number = Some("00000000009".into());
        assert!(diffs(&p).is_empty());

        // ayni yazim fark degil; bastaki sifir farktir (AD'deki yazim alinabilsin)
        p.employee_number = Some("00000000009".into());
        assert!(diffs(&p).is_empty());
        p.employee_number = Some("9".into());
        assert_eq!(diffs(&p)[0].ours, "9");

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
    fn name_rows_compare_exactly_and_ignore_an_empty_directory_value() {
        let mut p = pair();
        assert!(diffs(&p).is_empty());
        p.ad_given_name = Some(" Draco ".into());
        assert!(diffs(&p).is_empty(), "boşluk fark değil");
        p.ad_given_name = Some("draco".into());
        p.ad_surname = Some("Malfoy-Black".into());
        let rows = diffs(&p);
        assert_eq!(
            rows.iter()
                .map(|r| (r.field, r.label()))
                .collect::<Vec<_>>(),
            [
                ("given_name", "field.given_name"),
                ("surname", "field.surname")
            ]
        );
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
