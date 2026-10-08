// --- START FEATURE: bulk-adoption ---
// Toplu sahiplenme (ADR-018, uygulamasi ADR-102): mutabakat ekranindaki
// "yonetilmeyen" hesaplar secilip tek seferde kimlige donusturulur.
//
// Hedefe hicbir sey yazilmaz: backend yalnizca kimlik satirini acar ve
// `existing_ad_account_hint` doldurur; hesabi kimlige **worker** baglar
// (`engine::adopt`, ADR-086) ve baglanti `observed` modunda kurulur — yani
// OpenSicil hesabi gozler, yonetmeye baslamaz. Yonetime alma ayri ve bilincli
// bir adim (ADR-087).
//
// Kisi alanlari AD'den gelir ama backend AD'ye baglanmaz (ADR-004): degerler
// son mutabakat taramasinin `reconcile_findings` satirlarinda duruyor.

use sqlx::PgPool;

use crate::identity::{self, IdentityForm};
use crate::national_id::{self, Keys};

/// Toplu sahiplenme yetkisi: kimlik acan islem, kayit yetkisiyle ayni kapi.
pub const AUTHORITIES: &[&str] = &["hr", "admin"];

/// Tek seferde sahiplenilecek en fazla hesap. Sinir keyfi degil: her kimlik
/// hedef basina bir is aciyor, buyuk bir secim kuyrugu tek hamlede doldurur.
/// Operator kalanlari ikinci bir secimle alir.
pub const MAX_BATCH: usize = 200;

/// Sahiplenilebilir bulgu: son taramada "yonetilmeyen" cikmis bir AD hesabi.
pub struct Candidate {
    /// `reconcile_findings.id`; form bunu geri yollar
    pub id: i64,
    pub account_name: String,
    pub display_name: String,
    pub given_name: String,
    pub surname: String,
    pub employee_number: String,
    /// AD'nin serbest metin `department` degeri
    pub department_name: String,
    /// AD'deki departman adi departman agacinda bulundu mu; bulunmadiysa
    /// formdaki departman kullanilir ve ekran bunu soyler
    pub department_id: Option<i64>,
    /// AD'nin `title` ozniteligi (ham deger, cogu zaman bos)
    pub title: String,
    /// `title`in agactaki tekil karsiligi olan birincil rol; yoksa (eslesme
    /// yok ya da unvan birden fazla role karsilik geliyor) partinin varsayilan
    /// rolu kullanilir (ADR-125)
    pub role_id: Option<i64>,
    /// AD'deki e-posta: kimlige **yazilmaz**, kullanici adi/e-posta uretimi
    /// worker'in isidir (ADR-015) ve sahiplenmede AD'den bos alanlari zaten o
    /// dolduruyor (ADR-086). Burada yalnizca "AD'de ne var" diye gorunur
    pub mail: String,
    /// Cep (`mobile`) ve sabit hat (`telephoneNumber`) ham degerleri
    pub mobile: String,
    pub telephone: String,
    /// Hesabin AD'de acilis gunu (`whenCreated`, `YYYY-MM-DD`); doluysa
    /// kimligin baslangic tarihi budur, bossa formdaki tarih (ADR-103 madde 6)
    pub when_created: String,
    /// Taramanin AD'den okudugu TC kimlik no (Yapilandirma'daki oznitelik,
    /// ADR-106 madde 5). Bulguda AEAD ile sifreli durur, burada cozulmus;
    /// ekrana yalnizca maskeli cikar, kimlige dogrulamadan gecerse yazilir
    pub national_id: String,
}

impl Candidate {
    /// TR kontrol hanelerinden gecer mi; gecmeyen deger kimlige yazilmaz, rozet cikar.
    pub fn national_id_ok(&self) -> bool {
        !self.national_id.is_empty() && national_id::parse("TR", &self.national_id).is_ok()
    }

    /// ADR-010: ekranda maskeli (`12*******34`)
    pub fn national_id_masked(&self) -> String {
        national_id::mask(&self.national_id)
    }

    /// AD'de duran telefon: once cep, cep bossa sabit hat. Ham deger.
    pub fn ad_phone(&self) -> &str {
        match self.mobile.is_empty() {
            true => &self.telephone,
            false => &self.mobile,
        }
    }

    /// Kimligin cep alanina yazilabilir mi? Kimlikteki alan E.164 cep
    /// numarasidir (docs/03); sabit hat biciminde bir deger ("01632 960001")
    /// oraya yazilmaz. Uymayan deger bos kalir, satirda rozet cikar ve
    /// operator duzeltir — `identity::validate` kurali gevsemez (ADR-106).
    pub fn phone_ok(&self) -> bool {
        let phone = self.ad_phone();
        !phone.is_empty() && identity::valid_e164(phone.to_string()).is_ok()
    }

    /// Ad ve soyad AD'de bos olabilir (docs/11 W8). O zaman `displayName`
    /// ikiye bolunur: ilk parca ad, kalani soyad.
    fn names(&self) -> (String, String) {
        if !self.given_name.is_empty() && !self.surname.is_empty() {
            return (self.given_name.clone(), self.surname.clone());
        }
        match self.display_name.trim().split_once(char::is_whitespace) {
            Some((first, rest)) => (first.to_string(), rest.trim().to_string()),
            None => (self.given_name.clone(), self.surname.clone()),
        }
    }
}

/// id, hesap adi, displayName, ad, soyad, sicil, departman adi, departman id,
/// mail, cep, sabit hat, acilis gunu, sifreli TC, unvan, unvanin rol id'si
type CandidateRow = (
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    String,
    String,
    String,
    String,
    Option<Vec<u8>>,
    String,
    Option<i64>,
);

/// Son taramadaki **henuz sahiplenilmemis** yonetilmeyen hesaplar. Sahiplenilen
/// hesap bir sonraki taramayi beklemeden listeden duser (ADR-126): ya kimlik
/// satiri onu isaretlemistir (`existing_ad_account_hint`) ya da worker
/// baglantiyi kurmustur (`account_links.external_id`, bulgudaki `objectGUID`).
/// Kapi ekranda degil sorguda: ikinci kez gonderilen bulgu ikinci kimlik acmaz.
///
/// AD'deki departman adi departman
/// agaciyla, unvani (`title`) birincil rollerin `title` koluyla (buyuk/kucuk
/// harf ve bosluk duyarsiz, tekil eslesme) eslenir — unvan iki role karsilik
/// geliyorsa (`HAVING count(*) = 1`) eslesme yok sayilir, yer tutucu rol hic
/// eslesmez (worker'in gece taramasindaki `fill_placeholder_roles` kuralinin
/// ayni, sahiplenme anina tasindi — ADR-125). Sifreli TC kimlik no burada
/// cozulur; cozulemeyen (anahtar donmus) deger bos sayilir.
pub async fn candidates(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    target: i64,
) -> Result<Vec<Candidate>, sqlx::Error> {
    let rows: Vec<CandidateRow> = sqlx::query_as(
        "SELECT f.id, f.account_name, COALESCE(f.display_name, ''), \
                COALESCE(f.given_name, ''), COALESCE(f.surname, ''), \
                COALESCE(f.employee_number, ''), COALESCE(f.department_name, ''), d.id, \
                COALESCE(f.mail, ''), COALESCE(f.mobile, ''), \
                COALESCE(f.telephone_number, ''), \
                COALESCE(to_char(f.when_created, 'YYYY-MM-DD'), ''), f.national_id_enc, \
                COALESCE(f.title, ''), \
                (SELECT max(r.id) FROM roles r \
                   WHERE r.kind = 'primary' AND NOT r.placeholder \
                     AND lower(btrim(r.title)) = lower(btrim(f.title)) \
                   HAVING count(*) = 1) \
         FROM reconcile_findings f \
         LEFT JOIN departments d ON lower(d.name) = lower(f.department_name) \
         WHERE f.target_system_id = $1 AND f.kind = 'unmanaged' \
           AND NOT EXISTS (SELECT 1 FROM account_links l \
             WHERE l.target_system_id = f.target_system_id \
               AND l.external_id = f.external_id) \
           AND NOT EXISTS (SELECT 1 FROM identities i \
             WHERE i.deleted_at IS NULL \
               AND lower(i.existing_ad_account_hint) = lower(f.account_name)) \
         ORDER BY f.account_name",
    )
    .bind(target)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            id: r.0,
            account_name: r.1,
            display_name: r.2,
            given_name: r.3,
            surname: r.4,
            employee_number: r.5,
            department_name: r.6,
            department_id: r.7,
            mail: r.8,
            mobile: r.9,
            telephone: r.10,
            when_created: r.11,
            national_id: decrypted_national_id(aead_key, r.12.as_deref()),
            title: r.13,
            role_id: r.14,
        })
        .collect())
}

/// Bulgudaki sifreli TC'yi cozer; cozulemeyen (anahtar donmus, bozuk) deger bos
/// sayilir ve yalnizca log'a dusulur — degerin kendisi degil, hata.
fn decrypted_national_id(aead_key: &[u8; crate::crypto::KEY_LEN], enc: Option<&[u8]>) -> String {
    match enc.map(|enc| national_id::decrypt(aead_key, enc)) {
        Some(Ok(value)) => value,
        Some(Err(e)) => {
            log_error!("bulk_adopt: bulgudaki kimlik numarası çözülemedi: {e}");
            String::new()
        }
        None => String::new(),
    }
}

/// Toplu secimin bir kimlik icin tasidigi ortak alanlar; AD'de karsiligi
/// olmayan (rol, calisma tipi, baslangic) ya da guvenilir olmayan degerler.
pub struct Batch {
    pub primary_role_id: i64,
    pub employment_type: String,
    pub start_date: String,
    /// AD'deki departman eslesmeyince kullanilacak departman. Bos birakilabilir:
    /// AD'nin kendi departmani agacta bulunuyorsa zaten o kullanilir, eslesmeyen
    /// hesap da uydurma departmanla acilmaktansa atlanir.
    pub fallback_department_id: Option<i64>,
}

#[derive(Default)]
pub struct Outcome {
    /// Acilan kimliklerin id'si
    pub created: Vec<i64>,
    /// Atlanan hesap ve nedeni (i18n anahtari)
    pub skipped: Vec<(String, &'static str)>,
}

pub enum AdoptError {
    Db(sqlx::Error),
    /// Form alani gecersiz; i18n anahtari
    Invalid(&'static str),
}

impl From<sqlx::Error> for AdoptError {
    fn from(e: sqlx::Error) -> AdoptError {
        AdoptError::Db(e)
    }
}

/// Secilen hesaplari kimlige cevirir. Her kimlik kendi transaction'inda acilir
/// (`identity::create`): bir hesabin eksik alani butun partiyi dusurmez, atlanan
/// hesap nedeniyle birlikte geri doner.
pub async fn adopt(
    pool: &PgPool,
    keys: &Keys<'_>,
    time_zone: &str,
    target: i64,
    selected: &[i64],
    batch: &Batch,
) -> Result<Outcome, AdoptError> {
    if selected.is_empty() {
        return Err(AdoptError::Invalid("err.bulk_adopt_empty"));
    }
    if selected.len() > MAX_BATCH {
        return Err(AdoptError::Invalid("err.bulk_adopt_too_many"));
    }
    let all = candidates(pool, keys.aead, target).await?;
    let mut outcome = Outcome::default();
    // ADR-126: aday sorgusu sahiplenilmis hesabi disarida biraktigi icin ikinci
    // kimlik acilmaz; hesap sessizce kaybolmasin diye atlananlara girer.
    for name in already_adopted(pool, target, selected, &all).await? {
        outcome.skipped.push((name, "err.already_adopted"));
    }
    for candidate in all.iter().filter(|c| selected.contains(&c.id)) {
        match create_one(pool, keys, time_zone, candidate, batch).await {
            Ok(id) => outcome.created.push(id),
            Err(reason) => outcome
                .skipped
                .push((candidate.account_name.clone(), reason)),
        }
    }
    Ok(outcome)
}

/// Secilen ama artik aday olmayan bulgularin hesap adi: sahiplenme olmus.
/// Listede hic olmayan id (eski tarama, uydurma deger) bos doner — 404 yerine
/// atlama, bugunku davranis (ADR-126).
async fn already_adopted(
    pool: &PgPool,
    target: i64,
    selected: &[i64],
    candidates: &[Candidate],
) -> Result<Vec<String>, sqlx::Error> {
    let gone: Vec<i64> = selected
        .iter()
        .copied()
        .filter(|id| !candidates.iter().any(|c| c.id == *id))
        .collect();
    if gone.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(
        "SELECT account_name FROM reconcile_findings \
         WHERE target_system_id = $1 AND id = ANY($2) ORDER BY account_name",
    )
    .bind(target)
    .bind(&gone)
    .fetch_all(pool)
    .await
}

/// Tek hesap: kimlik satirini kur, dogrula, ac. Hata i18n anahtari olarak doner
/// ve yalnizca o hesabi atlar.
async fn create_one(
    pool: &PgPool,
    keys: &Keys<'_>,
    time_zone: &str,
    candidate: &Candidate,
    batch: &Batch,
) -> Result<i64, &'static str> {
    let department = candidate
        .department_id
        .or(batch.fallback_department_id)
        .ok_or("err.department_required")?;
    // ADR-125: unvan agacta tekil bir role karsilik geliyorsa o rol, yoksa
    // partinin varsayilani (bugun yer tutucu `Tanimsiz`, ADR-103 madde 4).
    let role = candidate.role_id.unwrap_or(batch.primary_role_id);
    let form = form_for(candidate, batch, department, role);
    let new = identity::validate(&form)?;
    match identity::create(pool, keys, time_zone, &new, None).await {
        Ok((id, _)) => Ok(id),
        Err(identity::CreateError::DuplicateNationalId) => Err("err.duplicate_national_id"),
        Err(identity::CreateError::Db(e)) => {
            log_error!("web: toplu sahiplenme kimlik açamadı: {e}");
            Err("err.bulk_adopt_failed")
        }
    }
}

/// Kayit formunun toplu sahiplenmedeki karsiligi: AD'den gelen alanlar
/// (dogrulamadan gecenler) + partinin ortak alanlari.
fn form_for(candidate: &Candidate, batch: &Batch, department: i64, role: i64) -> IdentityForm {
    let (given_name, surname) = candidate.names();
    IdentityForm {
        given_name,
        surname,
        employee_number: candidate.employee_number.clone(),
        // Bicimi uymayan telefon bos gecer: uydurma numara yazmaktansa alan
        // bos kalir, ekrandaki rozet operatore soyler (ADR-106).
        mobile_phone: match candidate.phone_ok() {
            true => candidate.ad_phone().to_string(),
            false => String::new(),
        },
        // ADR-106 madde 5: dogrulamadan gecen TC kimlik no sifreli + blind
        // index'li yazilir (`identity::create`); gecmeyen bos kalir, blind index
        // cakismasi (ayni numara baska kimlikte) hesabi atlatir
        national_id_country: "TR".to_string(),
        national_id: match candidate.national_id_ok() {
            true => candidate.national_id.clone(),
            false => String::new(),
        },
        department_id: department.to_string(),
        primary_role_id: role.to_string(),
        employment_type: batch.employment_type.clone(),
        // Baslangic uydurulmaz: AD'deki acilis gunu varsa o, yoksa formdaki
        // tarih (ADR-103 madde 6)
        start_date: match candidate.when_created.is_empty() {
            true => batch.start_date.clone(),
            false => candidate.when_created.clone(),
        },
        // Sahiplenmenin kendisi: worker hesabi bu adla bulur ve baglar,
        // yeni hesap **acmaz** (ADR-086).
        existing_ad_account_hint: candidate.account_name.clone(),
        ..IdentityForm::default()
    }
}
// --- END FEATURE: bulk-adoption ---

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(given: &str, surname: &str, display: &str) -> Candidate {
        Candidate {
            id: 1,
            account_name: "harry.potter".to_string(),
            display_name: display.to_string(),
            given_name: given.to_string(),
            surname: surname.to_string(),
            employee_number: String::new(),
            department_name: String::new(),
            department_id: None,
            title: String::new(),
            role_id: None,
            mail: String::new(),
            mobile: String::new(),
            telephone: String::new(),
            when_created: String::new(),
            national_id: String::new(),
        }
    }

    /// ADR-106 madde 5: yalnizca TR kontrol hanelerinden gecen deger yazilir,
    /// ekranda hep maskeli.
    #[test]
    fn only_a_valid_tr_national_id_passes_and_the_screen_sees_a_mask() {
        let with = |value: &str| Candidate {
            national_id: value.to_string(),
            ..candidate("Harry", "Potter", "Harry Potter")
        };
        assert!(with("10000000146").national_id_ok());
        assert_eq!(with("10000000146").national_id_masked(), "10*******46");
        assert!(!with("10000000147").national_id_ok(), "kontrol hanesi");
        assert!(!with("123").national_id_ok());
        assert!(!with("").national_id_ok());
    }

    /// ADR-106: kimlikteki alan E.164 **cep**tir. Cep varsa cep, yoksa sabit
    /// hat gosterilir; bicimi uymayan deger yazilmaz (gercek Hogwarts AD'sinde
    /// 29 hesabin hepsinde sabit hat var, hicbirinde cep yok).
    #[test]
    fn the_mobile_wins_and_a_non_e164_number_is_not_written() {
        let phone = |mobile: &str, telephone: &str| Candidate {
            mobile: mobile.to_string(),
            telephone: telephone.to_string(),
            ..candidate("Harry", "Potter", "Harry Potter")
        };

        // Cep doluysa cep kazanir
        let both = phone("+905321234567", "01632 960001");
        assert_eq!(both.ad_phone(), "+905321234567");
        assert!(both.phone_ok());

        // Cep bossa sabit hat gosterilir ama bicimi uymadigi icin yazilmaz
        let landline = phone("", "01632 960001");
        assert_eq!(landline.ad_phone(), "01632 960001");
        assert!(!landline.phone_ok(), "sabit hat biçimi E.164 değil");

        // E.164 sabit hat kabul edilir: kural bicime bakar, ozniteligin adina degil
        assert!(phone("", "+442079460001").phone_ok());

        // Ikisi de bossa rozet de cikmaz
        let empty = phone("", "");
        assert_eq!(empty.ad_phone(), "");
        assert!(!empty.phone_ok());
    }

    #[test]
    fn names_prefer_ad_attributes_and_fall_back_to_display_name() {
        // Ikisi de doluysa AD'nin kendi alanlari kullanilir
        let both = candidate("Harry", "Potter", "Yanlış Ad");
        assert_eq!(both.names(), ("Harry".into(), "Potter".into()));

        // givenName/sn bos: displayName ilk bosluktan bolunur (docs/11 W8)
        let only_display = candidate("", "", "Harry James Potter");
        assert_eq!(
            only_display.names(),
            ("Harry".into(), "James Potter".into())
        );

        // Tek parcali displayName: soyad bos kalir ve `validate` bunu reddeder,
        // hesap atlanir — uydurma soyad yazmaktansa operator duzeltir
        let single = candidate("", "", "Hagrid");
        assert_eq!(single.names(), (String::new(), String::new()));
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn adopting_creates_identities_with_the_hint_and_skips_unusable_rows() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let department: i64 = sqlx::query_scalar("SELECT id FROM departments LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let role: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE kind = 'primary' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
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
        let keys = Keys {
            aead: &[7u8; crate::crypto::KEY_LEN],
            blind_index: &[9u8; crate::crypto::KEY_LEN],
        };
        // Taramanin yazdigi bicim: AEAD + surum bayti (worker `encrypt_versioned`)
        let encrypted = |value: &str| crate::crypto::encrypt_versioned(keys.aead, value.as_bytes());
        // Dort bulgu: AD alanlari dolu, yalnizca displayName'i olan, soyadsiz, ve
        // Harry'nin TC'sini tasiyan mukerrer. Telefon ikisinde farkli: Harry'de
        // Hogwarts'in sabit hatti (E.164 degil), Ron'da gercek bir cep — ADR-106.
        // Acilis gunu yalnizca Harry'de: baslangic ondan, Ron'da formdan (ADR-103
        // madde 6). TC: Harry'de gecerli, Ron'da bozuk, zz.dup'ta Harry'ninki.
        for (guid, sam, display, given, sn, dept, mail, mobile, phone, created, tc) in [
            (
                "g1",
                "harry.potter",
                "Harry Potter",
                "Harry",
                "Potter",
                "Test Birimi",
                "harry.potter@hogwarts.local",
                "",
                "01632 960001",
                Some("2024-03-05"),
                Some("10000000146"),
            ),
            (
                "g2",
                "ron.weasley",
                "Ron Billius Weasley",
                "",
                "",
                "Bilinmeyen",
                "",
                "+905321234567",
                "",
                None,
                Some("123"),
            ),
            ("g3", "hagrid", "Hagrid", "", "", "", "", "", "", None, None),
            (
                "g4",
                "zz.dup",
                "Zz Dup",
                "Zz",
                "Dup",
                "Test Birimi",
                "",
                "",
                "",
                None,
                Some("10000000146"),
            ),
        ] {
            sqlx::query(
                "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
                 external_id, account_name, display_name, container, enabled, \
                 given_name, surname, department_name, mail, mobile, telephone_number, \
                 when_created, national_id_enc) \
                 VALUES ($1, $2, 'unmanaged', $3, $4, $5, 'OU=Users', true, $6, $7, $8, \
                 $9, $10, $11, $12::date, $13)",
            )
            .bind(target)
            .bind(read_job)
            .bind(guid)
            .bind(sam)
            .bind(display)
            .bind(given)
            .bind(sn)
            .bind(dept)
            .bind(mail)
            .bind(mobile)
            .bind(phone)
            .bind(created)
            .bind(tc.map(encrypted))
            .execute(&pool)
            .await
            .unwrap();
        }

        let found = candidates(&pool, keys.aead, target).await.unwrap();
        assert_eq!(found.len(), 4);
        // AD'deki "Test Birimi" departman agacinda var, "Bilinmeyen" yok
        let harry = found
            .iter()
            .find(|c| c.account_name == "harry.potter")
            .unwrap();
        assert_eq!(harry.department_id, Some(department));
        let ron = found
            .iter()
            .find(|c| c.account_name == "ron.weasley")
            .unwrap();
        assert_eq!(
            ron.department_id, None,
            "eşleşmeyen departman formdan gelir"
        );
        // ADR-106 madde 5: sifreli deger cozuldu, ekrana maskeli; bozuk olan gecmez
        assert_eq!(harry.national_id, "10000000146");
        assert!(harry.national_id_ok());
        assert_eq!(ron.national_id, "123");
        assert!(!ron.national_id_ok());

        // Departman bos birakilabilir: eslesmeyen hesap uydurma departmanla
        // acilmaktansa atlanir, eslesenler icin alan hic gerekmez
        let no_fallback = Batch {
            primary_role_id: role,
            employment_type: "permanent".to_string(),
            start_date: "2026-10-01".to_string(),
            fallback_department_id: None,
        };
        let unmatched = adopt(
            &pool,
            &keys,
            "Europe/Istanbul",
            target,
            &[ron.id],
            &no_fallback,
        )
        .await
        .unwrap_or_else(|_| panic!("toplu sahiplenme başarısız"));
        assert!(unmatched.created.is_empty());
        assert_eq!(
            unmatched.skipped,
            vec![("ron.weasley".to_string(), "err.department_required")]
        );

        let batch = Batch {
            primary_role_id: role,
            employment_type: "permanent".to_string(),
            start_date: "2026-10-01".to_string(),
            fallback_department_id: Some(department),
        };
        let selected: Vec<i64> = found.iter().map(|c| c.id).collect();
        let outcome = adopt(&pool, &keys, "Europe/Istanbul", target, &selected, &batch)
            .await
            .unwrap_or_else(|_| panic!("toplu sahiplenme başarısız"));

        // Soyadi cozulemeyen hesap ve Harry'nin TC'sini tasiyan mukerrer atlandi
        // (blind index cakismasi, ADR-010), oburu ikisi acildi
        assert_eq!(outcome.created.len(), 2);
        assert_eq!(
            outcome.skipped,
            vec![
                ("hagrid".to_string(), "err.given_name_blank"),
                ("zz.dup".to_string(), "err.duplicate_national_id"),
            ]
        );
        // Gecerli TC sifreli + blind index'li yazildi, bozuk olan bos kaldi
        let national_ids: Vec<(String, bool)> = sqlx::query_as(
            "SELECT existing_ad_account_hint, national_id_bidx IS NOT NULL FROM identities \
             WHERE existing_ad_account_hint IS NOT NULL ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            national_ids,
            vec![("harry.potter".into(), true), ("ron.weasley".into(), false)]
        );

        // Ipucu yazildi: hesabi worker baglayacak, backend AD'ye dokunmadi
        let hints: Vec<(String, String, String, Option<i64>)> = sqlx::query_as(
            "SELECT given_name, surname, existing_ad_account_hint, department_id \
             FROM identities WHERE existing_ad_account_hint IS NOT NULL ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].0, "Harry");
        assert_eq!(hints[0].1, "Potter");
        assert_eq!(hints[0].2, "harry.potter");
        assert_eq!(hints[0].3, Some(department));
        // displayName'den bolunen ad
        assert_eq!(hints[1].0, "Ron");
        assert_eq!(hints[1].1, "Billius Weasley");

        // ADR-103 madde 6: baslangic AD'deki acilis gununden, yoksa formdan
        let starts: Vec<(String, String)> = sqlx::query_as(
            "SELECT existing_ad_account_hint, to_char(start_date, 'YYYY-MM-DD') \
             FROM identities WHERE existing_ad_account_hint IS NOT NULL ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(starts[0], ("harry.potter".into(), "2024-03-05".into()));
        assert_eq!(starts[1], ("ron.weasley".into(), "2026-10-01".into()));

        // ADR-106: AD'nin telefonu kimlige yazildi ama yalnizca E.164 olani.
        // Harry'nin sabit hatti bos gecti (uydurma numara yok), Ron'un cebi
        // yazildi. E-posta kimlige yazilmaz: adi ve e-postayi worker uretir
        // (ADR-015), bulgudaki `mail` ekranda ipucu olarak durur
        let phones: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT existing_ad_account_hint, mobile_phone FROM identities \
             WHERE existing_ad_account_hint IS NOT NULL ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(phones[0].0, "harry.potter");
        assert_eq!(
            phones[0].1, None,
            "sabit hat E.164 değil, kimliğe yazılmadı"
        );
        assert_eq!(phones[1].1.as_deref(), Some("+905321234567"));
        let mails: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM identities WHERE existing_ad_account_hint IS NOT NULL \
             AND email IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(mails, 0, "e-posta backend'in işi değil (ADR-015)");

        // Her yeni kimlik icin her hedefe is acildi (seed'deki iki kimlik haric)
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE identity_id = ANY($1)")
            .bind(&outcome.created)
            .fetch_one(&pool)
            .await
            .unwrap();
        let targets: i64 = sqlx::query_scalar("SELECT count(*) FROM target_systems")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 2 * targets);
        assert_eq!(ids.len(), 2, "seed kimlikleri duruyor");

        // ADR-126: sahiplenilen hesap sonraki taramayi beklemeden listeden
        // duser — kimlik satiri onu `existing_ad_account_hint` ile isaretledi.
        let names = |cs: &[Candidate]| -> Vec<String> {
            cs.iter().map(|c| c.account_name.clone()).collect()
        };
        let left = candidates(&pool, keys.aead, target).await.unwrap();
        assert_eq!(names(&left), ["hagrid", "zz.dup"], "sahiplenilen iki hesap");

        // Ayni secim ikinci kez gelse (cift tik, geri tusu) ikinci kimlik
        // acilmaz; hesap "zaten sahiplenildi" nedeniyle atlananlara girer
        let again = adopt(&pool, &keys, "Europe/Istanbul", target, &selected, &batch)
            .await
            .unwrap_or_else(|_| panic!("toplu sahiplenme başarısız"));
        assert!(again.created.is_empty(), "ikinci kimlik açılmaz");
        assert_eq!(
            again.skipped,
            vec![
                ("harry.potter".to_string(), "err.already_adopted"),
                ("ron.weasley".to_string(), "err.already_adopted"),
                ("hagrid".to_string(), "err.given_name_blank"),
                ("zz.dup".to_string(), "err.duplicate_national_id"),
            ]
        );
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM identities WHERE existing_ad_account_hint IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(total, 2, "mükerrer kayıt açılmadı");

        // Kapinin ikinci yarisi: worker baglantiyi kurduysa (ipucu yok, bulgunun
        // `objectGUID`i bagli) bulgu yine aday degildir
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             VALUES ($1, $2, 'g3', 'adopted', 'observed')",
        )
        .bind(ids[0])
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        let linked = candidates(&pool, keys.aead, target).await.unwrap();
        assert_eq!(names(&linked), ["zz.dup"], "bağlanan hesap da aday değil");

        // Bos secim ve sinir asimi forma hata doner, kimlik acilmaz
        assert!(matches!(
            adopt(&pool, &keys, "Europe/Istanbul", target, &[], &batch).await,
            Err(AdoptError::Invalid("err.bulk_adopt_empty"))
        ));
        let too_many: Vec<i64> = (0..=MAX_BATCH as i64).collect();
        assert!(matches!(
            adopt(&pool, &keys, "Europe/Istanbul", target, &too_many, &batch).await,
            Err(AdoptError::Invalid("err.bulk_adopt_too_many"))
        ));

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-125: unvan agacta tekil bir role karsilik geliyorsa aday dogrudan o
    // role acilir; eslesmeyen ve belirsiz (iki role karsilik gelen) unvan
    // partinin varsayilanina duser — worker'in gece taramasindaki
    // `fill_placeholder_roles` kuralinin ayni, sahiplenme anina tasindi.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_role_resolves_from_the_ad_title_and_falls_back_to_the_batch_default() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let department: i64 =
            sqlx::query_scalar("INSERT INTO departments (name) VALUES ('Birim') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let placeholder: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE placeholder")
            .fetch_one(&pool)
            .await
            .unwrap();
        let matched_role: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, title) \
             VALUES ('primary', 'Sistem Uzmanı', 'Sistem Uzmanı') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // Ayni unvani tasiyan iki rol: eslesme belirsizlesir, HAVING count(*) = 1 eler.
        sqlx::query(
            "INSERT INTO roles (kind, name, title) \
             VALUES ('primary', 'Kıdemli Uzman', 'Belirsiz Unvan'), \
                    ('primary', 'İkinci Uzman', 'Belirsiz Unvan')",
        )
        .execute(&pool)
        .await
        .unwrap();
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

        for (guid, sam, title) in [
            // Bosluk ve buyuk/kucuk harf farki yok sayilir (worker kuraliyla ayni)
            ("g1", "eslesen", Some("  sistem uzmanı ")),
            ("g2", "eslesmeyen", Some("Olmayan Unvan")),
            ("g3", "belirsiz", Some("Belirsiz Unvan")),
            ("g4", "unvansiz", None),
        ] {
            sqlx::query(
                "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
                 external_id, account_name, display_name, container, enabled, \
                 given_name, surname, department_name, title) \
                 VALUES ($1, $2, 'unmanaged', $3, $4, $4, 'OU=Users', true, 'Ad', 'Soyad', \
                 'Birim', $5)",
            )
            .bind(target)
            .bind(read_job)
            .bind(guid)
            .bind(sam)
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
        }

        let keys = Keys {
            aead: &[3u8; crate::crypto::KEY_LEN],
            blind_index: &[4u8; crate::crypto::KEY_LEN],
        };
        let found = candidates(&pool, keys.aead, target).await.unwrap();
        let role_of = |sam: &str| {
            found
                .iter()
                .find(|c| c.account_name == sam)
                .unwrap_or_else(|| panic!("{sam} bulunamadı"))
                .role_id
        };
        assert_eq!(
            role_of("eslesen"),
            Some(matched_role),
            "boşluk ve büyük/küçük harf yok sayılır"
        );
        assert_eq!(role_of("eslesmeyen"), None);
        assert_eq!(
            role_of("belirsiz"),
            None,
            "unvan iki role karşılık geliyorsa eşleşme yok sayılır"
        );
        assert_eq!(role_of("unvansiz"), None);

        // Sahiplenince: eslesen kisi dogrudan role acilir, gerisi parti
        // varsayilanina (burada yer tutucu) duser.
        let batch = Batch {
            primary_role_id: placeholder,
            employment_type: "permanent".to_string(),
            start_date: "2026-10-01".to_string(),
            fallback_department_id: Some(department),
        };
        let selected: Vec<i64> = found.iter().map(|c| c.id).collect();
        let outcome = adopt(&pool, &keys, "Europe/Istanbul", target, &selected, &batch)
            .await
            .unwrap_or_else(|_| panic!("toplu sahiplenme başarısız"));
        assert_eq!(outcome.created.len(), 4);
        let roles: Vec<(String, i64)> = sqlx::query_as(
            "SELECT existing_ad_account_hint, primary_role_id FROM identities \
             WHERE existing_ad_account_hint IS NOT NULL ORDER BY existing_ad_account_hint",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            roles,
            vec![
                ("belirsiz".to_string(), placeholder),
                ("eslesen".to_string(), matched_role),
                ("eslesmeyen".to_string(), placeholder),
                ("unvansiz".to_string(), placeholder),
            ]
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
