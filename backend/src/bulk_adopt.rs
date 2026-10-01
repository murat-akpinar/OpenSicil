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
use crate::national_id::Keys;

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
}

impl Candidate {
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

/// Son taramadaki yonetilmeyen hesaplar; AD'deki departman adi departman
/// agaciyla adina gore (buyuk/kucuk harf duyarsiz) eslenir.
pub async fn candidates(pool: &PgPool, target: i64) -> Result<Vec<Candidate>, sqlx::Error> {
    type Row = (
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        Option<i64>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT f.id, f.account_name, COALESCE(f.display_name, ''), \
                COALESCE(f.given_name, ''), COALESCE(f.surname, ''), \
                COALESCE(f.employee_number, ''), COALESCE(f.department_name, ''), d.id \
         FROM reconcile_findings f \
         LEFT JOIN departments d ON lower(d.name) = lower(f.department_name) \
         WHERE f.target_system_id = $1 AND f.kind = 'unmanaged' \
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
        })
        .collect())
}

/// Toplu secimin bir kimlik icin tasidigi ortak alanlar; AD'de karsiligi
/// olmayan (rol, calisma tipi, baslangic) ya da guvenilir olmayan degerler.
pub struct Batch {
    pub primary_role_id: i64,
    pub employment_type: String,
    pub start_date: String,
    /// AD'deki departman eslesmeyince kullanilacak departman
    pub fallback_department_id: i64,
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
    let all = candidates(pool, target).await?;
    let mut outcome = Outcome::default();
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

/// Tek hesap: kimlik satirini kur, dogrula, ac. Hata i18n anahtari olarak doner
/// ve yalnizca o hesabi atlar.
async fn create_one(
    pool: &PgPool,
    keys: &Keys<'_>,
    time_zone: &str,
    candidate: &Candidate,
    batch: &Batch,
) -> Result<i64, &'static str> {
    let (given_name, surname) = candidate.names();
    let department = candidate
        .department_id
        .unwrap_or(batch.fallback_department_id);
    let form = IdentityForm {
        given_name,
        surname,
        employee_number: candidate.employee_number.clone(),
        department_id: department.to_string(),
        primary_role_id: batch.primary_role_id.to_string(),
        employment_type: batch.employment_type.clone(),
        start_date: batch.start_date.clone(),
        // Sahiplenmenin kendisi: worker hesabi bu adla bulur ve baglar,
        // yeni hesap **acmaz** (ADR-086).
        existing_ad_account_hint: candidate.account_name.clone(),
        ..IdentityForm::default()
    };
    let new = identity::validate(&form)?;
    match identity::create(pool, keys, time_zone, &new, None).await {
        Ok((id, _)) => Ok(id),
        Err(identity::CreateError::DuplicateNationalId) => Err("err.duplicate_national_id"),
        Err(identity::CreateError::Db(e)) => {
            eprintln!("web: toplu sahiplenme kimlik açamadı: {e}");
            Err("err.bulk_adopt_failed")
        }
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
        }
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
        // Uc bulgu: AD alanlari dolu, yalnizca displayName'i olan, ve soyadsiz
        for (guid, sam, display, given, sn, dept) in [
            (
                "g1",
                "harry.potter",
                "Harry Potter",
                "Harry",
                "Potter",
                "Test Birimi",
            ),
            (
                "g2",
                "ron.weasley",
                "Ron Billius Weasley",
                "",
                "",
                "Bilinmeyen",
            ),
            ("g3", "hagrid", "Hagrid", "", "", ""),
        ] {
            sqlx::query(
                "INSERT INTO reconcile_findings (target_system_id, read_job_id, kind, \
                 external_id, account_name, display_name, container, enabled, \
                 given_name, surname, department_name) \
                 VALUES ($1, $2, 'unmanaged', $3, $4, $5, 'OU=Users', true, $6, $7, $8)",
            )
            .bind(target)
            .bind(read_job)
            .bind(guid)
            .bind(sam)
            .bind(display)
            .bind(given)
            .bind(sn)
            .bind(dept)
            .execute(&pool)
            .await
            .unwrap();
        }

        let found = candidates(&pool, target).await.unwrap();
        assert_eq!(found.len(), 3);
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

        let keys = Keys {
            aead: &[7u8; crate::crypto::KEY_LEN],
            blind_index: &[9u8; crate::crypto::KEY_LEN],
        };
        let batch = Batch {
            primary_role_id: role,
            employment_type: "permanent".to_string(),
            start_date: "2026-10-01".to_string(),
            fallback_department_id: department,
        };
        let selected: Vec<i64> = found.iter().map(|c| c.id).collect();
        let outcome = adopt(&pool, &keys, "Europe/Istanbul", target, &selected, &batch)
            .await
            .unwrap_or_else(|_| panic!("toplu sahiplenme başarısız"));

        // Soyadi cozulemeyen hesap atlandi, oburu ikisi acildi
        assert_eq!(outcome.created.len(), 2);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].0, "hagrid");

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
}
