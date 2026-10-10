// --- START FEATURE: identity-registration ---
// Kimlik kayit formu ve kisi sayfasi verisi (docs/03 kimlik semasi, F-12, ADR-078).
// Dogrulama saf; kayit tek transaction; durum desired_state ile turetilir,
// saat dilimi cevirisi Postgres'te (operator reddiyle ayni yol).

use serde::Deserialize;
use sqlx::PgPool;

use crate::desired_state::{lifecycle_state, Date, LifecycleState};
use crate::i18n::Lang;
use crate::national_id::{self, Keys, NationalId};
use crate::operator_guard::{timeline_from_row, TimelineRow};
use crate::timeline_sql;

// Ekran karsiligi i18n'de: employment.<anahtar> (ADR-089)
pub const EMPLOYMENT_TYPES: [&str; 4] = ["permanent", "contract", "intern", "outsourced"];
const PERMANENT: &str = "permanent";
const E164_MIN_DIGITS: usize = 8;
const E164_MAX_DIGITS: usize = 15;
const INTERVENTION_PREFIX: &str = "müdahale gerekiyor: ";
pub(crate) const INTERVENTION_STATUS: &str = "needs_intervention";
const RECENT_LIMIT: i64 = 50;
/// Ana sayfadaki "son kimlikler" kutusu: panelin bir karti, liste degil.
/// Tam liste `/identities`te ve sayfali — panel 50 satirla uzayip gidiyordu.
const HOME_RECENT_LIMIT: i64 = 8;
const EVENT_LIMIT: i64 = 20;

#[derive(Deserialize, Default, Clone)]
pub struct IdentityForm {
    #[serde(default)]
    pub given_name: String,
    #[serde(default)]
    pub surname: String,
    #[serde(default)]
    pub national_id_country: String,
    #[serde(default)]
    pub national_id: String,
    #[serde(default)]
    pub employee_number: String,
    #[serde(default)]
    pub mobile_phone: String,
    #[serde(default)]
    pub department_id: String,
    #[serde(default)]
    pub primary_role_id: String,
    #[serde(default)]
    pub manager_id: String,
    #[serde(default)]
    pub employment_type: String,
    #[serde(default)]
    pub start_date: String,
    /// ADR-056: "Kaydet ve ilk parolayı ver" düğmesi; bos degilse istenmistir
    #[serde(default)]
    pub issue_first_password: String,
    /// ADR-018: sahiplenilecek AD kullanici adi; yalnizca sahiplenme acikken gosterilir
    #[serde(default)]
    pub existing_ad_account_hint: String,
    #[serde(default)]
    pub end_date: String,
    // ADR-022: istege bagli elle kullanici adi; bossa sablon
    #[serde(default)]
    pub requested_username: String,
    // "Yine de kaydet" kutusu (mukerrer kisi uyarisi, docs/03)
    #[serde(default)]
    pub confirm_duplicate: Option<String>,
}

#[derive(Debug)]
pub struct NewIdentity {
    pub given_name: String,
    pub surname: String,
    pub national_id: Option<NationalId>,
    pub employee_number: Option<String>,
    pub mobile_phone: Option<String>,
    pub department_id: i64,
    pub primary_role_id: i64,
    pub manager_id: Option<i64>,
    pub employment_type: String,
    pub start_date: String,
    pub end_date: Option<String>,
    pub requested_username: Option<String>,
    /// ADR-018/086: sahiplenilecek AD hesabi (sAMAccountName); doluysa hesap acilmaz
    pub existing_ad_account_hint: Option<String>,
}

// ADR-086: sAMAccountName bicimi; LDAP kacisi worker'da, burada yalnizca sekil.
pub fn valid_account_hint(raw: &str) -> Result<Option<String>, &'static str> {
    const MAX_LEN: usize = 20;
    let hint = raw.trim();
    if hint.is_empty() {
        return Ok(None);
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    if hint.len() > MAX_LEN || !hint.chars().all(allowed) {
        return Err("err.account_hint_shape");
    }
    Ok(Some(hint.to_string()))
}

// Sekil kontrolu burada; ADR-011 normallestirmesi worker'da (validate_manual).
pub fn valid_requested_username(raw: &str) -> Result<Option<String>, &'static str> {
    const MAX_LEN: usize = 20;
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Ok(None);
    }
    let shape_ok = name.chars().count() <= MAX_LEN
        && name.chars().all(|c| c.is_alphanumeric() || c == '.')
        && !name.starts_with('.')
        && !name.ends_with('.');
    // err.username_shape metni MAX_LEN degerini icerir
    shape_ok.then_some(Some(name)).ok_or("err.username_shape")
}

// Doner: i18n anahtari (ADR-089); ceviri web katmaninda.
pub fn validate(f: &IdentityForm) -> Result<NewIdentity, &'static str> {
    let given_name = required(&f.given_name, "err.given_name_blank")?;
    let surname = required(&f.surname, "err.surname_blank")?;
    let national_id = optional(&f.national_id)
        .map(|raw| national_id::parse(&f.national_id_country, &raw))
        .transpose()?;
    let mobile_phone = optional(&f.mobile_phone).map(valid_e164).transpose()?;
    let department_id = parse_id(&f.department_id, "err.department_required")?;
    let primary_role_id = parse_id(&f.primary_role_id, "err.primary_role_required")?;
    let manager_id = optional(&f.manager_id)
        .map(|m| parse_id(&m, "err.manager_invalid"))
        .transpose()?;
    if !EMPLOYMENT_TYPES.contains(&f.employment_type.as_str()) {
        return Err("err.employment_type_required");
    }
    let start = valid_date(&f.start_date, "err.start_date_format")?;
    let end = optional(&f.end_date)
        .map(|d| valid_date(&d, "err.end_date_format"))
        .transpose()?;
    if f.employment_type != PERMANENT && end.is_none() {
        return Err("err.end_required_non_permanent");
    }
    if end.is_some_and(|e| e < start) {
        return Err("err.end_before_start");
    }
    Ok(NewIdentity {
        given_name,
        surname,
        national_id,
        employee_number: optional(&f.employee_number),
        mobile_phone,
        department_id,
        primary_role_id,
        manager_id,
        employment_type: f.employment_type.clone(),
        start_date: f.start_date.trim().to_string(),
        end_date: optional(&f.end_date),
        requested_username: valid_requested_username(&f.requested_username)?,
        existing_ad_account_hint: valid_account_hint(&f.existing_ad_account_hint)?,
    })
}

fn required(value: &str, key: &'static str) -> Result<String, &'static str> {
    optional(value).ok_or(key)
}

fn optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_id(value: &str, key: &'static str) -> Result<i64, &'static str> {
    value.trim().parse().map_err(|_| key)
}

fn valid_date(value: &str, key: &'static str) -> Result<Date, &'static str> {
    Date::from_iso(value.trim()).ok_or(key)
}

// E.164: '+' ve 8–15 rakam, ilk rakam 0 degil (docs/03 cep telefonu).
pub(crate) fn valid_e164(value: String) -> Result<String, &'static str> {
    let digits = value.strip_prefix('+').unwrap_or("");
    let ok = (E164_MIN_DIGITS..=E164_MAX_DIGITS).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.starts_with('0');
    ok.then_some(value).ok_or("err.mobile_format")
}

#[derive(Debug)]
pub enum CreateError {
    DuplicateNationalId,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for CreateError {
    fn from(e: sqlx::Error) -> Self {
        CreateError::Db(e)
    }
}

// Bitis gunu → bitis ani: ertesi gun 00:00, kurulum saat diliminde (ADR-038/039).
// Kimlik, her hedefe tek kimlik isi ve (istenmisse) ilk parola istegi tek transaction'da
// (ADR-056); kayit gorunuyorsa isi de vardir. Doner: (kimlik, ilk parola istegi).
pub async fn create(
    pool: &PgPool,
    keys: &Keys<'_>,
    time_zone: &str,
    new: &NewIdentity,
    first_password_by: Option<&str>,
) -> Result<(i64, Option<i64>), CreateError> {
    reject_known_national_id(pool, keys, new).await?;
    let mut tx = pool.begin().await?;
    let id = insert_identity(&mut tx, time_zone, new).await?;
    if let Some(nid) = &new.national_id {
        national_id::store(&mut *tx, keys, id, nid)
            .await
            .map_err(|e| match e.as_database_error() {
                Some(db) if db.is_unique_violation() => CreateError::DuplicateNationalId,
                _ => CreateError::Db(e),
            })?;
    }
    let first_password = open_jobs(&mut tx, id, first_password_by).await?;
    tx.commit().await?;
    Ok((id, first_password))
}

/// Kimlik satirinin kendisi; kayit formu ve CSV ice aktarma (tek transaction'da
/// cok satir) ayni INSERT'i kullanir.
pub(crate) async fn insert_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    time_zone: &str,
    new: &NewIdentity,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO identities (given_name, surname, employee_number, mobile_phone, \
         department_id, primary_role_id, manager_id, employment_type, start_date, end_at, \
         requested_username, existing_ad_account_hint) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::date, \
         (($10::date + 1)::timestamp AT TIME ZONE $11), $12, $13) RETURNING id",
    )
    .bind(&new.given_name)
    .bind(&new.surname)
    .bind(&new.employee_number)
    .bind(&new.mobile_phone)
    .bind(new.department_id)
    .bind(new.primary_role_id)
    .bind(new.manager_id)
    .bind(&new.employment_type)
    .bind(&new.start_date)
    .bind(&new.end_date)
    .bind(time_zone)
    .bind(&new.requested_username)
    .bind(&new.existing_ad_account_hint)
    .fetch_one(&mut **tx)
    .await
}

// On kontrol operatore erken ve net cevap verir; yaris durumunda UNIQUE indeks yakalar.
async fn reject_known_national_id(
    pool: &PgPool,
    keys: &Keys<'_>,
    new: &NewIdentity,
) -> Result<(), CreateError> {
    let Some(nid) = &new.national_id else {
        return Ok(());
    };
    match national_id::find_identity(pool, keys.blind_index, nid).await? {
        Some(_) => Err(CreateError::DuplicateNationalId),
        None => Ok(()),
    }
}

// Her hedefe tek kimlik isi; istenmisse AD hedefine ilk parola istegi (ADR-056).
async fn open_jobs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
    first_password_by: Option<&str>,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query(
        "INSERT INTO jobs (identity_id, target_system_id, priority) \
         SELECT $1, id, $2 FROM target_systems ORDER BY id",
    )
    .bind(id)
    .bind(crate::jobs::Priority::Single as i16)
    .execute(&mut **tx)
    .await?;
    let Some(operator) = first_password_by else {
        return Ok(None);
    };
    sqlx::query_scalar(
        "INSERT INTO first_passwords (identity_id, target_system_id, requested_by) \
         SELECT $1, id, $2 FROM target_systems WHERE kind = 'ad' RETURNING id",
    )
    .bind(id)
    .bind(operator)
    .fetch_optional(&mut **tx)
    .await
}

// ADR-056: ilk parola yalnizca baslangici bugun ya da gecmiste olan kayda, kurum saatiyle.
pub async fn starts_by_today(
    pool: &PgPool,
    start_date: &str,
    time_zone: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT $1::date <= (now() AT TIME ZONE $2)::date")
        .bind(start_date)
        .bind(time_zone)
        .fetch_one(pool)
        .await
}

/// Kurulum saat diliminde bugun, `YYYY-MM-DD` (ADR-039). Tarih girdisinin
/// varsayilani; sunucunun yerel saatiyle degil kurumun saatiyle doldurulur.
pub async fn today(pool: &PgPool, time_zone: &str) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT to_char((now() AT TIME ZONE $1)::date, 'YYYY-MM-DD')")
        .bind(time_zone)
        .fetch_one(pool)
        .await
}

// Kimlik no yokken ad-soyad esleşmesi uyaridir, engel degil (docs/03).
// Katlama DB yerel ayarindan bagimsiz: Turkce buyuk harfler (I/İ dahil) once
// translate ile kucuge indirilir, lower() kalan ASCII'yi halleder.
pub async fn similar_name_exists(
    pool: &PgPool,
    given_name: &str,
    surname: &str,
) -> Result<bool, sqlx::Error> {
    Ok(similar_person(pool, given_name, surname).await?.is_some())
}

/// Ad-soyadi (Turkce harf duyarsiz) ayni olan silinmemis ilk kimlik:
/// (id, "Ad Soyad", sicil no). CSV onizlemesi "olasi mukerrer" listesi icin (ADR-042).
// Ad karsilastirmasinin tek kaynagi: Turkce buyuk harfler katlanir, sonra kucuk harf.
macro_rules! folded {
    ($column:literal) => {
        concat!("lower(translate(", $column, ", 'IİıŞĞÜÖÇ', 'iiişğüöç'))")
    };
}

pub async fn similar_person(
    pool: &PgPool,
    given_name: &str,
    surname: &str,
) -> Result<Option<(i64, String, String)>, sqlx::Error> {
    let found = similar_people(pool, &[(given_name.to_string(), surname.to_string())]).await?;
    Ok(found.into_iter().next().flatten())
}

/// Toplu hali (CSV onizlemesi): her ad-soyad icin en kucuk id'li silinmemis eslesme,
/// girdiyle ayni sirada. Tek sorgu — satir basina sorgu 20.000 kimlikte 1.000 satiri
/// 16 sn'de tariyordu (2026-10-08 olcumu).
pub async fn similar_people(
    pool: &PgPool,
    names: &[(String, String)],
) -> Result<Vec<Option<(i64, String, String)>>, sqlx::Error> {
    let (given, surname): (Vec<&str>, Vec<&str>) =
        names.iter().map(|(g, s)| (g.as_str(), s.as_str())).unzip();
    let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(concat!(
        "SELECT DISTINCT ON (n.idx) n.idx, i.id, i.given_name || ' ' || i.surname, \
         COALESCE(i.employee_number, '') \
         FROM unnest($1::text[], $2::text[]) WITH ORDINALITY AS n(given, surname, idx) \
         JOIN identities i ON i.deleted_at IS NULL \
          AND ",
        folded!("i.given_name"),
        " = ",
        folded!("n.given"),
        " AND ",
        folded!("i.surname"),
        " = ",
        folded!("n.surname"),
        " ORDER BY n.idx, i.id"
    ))
    .bind(&given)
    .bind(&surname)
    .fetch_all(pool)
    .await?;
    let mut found = vec![None; names.len()];
    for (idx, id, person, number) in rows {
        if let Some(slot) = usize::try_from(idx - 1).ok().and_then(|i| found.get_mut(i)) {
            *slot = Some((id, person, number));
        }
    }
    Ok(found)
}

// Her hedef icin kimlik bazli is (ADR-016); connector'i olmayan hedefte
// worker sonucu "connector'i yok" yazar, ekran gizlemez (ADR-078).
pub async fn enqueue_all_targets(
    pool: &PgPool,
    identity_id: i64,
    priority: crate::jobs::Priority,
) -> Result<(), sqlx::Error> {
    let targets: Vec<i64> = sqlx::query_scalar("SELECT id FROM target_systems ORDER BY id")
        .fetch_all(pool)
        .await?;
    for target in targets {
        crate::jobs::enqueue(pool, identity_id, target, priority).await?;
    }
    Ok(())
}

pub struct Choice {
    pub id: String,
    pub label: String,
}

pub struct FormOptions {
    pub departments: Vec<Choice>,
    pub roles: Vec<Choice>,
    pub managers: Vec<Choice>,
}

// Yonetici: silinmemis ve ayrilmamis kimlikler (docs/03 "ayrildi secilemez").
pub async fn form_options(pool: &PgPool) -> Result<FormOptions, sqlx::Error> {
    Ok(FormOptions {
        departments: choices(pool, "SELECT id, name FROM departments ORDER BY name").await?,
        roles: choices(
            pool,
            "SELECT id, name FROM roles WHERE kind = 'primary' ORDER BY name",
        )
        .await?,
        managers: choices(
            pool,
            "SELECT id, given_name || ' ' || surname FROM identities \
             WHERE deleted_at IS NULL AND (end_at IS NULL OR end_at > now()) ORDER BY 2",
        )
        .await?,
    })
}

async fn choices(pool: &PgPool, sql: &'static str) -> Result<Vec<Choice>, sqlx::Error> {
    let rows: Vec<(i64, String)> = sqlx::query_as(sql).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, label)| Choice {
            id: id.to_string(),
            label,
        })
        .collect())
}

// Durum etiketinin rengi (arayuz kabugu, ADR-088): sablonda `badge badge-<kind>`.
pub fn state_kind(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Pending => "info",
        LifecycleState::Active => "ok",
        LifecycleState::Suspended => "warn",
        LifecycleState::Departed => "err",
        LifecycleState::Deleted => "muted",
    }
}

// account_links.applied_state anahtarlari (worker engine::state_name ile ayni).
pub fn state_key(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Pending => "pending",
        LifecycleState::Active => "active",
        LifecycleState::Suspended => "suspended",
        LifecycleState::Departed => "departed",
        LifecycleState::Deleted => "deleted",
    }
}

pub async fn load_state(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
) -> Result<Option<LifecycleState>, sqlx::Error> {
    let row: Option<TimelineRow> = sqlx::query_as(concat!(timeline_sql!("id = $1"), " LIMIT 1"))
        .bind(id)
        .bind(time_zone)
        .fetch_optional(pool)
        .await?;
    row.map(|r| timeline_from_row(&r).map(|(t, c)| lifecycle_state(&t, &c)))
        .transpose()
}

pub struct Listed {
    pub id: i64,
    pub name: String,
    /// Listedeki bas harf avatari (ADR-096 madde 3); kabugun cipiyle ayni yardimci
    pub initials: String,
    pub employee_number: String,
    /// Worker uretene kadar bos (ADR-015)
    pub username: String,
    /// Departman adi; atanmamissa bos
    pub department: String,
    pub state: &'static str,
    pub state_kind: &'static str,
}

/// (id, ad soyad, sicil, kullanici adi, departman) — liste sorgularinin ortak
/// cikti sirasi; `with_state` bu sirayi bekler
type ListedRow = (i64, String, Option<String>, Option<String>, Option<String>);

/// Liste sorgularinin ortak SELECT + JOIN'i; `$tail` derleme aninda eklenir
/// (`concat!`, calisma aninda birlestirme yok — `timeline_sql!` ile ayni kalip).
/// Kolon sirasi `ListedRow` ile birebir.
macro_rules! listed_select {
    () => {
        "SELECT i.id, i.given_name || ' ' || i.surname, i.employee_number, \
         i.username, d.name \
         FROM identities i LEFT JOIN departments d ON d.id = i.department_id "
    };
}

/// Ad, kullanici adi ve sicilde arama; kimlik numarasi kapsam disi (ADR-010).
/// Bos sorgu `%%` olur ve `given_name` NOT NULL oldugu icin her satiri getirir.
macro_rules! listed_where_match {
    () => {
        "i.deleted_at IS NULL AND ( \
               i.given_name ILIKE $1 ESCAPE '\\' OR i.surname ILIKE $1 ESCAPE '\\' \
            OR i.given_name || ' ' || i.surname ILIKE $1 ESCAPE '\\' \
            OR i.username ILIKE $1 ESCAPE '\\' OR i.employee_number ILIKE $1 ESCAPE '\\')"
    };
}

// ponytail: kimlik basina bir durum sorgusu, listeler 50 ile sinirli; tek
// sorguya almak `lifecycle_state`i SQL'e kopyalamak demek (ADR-038 tek yer).
async fn with_state(
    pool: &PgPool,
    time_zone: &str,
    rows: Vec<ListedRow>,
) -> Result<Vec<Listed>, sqlx::Error> {
    let mut listed = Vec::with_capacity(rows.len());
    for (id, name, employee_number, username, department) in rows {
        let state = load_state(pool, time_zone, id).await?;
        listed.push(Listed {
            id,
            initials: crate::shell::initials(&name),
            name,
            employee_number: employee_number.unwrap_or_default(),
            username: username.unwrap_or_default(),
            department: department.unwrap_or_default(),
            state: state.map(state_key).unwrap_or(""),
            state_kind: state.map(state_kind).unwrap_or("muted"),
        });
    }
    Ok(listed)
}

pub async fn recent(pool: &PgPool, time_zone: &str) -> Result<Vec<Listed>, sqlx::Error> {
    let rows: Vec<ListedRow> = sqlx::query_as(concat!(
        listed_select!(),
        "WHERE i.deleted_at IS NULL ORDER BY i.created_at DESC, i.id DESC LIMIT $1"
    ))
    .bind(HOME_RECENT_LIMIT)
    .fetch_all(pool)
    .await?;
    with_state(pool, time_zone, rows).await
}

/// Ust bardaki arama (ADR-096 madde 4): ad, soyad, tam ad, kullanici adi ve
/// sicil. Kimlik numarasi **aranmaz** — sifreli ve blind index'li (ADR-010),
/// aramasi `pii_reader` yetkisi ister; bu kutu her operatore acik.
pub async fn search(
    pool: &PgPool,
    time_zone: &str,
    query: &str,
) -> Result<Vec<Listed>, sqlx::Error> {
    let rows: Vec<ListedRow> = sqlx::query_as(concat!(
        listed_select!(),
        "WHERE ",
        listed_where_match!(),
        " ORDER BY i.surname, i.given_name, i.id LIMIT $2"
    ))
    .bind(like_contains(query))
    .bind(RECENT_LIMIT)
    .fetch_all(pool)
    .await?;
    with_state(pool, time_zone, rows).await
}

/// Panelin sayac kartlarinin isaret ettigi pencere filtreleri (ADR-117).
/// Deger izinli listeden gelir; baska bir metin filtre acmaz.
pub const WINDOW_FILTERS: [&str; 3] = ["joined", "departed", "changed"];

/// `?window=` degeri: yalnizca `WINDOW_FILTERS`tan biri, aksi halde filtre yok.
pub fn window_filter(requested: Option<&str>) -> Option<&str> {
    requested.filter(|w| WINDOW_FILTERS.contains(w))
}

/// Siralama secenekleri (ADR-117 E): anahtar, artan ve azalan `ORDER BY`
/// parcasi. Parcalar derleme zamani sabit, anahtar izinli listeden secilir —
/// kullanicinin yazdigi hicbir metin SQL'e girmez.
pub const SORTS: [(&str, &str, &str); 4] = [
    (
        // Anahtar ayni zamanda baslik metninin i18n adi: `field.full_name`
        "full_name",
        "i.surname, i.given_name, i.id",
        "i.surname DESC, i.given_name DESC, i.id DESC",
    ),
    (
        "employee_number",
        "i.employee_number, i.id",
        "i.employee_number DESC, i.id DESC",
    ),
    (
        "username",
        "i.username = '', i.username, i.id",
        "i.username = '', i.username DESC, i.id DESC",
    ),
    (
        "department",
        "d.name ASC NULLS LAST, i.surname, i.id",
        "d.name DESC NULLS LAST, i.surname, i.id",
    ),
];

/// `?sort=` ve `?dir=` degerlerinin `ORDER BY` parcasi; taninmayan anahtar
/// varsayilana (soyada gore artan) duser.
pub fn sort_clause(sort: Option<&str>, descending: bool) -> &'static str {
    let found = sort.and_then(|key| SORTS.iter().find(|(k, ..)| *k == key));
    let (_, asc, desc) = found.unwrap_or(&SORTS[0]);
    if descending {
        desc
    } else {
        asc
    }
}

/// Personel listesinin sorgusu: arama, filtreler, pencere, siralama ve sayfa.
pub struct Listing<'a> {
    pub query: &'a str,
    /// ADR-103 madde 4: yalnizca yer tutucu (`Tanimsiz`) rolu tasiyanlar
    pub unassigned_only: bool,
    /// ADR-117: panelin sayac kartindan gelen pencere (`WINDOW_FILTERS`), gun
    /// sayisiyla birlikte. `None` = filtre yok.
    pub window: Option<&'a str>,
    pub window_days: i32,
    /// Arac cubugunun filtreleri (ADR-117 E); `None` = filtre yok
    pub department: Option<i64>,
    pub role: Option<i64>,
    /// `ORDER BY` parcasi; yalnizca `sort_clause` uretir
    pub order: &'static str,
    pub offset: i64,
    pub limit: i64,
}

/// Yer tutucu rol filtresi; `listed_select!`in JOIN'lerinden sonra gelir.
macro_rules! listed_join_placeholder {
    () => {
        "JOIN roles r ON r.id = i.primary_role_id AND r.placeholder "
    };
}

/// Departman ve rol filtresi; `$d` ve `$r` NULL ise filtre yok. Numaralar
/// cagiran sorguya gore degisir (bkz. `listed_where_window!`).
macro_rules! listed_where_who {
    ($d:literal, $r:literal) => {
        concat!(
            " AND (",
            $d,
            "::bigint IS NULL OR i.department_id = ",
            $d,
            ")",
            " AND (",
            $r,
            "::bigint IS NULL OR i.primary_role_id = ",
            $r,
            ")"
        )
    };
}

/// Pencere filtresi (ADR-117): kosullar panelin `dashboard::totals` sorgusuyla
/// ayni — kart "son 30 gunde 7 kayit" diyorsa liste de yedi satir gostermeli.
/// Placeholder numaralari cagiran sorguya gore degisir (sayma sorgusunda $2'den,
/// satir sorgusunda $4'ten baslar), bu yuzden parametre olarak verilir:
/// `$w` pencere adi (NULL = filtre yok), `$tz` saat dilimi, `$d` gun sayisi,
/// `$c` `identity.changed` olay turu.
macro_rules! listed_where_window {
    ($w:literal, $tz:literal, $d:literal, $c:literal) => {
        concat!(
            " AND (",
            $w,
            "::text IS NULL",
            " OR (",
            $w,
            " = 'joined' AND i.start_date > (now() AT TIME ZONE ",
            $tz,
            ")::date - ",
            $d,
            "::int)",
            " OR (",
            $w,
            " = 'departed' AND i.end_at IS NOT NULL AND i.end_at <= now()",
            "      AND i.end_at > now() - make_interval(days => ",
            $d,
            "::int))",
            " OR (",
            $w,
            " = 'changed' AND EXISTS (SELECT 1 FROM audit_log a",
            "      WHERE a.identity_id = i.id AND a.event_type = ",
            $c,
            "      AND a.occurred_at > now() - make_interval(days => ",
            $d,
            "::int)))",
            ")"
        )
    };
}

/// Personel sayfasinin bir sayfasi (`/identities`): toplam sayi + satirlar.
/// Ust bardaki arama kutusu en fazla `RECENT_LIMIT` satir dondururken burada
/// liste sayfalanir — 20.000 kimlikte (N-03) tek sayfada basmak olmazdi.
/// Iki filtre hali derleme aninda iki sabit; calisma aninda birlestirme yok.
pub async fn page(
    pool: &PgPool,
    time_zone: &str,
    listing: &Listing<'_>,
) -> Result<(Vec<Listed>, i64), sqlx::Error> {
    let pattern = like_contains(listing.query);
    let (count_sql, rows_sql) = match listing.unassigned_only {
        false => (
            concat!(
                "SELECT count(*) FROM identities i WHERE ",
                listed_where_match!(),
                listed_where_window!("$2", "$3", "$4", "$5"),
                listed_where_who!("$6", "$7")
            ),
            concat!(
                listed_select!(),
                "WHERE ",
                listed_where_match!(),
                listed_where_window!("$4", "$5", "$6", "$7"),
                listed_where_who!("$8", "$9")
            ),
        ),
        true => (
            concat!(
                "SELECT count(*) FROM identities i ",
                listed_join_placeholder!(),
                "WHERE ",
                listed_where_match!(),
                listed_where_window!("$2", "$3", "$4", "$5"),
                listed_where_who!("$6", "$7")
            ),
            concat!(
                listed_select!(),
                listed_join_placeholder!(),
                "WHERE ",
                listed_where_match!(),
                listed_where_window!("$4", "$5", "$6", "$7"),
                listed_where_who!("$8", "$9")
            ),
        ),
    };
    // Siralama parcasi `sort_clause`in dondurdugu derleme zamani sabiti:
    // kullanicinin `?sort=`/`?dir=` degeri yalnizca `SORTS` icinden bir satir
    // *secer*, metni SQL'e hic girmez. `AssertSqlSafe` bu yuzden guvenli —
    // birlestirilen iki parcanin ikisi de literal (bkz. `sort_clause`).
    let rows_sql = sqlx::AssertSqlSafe(format!(
        "{rows_sql} ORDER BY {} LIMIT $2 OFFSET $3",
        listing.order
    ));
    let total: i64 = sqlx::query_scalar(count_sql)
        .bind(&pattern)
        .bind(listing.window)
        .bind(time_zone)
        .bind(listing.window_days)
        .bind(crate::audit::IDENTITY_CHANGED)
        .bind(listing.department)
        .bind(listing.role)
        .fetch_one(pool)
        .await?;
    let rows: Vec<ListedRow> = sqlx::query_as(rows_sql)
        .bind(&pattern)
        .bind(listing.limit)
        .bind(listing.offset)
        .bind(listing.window)
        .bind(time_zone)
        .bind(listing.window_days)
        .bind(crate::audit::IDENTITY_CHANGED)
        .bind(listing.department)
        .bind(listing.role)
        .fetch_all(pool)
        .await?;
    Ok((with_state(pool, time_zone, rows).await?, total))
}

/// ADR-103 madde 4: rolu yer tutucu (`Tanimsiz`) olan silinmemis kimlikler —
/// operatorun yapacak isi. Panel seridi ve personel listesi ayni sayiyi buradan okur.
pub async fn unassigned_role_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM identities i JOIN roles r ON r.id = i.primary_role_id \
         WHERE r.placeholder AND i.deleted_at IS NULL",
    )
    .fetch_one(pool)
    .await
}

/// Yer tutucu rolun id'si: toplu sahiplenme formunun varsayilani. Migration
/// seed'ler; tek satirdir (kismi tekil indeks).
/// docs/07, ADR-005: operator kendi kimlik kaydinda rolunu ve departmanini
/// degistiremez (kendine yetki grubu ya da OU veren degisiklik). Eslesme oturum
/// reddiyle ayni (`operator_guard::is_own_record`). `edit` (birincil rol,
/// departman) verilirse yalnizca biri degisiyorsa true; None: her rol islemi.
pub async fn own_role_change(
    pool: &PgPool,
    id: i64,
    username: &str,
    edit: Option<(i64, i64)>,
) -> Result<bool, sqlx::Error> {
    if !crate::operator_guard::is_own_record(pool, id, username).await? {
        return Ok(false);
    }
    let Some((primary, department)) = edit else {
        return Ok(true);
    };
    let changed: Option<bool> = sqlx::query_scalar(
        "SELECT primary_role_id <> $2 OR department_id <> $3 FROM identities WHERE id = $1",
    )
    .bind(id)
    .bind(primary)
    .bind(department)
    .fetch_optional(pool)
    .await?;
    Ok(changed == Some(true))
}

/// Degismez kural: `mode = 'managed'` baglantinin kimliginde yer tutucu rol olmaz.
/// Kapi kalkarsa gece dolumu (`fill_placeholder_roles`) yonetilen hesabin rolunu is
/// acmadan degistirir ve hesap eski OU'da kalir. Kayit her hedefte yonetilen hesap
/// actirdigi icin yer tutucuyu hic almaz; duzenleme yalnizca yonetilen baglantisi
/// olmayan kimlikte (gozlemdeki toplu sahiplenme, ADR-103) birakir.
pub async fn placeholder_refused(
    pool: &PgPool,
    role_id: i64,
    identity: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let refused: Option<bool> = sqlx::query_scalar(
        "SELECT r.placeholder AND ($2::bigint IS NULL OR EXISTS (SELECT 1 FROM account_links l \
         WHERE l.identity_id = $2 AND l.mode = 'managed' AND l.deleted_by_us_at IS NULL)) \
         FROM roles r WHERE r.id = $1",
    )
    .bind(role_id)
    .bind(identity)
    .fetch_optional(pool)
    .await?;
    Ok(refused == Some(true))
}

pub async fn placeholder_role_id(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM roles WHERE placeholder")
        .fetch_optional(pool)
        .await
}

/// `%ara%` kalibi; kullanicinin yazdigi `%` ve `_` joker degil harf sayilir.
fn like_contains(query: &str) -> String {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub struct Person {
    pub id: i64,
    pub name: String,
    pub employee_number: String,
    pub mobile_phone: String,
    pub national_id_masked: String,
    pub department: String,
    pub role: String,
    /// ADR-103: rol yer tutucu (`Tanimsiz`) — ekran rozet basar, yonetime alma kapali
    pub role_placeholder: bool,
    pub manager: String,
    pub employment_type: String,
    pub start_date: String,
    pub end_at: String,
    pub username: String,
    pub email: String,
    pub upn: String,
    pub requested_username: String,
    pub name_conflict_override: bool,
}

impl Person {
    /// Kisi sayfasi basligindaki bas harf avatari; liste satirlariyla (`Listed`)
    /// ayni yardimci, boylece ayni kisi her yerde ayni harfleri tasir.
    pub fn initials(&self) -> String {
        crate::shell::initials(&self.name)
    }
}

// ADR-022/042: istek ve karar yalnizca ad henuz olusmamisken yazilir (false = olusmus).
pub async fn request_names(
    pool: &PgPool,
    id: i64,
    requested_username: Option<&str>,
    name_conflict_override: bool,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE identities SET requested_username = $2, name_conflict_override = $3 \
         WHERE id = $1 AND username IS NULL",
    )
    .bind(id)
    .bind(requested_username)
    .bind(name_conflict_override)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub struct Account {
    pub target_id: i64,
    pub target: String,
    pub external_id: String,
    pub origin: String,
    pub mode: String,
    pub applied_state: String,
    pub diff: String,
    /// ADR-018: gozlem modu — fark gosterilir, "Yonetime al" dugmesi cikar
    pub observed: bool,
    /// ADR-087: istek yazildi, worker modu cevirecek
    pub manage_requested: bool,
}

pub struct Job {
    pub id: i64,
    pub target: String,
    /// Veritabani durumu; ekran karsiligi i18n'de (job.<anahtar>)
    pub status: String,
    pub status_kind: &'static str,
    pub attempts: i32,
    pub next_attempt_at: String,
    /// Son hareket: is bittiyse bitis, bitmediyse olusturulma ani
    pub at: String,
    /// Bitmemis is: deneme sayisi ve sonraki deneme anlamli, bitmiste degil
    pub pending: bool,
    pub summary: String,
    pub detail: String,
    /// ADR-050 freni: hata degil bekleme; operator dilinde sebep (F-12)
    pub waiting: String,
    pub result: String,
    pub retryable: bool,
}

pub struct Event {
    pub at: String,
    /// Ekran karsiligi i18n'de (`event.<tur>`)
    pub event_type: String,
    /// Olayin uzerinde oldugu sey: grup adi, oznitelik listesi, acil ayrilis
    /// gerekcesi. Ham DN degil — operator tanidigi adi gorur.
    pub subject: String,
    /// Worker satirlari operator adi tasimaz; bos ise ekran "otomatik" der
    pub actor: String,
    pub outcome: String,
    pub outcome_kind: &'static str,
    pub icon: &'static str,
    /// Olay kategorisi (`.feed-ico--<kategori>`); panelin akisiyla ayni tablo
    pub category: &'static str,
}

pub struct PersonPage {
    pub person: Person,
    /// Turetilen durum anahtari; ekran karsiligi i18n'de (state.<anahtar>)
    pub state: &'static str,
    pub state_kind: &'static str,
    pub accounts: Vec<Account>,
    pub jobs: Vec<Job>,
    pub events: Vec<Event>,
    /// Ad henuz yok ve bir is mudahalede: ADR-022/042 secenekleri gosterilir
    pub name_intervention: bool,
    pub additional_roles: Vec<AssignedRole>,
    pub role_options: Vec<Choice>,
    pub lifecycle: LifecycleInfo,
}

/// Kisi sayfasindaki yasam dongusu bolumu (docs/04; ADR-030/048/053/059/084).
pub struct LifecycleInfo {
    pub end_date: String,
    pub handover_manager_id: String,
    pub suspension_start: String,
    pub suspension_end: String,
    /// Iznin son gununun ertesi: "hesaplar X 00:00'da acilir" (ADR-059 madde 3)
    pub return_day: String,
    pub cancelled: bool,
    pub emergency: bool,
    pub departed: bool,
    pub deleted: bool,
    /// Hic sahiplenilmis baglantisi yoksa iptal dugmesi gosterilir (ADR-048)
    pub can_cancel: bool,
    /// ADR-041/F-38: yonetici ayrilmis mi ve (varsa) devir yoneticisinin adi
    pub manager_departed: bool,
    pub handover_name: String,
    pub subordinates: i64,
    /// Kimlik ayrilmis ve etkin devir yoneticisi yok: astlar yoneticisiz kalir
    pub orphaned_subordinates: bool,
    pub departure_note: String,
    /// Ayrilmis kimligin hesaplarinin saklama/silme durumu (ADR-111 madde 4)
    pub deletions: Vec<crate::deletions::Row>,
}

// Yoneticinin durumu ve devir yoneticisi; etkin yonetici ADR-041 tek atlama.
// Doner: (yonetici ayrilmis mi, devir yoneticisinin adi).
async fn manager_info(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
) -> Result<(bool, String), sqlx::Error> {
    let row: Option<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT m.id, m.handover_manager_id, h.given_name || ' ' || h.surname \
         FROM identities i JOIN identities m ON m.id = i.manager_id \
         LEFT JOIN identities h ON h.id = m.handover_manager_id WHERE i.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((manager, handover, handover_name)) = row else {
        return Ok((false, String::new()));
    };
    let gone = |s: Option<LifecycleState>| {
        matches!(
            s,
            Some(LifecycleState::Departed) | Some(LifecycleState::Deleted)
        )
    };
    if !gone(load_state(pool, time_zone, manager).await?) {
        return Ok((false, String::new()));
    }
    let handover_state = match handover {
        Some(h) => load_state(pool, time_zone, h).await?,
        None => None,
    };
    Ok(
        match (handover_name, gone(handover_state), handover.is_some()) {
            (Some(name), false, true) => (true, name),
            _ => (true, String::new()),
        },
    )
}

async fn subordinates(pool: &PgPool, id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM identities WHERE manager_id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_one(pool)
    .await
}

async fn load_lifecycle(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
    state: LifecycleState,
) -> Result<LifecycleInfo, sqlx::Error> {
    type Row = (
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        bool,
        bool,
        Option<String>,
    );
    let r: Row = sqlx::query_as(
        "SELECT to_char((end_at AT TIME ZONE $2) - interval '1 day', 'YYYY-MM-DD'), \
         handover_manager_id, to_char(suspension_start, 'YYYY-MM-DD'), \
         to_char(suspension_end, 'YYYY-MM-DD'), to_char(suspension_end + 1, 'YYYY-MM-DD'), \
         cancelled, emergency_departure, \
         NOT EXISTS (SELECT 1 FROM account_links l WHERE l.identity_id = i.id AND l.origin = 'adopted'), \
         departure_note FROM identities i WHERE id = $1",
    )
    .bind(id)
    .bind(time_zone)
    .fetch_one(pool)
    .await?;
    let manager = manager_info(pool, time_zone, id).await?;
    Ok(LifecycleInfo {
        end_date: r.0.unwrap_or_default(),
        handover_manager_id: r.1.map(|m| m.to_string()).unwrap_or_default(),
        suspension_start: r.2.unwrap_or_default(),
        suspension_end: r.3.unwrap_or_default(),
        return_day: r.4.unwrap_or_default(),
        cancelled: r.5,
        emergency: r.6,
        departed: state == LifecycleState::Departed,
        deleted: state == LifecycleState::Deleted,
        can_cancel: r.7 && state != LifecycleState::Deleted,
        manager_departed: manager.0,
        handover_name: manager.1,
        subordinates: subordinates(pool, id).await?,
        orphaned_subordinates: state == LifecycleState::Departed
            && !handover_effective(pool, time_zone, r.1).await?,
        departure_note: r.8.unwrap_or_default(),
        deletions: crate::deletions::load_for(pool, time_zone, Some(id)).await?,
    })
}

async fn handover_effective(
    pool: &PgPool,
    time_zone: &str,
    handover: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let Some(h) = handover else {
        return Ok(false);
    };
    Ok(matches!(
        load_state(pool, time_zone, h).await?,
        Some(LifecycleState::Active | LifecycleState::Pending | LifecycleState::Suspended)
    ))
}

#[derive(Debug, PartialEq, Eq)]
pub enum LifecycleChange {
    Applied,
    /// `ayrildi`dan cikis: geri alma sayilir (ADR-059 madde 2)
    Reverted,
    Rejected(&'static str),
}

/// Ayrilis formu: son calisma gunu, devir yoneticisi ve serbest metin neden (ADR-111).
pub struct Departure<'a> {
    pub end_date: &'a str,
    pub handover: Option<i64>,
    pub note: &'a str,
}

/// AD `description`a girer (ADR-111); tek satir ve kisa tutulur.
pub const DEPARTURE_NOTE_MAX_CHARS: usize = 200;

/// Saf: bos → None, uzun ya da cok satirli → hata anahtari.
pub fn departure_note(raw: &str) -> Result<Option<String>, &'static str> {
    let note = raw.trim();
    if note.chars().count() > DEPARTURE_NOTE_MAX_CHARS || note.chars().any(char::is_control) {
        return Err("err.departure_note_invalid");
    }
    Ok((!note.is_empty()).then(|| note.to_string()))
}

// Planli ayrilis: son calisma gunu → ertesi gun 00:00 (ADR-038). Kimlik `ayrildi`
// iken ileri tarih = geri alma.
pub async fn set_departure(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
    d: &Departure<'_>,
) -> Result<LifecycleChange, sqlx::Error> {
    if Date::from_iso(d.end_date).is_none() {
        return Ok(LifecycleChange::Rejected("err.end_date_format"));
    }
    let note = match departure_note(d.note) {
        Ok(note) => note,
        Err(key) => return Ok(LifecycleChange::Rejected(key)),
    };
    let before = load_state(pool, time_zone, id).await?;
    let done = sqlx::query(
        "UPDATE identities SET end_at = (($2::date + 1)::timestamp AT TIME ZONE $3), \
         handover_manager_id = $4, departure_note = $5, emergency_departure = FALSE, \
         cancelled = FALSE \
         WHERE id = $1 AND deleted_at IS NULL AND $2::date >= start_date \
         AND ($4::bigint IS NULL OR $4 <> $1)",
    )
    .bind(id)
    .bind(d.end_date)
    .bind(time_zone)
    .bind(d.handover)
    .bind(note)
    .execute(pool)
    .await?;
    if done.rows_affected() != 1 {
        return Ok(LifecycleChange::Rejected("err.departure_rejected"));
    }
    after_departed(pool, time_zone, id, before).await
}

async fn after_departed(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
    before: Option<LifecycleState>,
) -> Result<LifecycleChange, sqlx::Error> {
    let after = load_state(pool, time_zone, id).await?;
    Ok(
        if before == Some(LifecycleState::Departed) && after != Some(LifecycleState::Departed) {
            LifecycleChange::Reverted
        } else {
            LifecycleChange::Applied
        },
    )
}

// Acil ayrilis: bitis ani simdi, acil isareti (ADR-016/033); gerekce denetimde.
pub async fn set_emergency_departure(
    pool: &PgPool,
    id: i64,
    handover: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE identities SET end_at = now(), emergency_departure = TRUE, cancelled = FALSE, \
         handover_manager_id = $2 WHERE id = $1 AND deleted_at IS NULL \
         AND ($2::bigint IS NULL OR $2 <> $1)",
    )
    .bind(id)
    .bind(handover)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

// Geri alma (ADR-030/059): bitis kaldirilir; donus gunu ilerideyse baslangic o gun.
// Kadrolu disinda bitis zorunlu (docs/03): orada geri alma ileri tarihli bitisle
// yapilir (set_departure → Reverted).
pub async fn revert_departure(
    pool: &PgPool,
    id: i64,
    return_day: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE identities SET end_at = NULL, emergency_departure = FALSE, cancelled = FALSE, \
         departure_note = NULL, start_date = COALESCE($2::date, start_date) \
         WHERE id = $1 AND deleted_at IS NULL AND end_at IS NOT NULL \
         AND employment_type = 'permanent'",
    )
    .bind(id)
    .bind(return_day)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

// Kayit iptali (ADR-048): isaret + bitis simdi; dogrulama worker'da.
pub async fn cancel_registration(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE identities SET cancelled = TRUE, end_at = now(), emergency_departure = FALSE \
         WHERE id = $1 AND deleted_at IS NULL \
         AND NOT EXISTS (SELECT 1 FROM account_links l WHERE l.identity_id = $1 AND l.origin = 'adopted')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

// Tarihli aski (ADR-053): ilk gun ve iznin son gunu; bitis >= baslangic (CHECK).
pub async fn set_suspension(
    pool: &PgPool,
    id: i64,
    start: &str,
    end: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if Date::from_iso(start).is_none() || end.is_some_and(|e| Date::from_iso(e).is_none()) {
        return Ok(false);
    }
    if end.is_some_and(|e| e < start) {
        return Ok(false);
    }
    let done = sqlx::query(
        "UPDATE identities SET suspension_start = $2::date, suspension_end = $3::date \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(start)
    .bind(end)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn lift_suspension(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE identities SET suspension_start = NULL, suspension_end = NULL \
         WHERE id = $1 AND suspension_start IS NOT NULL",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn load_page(
    pool: &PgPool,
    time_zone: &str,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
    lang: Lang,
) -> Result<Option<PersonPage>, sqlx::Error> {
    let Some(state) = load_state(pool, time_zone, id).await? else {
        return Ok(None);
    };
    let person = load_person(pool, time_zone, aead_key, id, lang).await?;
    let jobs = load_jobs(pool, lang, time_zone, id).await?;
    let name_intervention =
        person.username.is_empty() && jobs.iter().any(|j| j.status == INTERVENTION_STATUS);
    Ok(Some(PersonPage {
        state: state_key(state),
        state_kind: state_kind(state),
        accounts: load_accounts(pool, id, state, lang).await?,
        events: load_events(pool, time_zone, id).await?,
        additional_roles: load_additional_roles(pool, id).await?,
        role_options: choices(
            pool,
            "SELECT id, name FROM roles WHERE kind = 'additional' ORDER BY name",
        )
        .await?,
        lifecycle: load_lifecycle(pool, time_zone, id, state).await?,
        person,
        jobs,
        name_intervention,
    }))
}

// Duzenleme formu mevcut degerlerle dolar (ADR-083); tarihler ve kimlik no formda yok
// ama validate() icin tasinir.
pub async fn load_form(pool: &PgPool, id: i64) -> Result<Option<IdentityForm>, sqlx::Error> {
    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
        i64,
        Option<i64>,
        String,
        String,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT given_name, surname, employee_number, mobile_phone, department_id, \
         primary_role_id, manager_id, employment_type, to_char(start_date, 'YYYY-MM-DD'), \
         to_char(end_at - interval '1 second', 'YYYY-MM-DD') FROM identities WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| IdentityForm {
        given_name: r.0,
        surname: r.1,
        employee_number: r.2.unwrap_or_default(),
        mobile_phone: r.3.unwrap_or_default(),
        department_id: r.4.to_string(),
        primary_role_id: r.5.to_string(),
        manager_id: r.6.map(|m| m.to_string()).unwrap_or_default(),
        employment_type: r.7,
        start_date: r.8,
        end_date: r.9.unwrap_or_default(),
        national_id_country: "TR".to_string(),
        ..IdentityForm::default()
    }))
}

// Gorev degisikligi / calisma tipi donusumu (docs/04 Mover, ADR-042): tarihler,
// kimlik no ve kullanici adi bu yoldan degismez.
pub async fn update_mover(pool: &PgPool, id: i64, new: &NewIdentity) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE identities SET given_name = $2, surname = $3, employee_number = $4, \
         mobile_phone = $5, department_id = $6, primary_role_id = $7, manager_id = $8, \
         employment_type = $9 WHERE id = $1 AND manager_id IS DISTINCT FROM id",
    )
    .bind(id)
    .bind(&new.given_name)
    .bind(&new.surname)
    .bind(&new.employee_number)
    .bind(&new.mobile_phone)
    .bind(new.department_id)
    .bind(new.primary_role_id)
    .bind(new.manager_id.filter(|m| *m != id))
    .bind(&new.employment_type)
    .execute(pool)
    .await?;
    Ok(())
}

pub struct AssignedRole {
    pub role_id: i64,
    pub name: String,
    pub ends_on: String,
    /// ADR-020: bitisi 30 gun icinde — zamanlayici rolu kendiliginden silecek,
    /// operator once gormus olsun
    pub ends_soon: bool,
}

pub async fn load_additional_roles(
    pool: &PgPool,
    id: i64,
) -> Result<Vec<AssignedRole>, sqlx::Error> {
    let rows: Vec<(i64, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT a.role_id, r.name, to_char(a.ends_on, 'YYYY-MM-DD'), \
         coalesce(a.ends_on - current_date BETWEEN 0 AND 30, false) \
         FROM identity_additional_roles a JOIN roles r ON r.id = a.role_id \
         WHERE a.identity_id = $1 ORDER BY r.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(role_id, name, ends_on, ends_soon)| AssignedRole {
            role_id,
            name,
            ends_on: ends_on.unwrap_or_default(),
            ends_soon,
        })
        .collect())
}

// ADR-020: bitis gunun sonudur, gecmis tarihli atama kaydedilemez; upsert tarihi uzatir.
pub async fn assign_role(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
    role_id: i64,
    ends_on: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if let Some(d) = ends_on {
        let future: bool = sqlx::query_scalar("SELECT $1::date >= (now() AT TIME ZONE $2)::date")
            .bind(d)
            .bind(time_zone)
            .fetch_one(pool)
            .await?;
        if !future {
            return Ok(false);
        }
    }
    sqlx::query(
        "INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) VALUES ($1, $2, $3::date) \
         ON CONFLICT (identity_id, role_id) DO UPDATE SET ends_on = EXCLUDED.ends_on",
    )
    .bind(id)
    .bind(role_id)
    .bind(ends_on)
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn remove_role(pool: &PgPool, id: i64, role_id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM identity_additional_roles WHERE identity_id = $1 AND role_id = $2",
    )
    .bind(id)
    .bind(role_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

// sqlx tuple'lari en fazla 16 kolon tasir: ad ve soyad SQL'de birlestirilir.
type PersonRow = (
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    bool,
);

async fn load_person(
    pool: &PgPool,
    time_zone: &str,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
    lang: Lang,
) -> Result<Person, sqlx::Error> {
    let r: PersonRow = sqlx::query_as(
        "SELECT i.given_name || ' ' || i.surname, i.employee_number, i.mobile_phone, \
         i.national_id_enc, d.name, r.name, m.given_name || ' ' || m.surname, \
         i.employment_type, to_char(i.start_date, 'YYYY-MM-DD'), \
         to_char(i.end_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), i.username, i.email, i.upn, \
         i.requested_username, i.name_conflict_override, r.placeholder \
         FROM identities i JOIN departments d ON d.id = i.department_id \
         JOIN roles r ON r.id = i.primary_role_id \
         LEFT JOIN identities m ON m.id = i.manager_id WHERE i.id = $1",
    )
    .bind(id)
    .bind(time_zone)
    .fetch_one(pool)
    .await?;
    Ok(Person {
        id,
        name: r.0,
        employee_number: r.1.unwrap_or_default(),
        mobile_phone: r.2.unwrap_or_default(),
        national_id_masked: r
            .3
            .as_deref()
            .map(|enc| masked(aead_key, enc, lang))
            .unwrap_or_default(),
        department: r.4,
        role: r.5,
        manager: r.6.unwrap_or_default(),
        employment_type: r.7,
        start_date: r.8,
        end_at: r.9.unwrap_or_default(),
        username: r.10.unwrap_or_default(),
        email: r.11.unwrap_or_default(),
        upn: r.12.unwrap_or_default(),
        requested_username: r.13.unwrap_or_default(),
        name_conflict_override: r.14,
        role_placeholder: r.15,
    })
}

// Acik goruntuleme ayri yetki ve denetim kaydi ister (docs/07); burada yalnizca maske.
fn masked(aead_key: &[u8; crate::crypto::KEY_LEN], enc: &[u8], lang: Lang) -> String {
    match national_id::decrypt(aead_key, enc) {
        Ok(value) => national_id::mask(&value),
        Err(e) => {
            log_error!("identity: kimlik numarası çözülemedi: {e}");
            lang.t("person.undecryptable").to_string()
        }
    }
}

// Hedefteki fark (3a kapsami, ADR-078): turetilen durum ↔ applied_state. Gozlem
// modunda farki motor hesaplar (ADR-018/087); son is sonucu oldugu gibi gosterilir.
type AccountRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
);

const ACCOUNTS_SQL: &str =
    "SELECT t.id, t.name, l.external_id, l.origin, l.mode, l.applied_state, \
     l.manage_requested_at IS NOT NULL, \
     (SELECT j.result FROM jobs j WHERE j.identity_id = $1 AND j.target_system_id = t.id \
     AND j.result IS NOT NULL ORDER BY j.finished_at DESC NULLS LAST, j.id DESC LIMIT 1) \
     FROM target_systems t \
     LEFT JOIN account_links l ON l.target_system_id = t.id AND l.identity_id = $1 \
     ORDER BY t.id";

// Gozlem modunda fark motorun son is sonucundan gelir (ADR-087); yonetilen
// baglantida turetilen durum ile `applied_state` karsilastirilir.
fn account_from(row: AccountRow, state: LifecycleState, lang: Lang) -> Account {
    let (target_id, target, external_id, origin, mode, applied, requested, result) = row;
    let observed = mode.as_deref() == Some(OBSERVED_MODE);
    Account {
        target_id,
        target,
        diff: match observed {
            true => result.unwrap_or_else(|| lang.t("diff.observed_pending").to_string()),
            false => diff_text(lang, state, external_id.is_some(), applied.as_deref()),
        },
        external_id: external_id.unwrap_or_default(),
        origin: origin.unwrap_or_default(),
        mode: mode.unwrap_or_default(),
        applied_state: applied.unwrap_or_default(),
        observed,
        manage_requested: requested,
    }
}

async fn load_accounts(
    pool: &PgPool,
    id: i64,
    state: LifecycleState,
    lang: Lang,
) -> Result<Vec<Account>, sqlx::Error> {
    let rows: Vec<AccountRow> = sqlx::query_as(ACCOUNTS_SQL)
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| account_from(row, state, lang))
        .collect())
}

const OBSERVED_MODE: &str = "observed";

#[derive(Debug, PartialEq, Eq)]
pub enum ManageOutcome {
    Requested,
    /// Gozlem modunda baglanti yok (yonetiliyor ya da silinmis)
    NoObservedAccount,
    /// ADR-103 madde 5: rolu yer tutucu olan kimlik yonetime alinamaz — motor
    /// rolde karsiligi olmayan uyelikleri fazlalik sayip sokerdi
    RoleUndefined,
}

// ADR-018/087: operator farki gorup onaylar; backend yalnizca istegi yazar, modu
// worker cevirir. Ayrilis yolu (`request_management_observed`) bu kapidan gecmez:
// orada uyeliklerin sokulmesi zaten istenen sonuctur.
pub async fn request_management(
    pool: &PgPool,
    id: i64,
    target_system_id: i64,
) -> Result<ManageOutcome, sqlx::Error> {
    let undefined: Option<bool> = sqlx::query_scalar(
        "SELECT r.placeholder FROM identities i JOIN roles r ON r.id = i.primary_role_id \
         WHERE i.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if undefined == Some(true) {
        return Ok(ManageOutcome::RoleUndefined);
    }
    let done = sqlx::query(
        "UPDATE account_links SET manage_requested_at = now() \
         WHERE identity_id = $1 AND target_system_id = $2 AND mode = $3 \
         AND deleted_by_us_at IS NULL",
    )
    .bind(id)
    .bind(target_system_id)
    .bind(OBSERVED_MODE)
    .execute(pool)
    .await?;
    Ok(match done.rows_affected() == 1 {
        true => ManageOutcome::Requested,
        false => ManageOutcome::NoObservedAccount,
    })
}

// ADR-018: gozlem modundaki kimlige ayrilis kaydedilirse ayni islem yonetime almayi
// da icerir; yoksa "ayrilis kaydedildi ama hicbir sey olmadi" olurdu.
pub async fn request_management_observed(pool: &PgPool, id: i64) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE account_links SET manage_requested_at = now() \
         WHERE identity_id = $1 AND mode = $2 AND deleted_by_us_at IS NULL",
    )
    .bind(id)
    .bind(OBSERVED_MODE)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

// account_links koken/mod ve durum anahtarlarinin ekran karsiligi i18n'de
// (link.<anahtar>, state.<anahtar>); burada yalnizca fark cumlesi kurulur.
pub fn diff_text(lang: Lang, state: LifecycleState, linked: bool, applied: Option<&str>) -> String {
    let desired = state_key(state);
    match applied {
        _ if !linked => lang.t("diff.no_account").to_string(),
        Some(a) if a == desired => lang.t("diff.in_sync").to_string(),
        Some(a) => lang.tn(
            "diff.mismatch",
            &[lang.key("state", a), lang.key("state", desired)],
        ),
        None => lang.t1("diff.not_applied", lang.key("state", desired)),
    }
}

// Is durumunun rengi (arayuz kabugu, ADR-088): sablonda `badge badge-<kind>`.
fn status_kind(status: &str) -> &'static str {
    match status {
        "queued" => "info",
        "running" => "info",
        "succeeded" => "ok",
        "needs_intervention" => "err",
        _ => "muted",
    }
}

/// ADR-050/091 freni is satirina `throttle:<sinif>:<kullanilan>/<sinir>` yazar;
/// bu bir hata degil beklemedir ve operatorun dilinde gosterilir (F-12).
pub fn throttle_reason(lang: Lang, last_error: &str) -> Option<String> {
    let rest = last_error.strip_prefix("throttle:")?;
    let (class, usage) = rest.split_once(':')?;
    let (used, limit) = usage.split_once('/')?;
    Some(lang.tn("wait.counter", &[lang.key("counter", class), used, limit]))
}

// Worker hata metni "sebep: teknik ayrinti" sozlesmesindedir (ADR-078 madde 6).
pub fn split_error(last_error: &str) -> (String, String) {
    let text = last_error
        .strip_prefix(INTERVENTION_PREFIX)
        .unwrap_or(last_error);
    match text.split_once(": ") {
        Some((summary, detail)) => (summary.to_string(), detail.to_string()),
        None => (text.to_string(), String::new()),
    }
}

type JobRow = (
    i64,
    String,
    String,
    i32,
    String,
    String,
    Option<String>,
    Option<String>,
    bool,
);

pub(crate) async fn load_jobs(
    pool: &PgPool,
    lang: Lang,
    time_zone: &str,
    id: i64,
) -> Result<Vec<Job>, sqlx::Error> {
    let rows: Vec<JobRow> = sqlx::query_as(
        "SELECT j.id, t.name, j.status, j.attempts, \
         to_char(j.next_attempt_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), \
         to_char(coalesce(j.finished_at, j.created_at) AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), \
         j.last_error, j.result, j.retry_requested FROM jobs j \
         JOIN target_systems t ON t.id = j.target_system_id \
         WHERE j.identity_id = $1 ORDER BY j.created_at DESC, j.id DESC",
    )
    .bind(id)
    .bind(time_zone)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| job_from(lang, row)).collect())
}

fn job_from(lang: Lang, row: JobRow) -> Job {
    let (id, target, status, attempts, next, at, error, result, retry) = row;
    let waiting = error
        .as_deref()
        .and_then(|e| throttle_reason(lang, e))
        .unwrap_or_default();
    let (summary, detail) = match waiting.is_empty() {
        true => error.as_deref().map(split_error).unwrap_or_default(),
        false => (String::new(), String::new()),
    };
    Job {
        id,
        target,
        status_kind: status_kind(&status),
        attempts,
        next_attempt_at: next,
        at,
        pending: status != "succeeded",
        summary,
        detail,
        waiting,
        result: result.unwrap_or_default(),
        retryable: status == INTERVENTION_STATUS && !retry,
        status,
    }
}

/// DN'in ilk bileseninin degeri: `CN=GG-Takim,OU=Groups,...` → `GG-Takim`.
/// Denetim kaydi DN'in tamamini tutar, operator tanidigi adi gormek ister.
fn first_rdn(dn: &str) -> String {
    let head = dn.split(',').next().unwrap_or(dn).trim();
    head.split_once('=').map_or(head, |(_, value)| value).into()
}

pub(crate) fn outcome_kind(outcome: &str) -> &'static str {
    match outcome {
        "succeeded" => "ok",
        "failed" => "err",
        // Etkinlik gecmisi: sonuc satiri olmayan niyet (ADR-062)
        "unknown" => "warn",
        _ => "",
    }
}

type EventRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

// Worker her hedef islemi icin niyet ve sonuc olmak uzere iki satir yazar (0006).
// Ekranda tek satir olmalari icin sonuc niyete `intent_id` ile baglanir, ciplak
// sonuc satiri listeye hic girmez: operator "ayni olay iki kez" gormez.
async fn load_events(pool: &PgPool, time_zone: &str, id: i64) -> Result<Vec<Event>, sqlx::Error> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT to_char(a.occurred_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), a.event_type, \
         coalesce(o.outcome, a.outcome, ''), a.actor_username, \
         coalesce(a.detail->>'group', a.detail->>'dn'), a.detail->>'reason', \
         CASE WHEN jsonb_typeof(a.detail->'attributes') = 'array' THEN \
           (SELECT string_agg(x->>'name', ', ' ORDER BY x->>'name') \
            FROM jsonb_array_elements(a.detail->'attributes') x) END \
         FROM audit_log a LEFT JOIN audit_log o ON o.intent_id = a.id \
         WHERE a.identity_id = $1 AND a.intent_id IS NULL \
         ORDER BY a.occurred_at DESC, a.id DESC LIMIT $3",
    )
    .bind(id)
    .bind(time_zone)
    .bind(EVENT_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(at, event_type, outcome, actor, dn, reason, attributes)| {
            let subject = match (attributes, dn, reason) {
                (Some(list), _, _) => list,
                (None, Some(dn), _) => first_rdn(&dn),
                (None, None, Some(reason)) => reason,
                _ => String::new(),
            };
            let (icon, category) = crate::dashboard::glyph_for(&event_type);
            Event {
                at,
                subject,
                actor: actor.unwrap_or_default(),
                outcome_kind: outcome_kind(&outcome),
                outcome,
                event_type,
                icon,
                category,
            }
        })
        .collect())
}
// --- END FEATURE: identity-registration ---

#[cfg(test)]
mod tests {
    use super::*;

    fn dep(end_date: &str, handover: Option<i64>) -> Departure<'_> {
        Departure {
            end_date,
            handover,
            note: "",
        }
    }

    async fn note_of(pool: &PgPool, id: i64) -> Option<String> {
        sqlx::query_scalar("SELECT departure_note FROM identities WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn departure_note_is_trimmed_single_line_and_bounded() {
        assert_eq!(departure_note("  "), Ok(None));
        assert_eq!(departure_note(" istifa "), Ok(Some("istifa".to_string())));
        assert!(departure_note("a\nb").is_err());
        let max = "ç".repeat(DEPARTURE_NOTE_MAX_CHARS);
        assert!(
            departure_note(&max).is_ok(),
            "sınır karakterle sayılır, baytla değil"
        );
        assert!(departure_note(&format!("{max}x")).is_err());
    }

    fn form() -> IdentityForm {
        IdentityForm {
            given_name: " Ayşe ".to_string(),
            surname: "Yılmaz".to_string(),
            national_id_country: "TR".to_string(),
            department_id: "1".to_string(),
            primary_role_id: "2".to_string(),
            employment_type: "contract".to_string(),
            start_date: "2026-10-01".to_string(),
            end_date: "2026-12-31".to_string(),
            ..IdentityForm::default()
        }
    }

    #[test]
    fn validate_trims_and_parses_fields() {
        let mut f = form();
        f.mobile_phone = "+905321234567".to_string();
        f.national_id = "100 000 001 46".to_string();
        f.manager_id = "7".to_string();
        let n = validate(&f).unwrap();
        assert_eq!(n.given_name, "Ayşe");
        assert_eq!(n.mobile_phone.as_deref(), Some("+905321234567"));
        assert_eq!(n.national_id.unwrap().value, "10000000146");
        assert_eq!(n.manager_id, Some(7));
        assert_eq!(n.end_date.as_deref(), Some("2026-12-31"));
    }

    // Dogrulama i18n anahtari dondurur (ADR-089); operator metni web katmaninda cevrilir.
    #[test]
    fn validate_rejects_bad_input_with_operator_messages() {
        let check = |mutate: fn(&mut IdentityForm), expected: &str| {
            let mut f = form();
            mutate(&mut f);
            let key = validate(&f).unwrap_err();
            assert_eq!(key, expected);
            assert_ne!(Lang::Tr.t(key), key, "anahtar tr.toml'de yok: {key}");
        };
        check(|f| f.given_name = "  ".to_string(), "err.given_name_blank");
        check(
            |f| f.department_id = String::new(),
            "err.department_required",
        );
        check(
            |f| f.employment_type = "x".to_string(),
            "err.employment_type_required",
        );
        check(
            |f| f.start_date = "01.10.2026".to_string(),
            "err.start_date_format",
        );
        check(
            |f| f.end_date = String::new(),
            "err.end_required_non_permanent",
        );
        check(
            |f| f.end_date = "2026-09-01".to_string(),
            "err.end_before_start",
        );
        check(
            |f| f.mobile_phone = "05321234567".to_string(),
            "err.mobile_format",
        );
        check(
            |f| f.mobile_phone = "+0532".to_string(),
            "err.mobile_format",
        );
        check(
            |f| f.national_id = "10000000147".to_string(),
            "err.tr_id_checksum",
        );
    }

    #[test]
    fn requested_username_shape_is_checked_here_normalization_in_worker() {
        assert_eq!(valid_requested_username("  ").unwrap(), None);
        assert_eq!(
            valid_requested_username(" Mehmet.Ali ").unwrap(),
            Some("mehmet.ali".to_string())
        );
        assert!(valid_requested_username("a b").is_err());
        assert!(valid_requested_username(".ad").is_err());
        assert!(valid_requested_username(&"a".repeat(21)).is_err());
        let mut f = form();
        f.requested_username = "m.ali".to_string();
        assert_eq!(
            validate(&f).unwrap().requested_username.as_deref(),
            Some("m.ali")
        );
    }

    #[test]
    fn permanent_needs_no_end_date() {
        let mut f = form();
        f.employment_type = PERMANENT.to_string();
        f.end_date = String::new();
        assert!(validate(&f).unwrap().end_date.is_none());
    }

    #[test]
    fn account_hint_is_trimmed_and_shaped_like_sam_account_name() {
        assert_eq!(valid_account_hint("  ").unwrap(), None);
        assert_eq!(
            valid_account_hint(" mevcut.personel ").unwrap().as_deref(),
            Some("mevcut.personel")
        );
        assert!(valid_account_hint("ayşe yılmaz").is_err());
        assert!(valid_account_hint("a*b").is_err());
        assert!(valid_account_hint("abcdefghijklmnopqrstu").is_err());
    }

    // ADR-050/091: fren is satirina makine okunur neden yazar; ekran operatorun
    // dilinde "bekleme" gosterir, hata degil (F-12).
    #[test]
    fn throttle_reason_is_shown_as_a_wait_in_the_operator_language() {
        let tr = throttle_reason(Lang::Tr, "throttle:grant:50/50").expect("fren nedeni");
        assert!(tr.contains("verme") && tr.contains("50/50"), "{tr}");
        let en = throttle_reason(Lang::En, "throttle:destructive:12/10").expect("fren nedeni");
        assert!(en.contains("destructive") && en.contains("12/10"), "{en}");
        assert_eq!(throttle_reason(Lang::Tr, "hesap açılamadı: LDAP"), None);
        assert_eq!(throttle_reason(Lang::Tr, "throttle:bozuk"), None);
    }

    #[test]
    fn split_error_follows_reason_detail_contract() {
        assert_eq!(
            split_error("hesap bağlantısı yazılamadı: db timeout"),
            (
                "hesap bağlantısı yazılamadı".to_string(),
                "db timeout".to_string()
            )
        );
        assert_eq!(
            split_error("müdahale gerekiyor: ad çakışıyor"),
            ("ad çakışıyor".to_string(), String::new())
        );
    }

    #[test]
    fn diff_text_compares_desired_with_applied() {
        use LifecycleState::*;
        let tr = Lang::Tr;
        assert_eq!(diff_text(tr, Active, false, None), "hesap yok");
        assert_eq!(diff_text(tr, Active, true, Some("active")), "uyumlu");
        assert_eq!(
            diff_text(tr, Active, true, Some("pending")),
            "hedefte bekliyor, olması gereken aktif"
        );
        assert!(diff_text(tr, Departed, true, None).contains("henüz uygulanmadı"));
        // Dil degisince ayni fark Ingilizce (ADR-089)
        assert_eq!(
            diff_text(Lang::En, Active, true, Some("pending")),
            "target has pending, expected active"
        );
    }

    /// ADR-117 E: siralama anahtari izinli listeden gecer; uydurma deger
    /// varsayilana duser ve kullanicinin metni SQL'e hic girmez.
    #[test]
    fn the_sort_clause_comes_only_from_the_allow_list() {
        let default = sort_clause(None, false);
        assert_eq!(default, SORTS[0].1);
        assert_eq!(sort_clause(Some("full_name"), true), SORTS[0].2);
        assert_eq!(sort_clause(Some("department"), false), SORTS[3].1);
        for bad in ["", "id; DROP TABLE identities", "i.national_id", "SURNAME"] {
            assert_eq!(sort_clause(Some(bad), false), default, "{bad}");
            assert_eq!(sort_clause(Some(bad), true), SORTS[0].2, "{bad}");
        }
        // Her anahtarin iki yonu de tanimli ve birbirinden farkli
        for (key, asc, desc) in SORTS {
            assert!(!asc.is_empty() && !desc.is_empty(), "{key}");
            assert_ne!(asc, desc, "{key}");
        }
    }

    /// ADR-117: pencere adi izinli listeden gelir; uydurma deger filtre acmaz.
    #[test]
    fn the_window_filter_accepts_only_listed_names() {
        assert_eq!(window_filter(Some("joined")), Some("joined"));
        assert_eq!(window_filter(Some("changed")), Some("changed"));
        assert_eq!(window_filter(None), None);
        for bad in ["", "JOINED", "1; DROP TABLE identities", "deleted"] {
            assert_eq!(window_filter(Some(bad)), None, "{bad}");
        }
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_personnel_page_searches_paginates_and_reports_the_total() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let tz = "Europe/Istanbul";
        async fn listed(
            pool: &PgPool,
            tz: &str,
            q: &str,
            offset: i64,
            limit: i64,
        ) -> (Vec<Listed>, i64) {
            windowed(pool, tz, q, None, offset, limit).await
        }
        async fn windowed(
            pool: &PgPool,
            tz: &str,
            q: &str,
            window: Option<&str>,
            offset: i64,
            limit: i64,
        ) -> (Vec<Listed>, i64) {
            let listing = Listing {
                query: q,
                unassigned_only: false,
                window,
                window_days: crate::dashboard::WINDOW_DAYS,
                department: None,
                role: None,
                order: sort_clause(None, false),
                offset,
                limit,
            };
            page(pool, tz, &listing).await.unwrap()
        }

        // Ilk sayfa: iki kisi, departman adi dolu, toplam dogru
        let (rows, total) = listed(&pool, tz, "", 0, 50).await;
        assert_eq!(total, 2);
        assert_eq!(rows.len(), 2);
        // Siralama soyada gore: Kaya, Yilmaz
        assert_eq!(rows[0].name, "Ali Kaya");
        assert_eq!(rows[0].department, "Test Birimi");
        assert_eq!(rows[0].username, "", "adi worker uretir, kayitta bos");
        assert_eq!(rows[0].state, "active");

        // Sayfa boyu: ikinci sayfa bir satir, toplam degismez
        let (first, total) = listed(&pool, tz, "", 0, 1).await;
        assert_eq!((first.len(), total), (1, 2));
        let (second, _) = listed(&pool, tz, "", 1, 1).await;
        assert_eq!(second[0].name, "Ayşe Yılmaz");

        // Arama hem satirlari hem toplami daraltir
        let (hit, total) = listed(&pool, tz, "yılmaz", 0, 50).await;
        assert_eq!((hit.len(), total), (1, 1));
        assert_eq!(hit[0].name, "Ayşe Yılmaz");

        // Joker karakter harf sayilir: `%` kimseyi getirmez
        let (none, total) = listed(&pool, tz, "%", 0, 50).await;
        assert!(none.is_empty());
        assert_eq!(total, 0);

        // ADR-117: panelin sayac kartlarindan gelen pencere filtreleri. Iki kisi
        // de bugun ise girdi; biri 40 gun once girip bugun ayrildi.
        sqlx::query(
            "UPDATE identities SET start_date = current_date - 40, \
                                   end_at = now() - interval '1 hour' \
             WHERE surname = 'Kaya'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (joined, total) = windowed(&pool, tz, "", Some("joined"), 0, 50).await;
        assert_eq!(
            (joined.len(), total),
            (1, 1),
            "pencerede yalnizca bugun giren"
        );
        assert_eq!(joined[0].name, "Ayşe Yılmaz");
        let (left, total) = windowed(&pool, tz, "", Some("departed"), 0, 50).await;
        assert_eq!((left.len(), total), (1, 1));
        assert_eq!(left[0].name, "Ali Kaya");
        // Degisiklik olayi olmayan kimse: liste bos, toplam sifir
        let (changed, total) = windowed(&pool, tz, "", Some("changed"), 0, 50).await;
        assert!(changed.is_empty());
        assert_eq!(total, 0);
        // Pencere arama kutusuyla birlikte calisir
        let (both, total) = windowed(&pool, tz, "yılmaz", Some("departed"), 0, 50).await;
        assert!(both.is_empty());
        assert_eq!(total, 0);

        // ADR-117 E: departman filtresi ve siralama. Iki kisi de ayni departmanda
        let dep: i64 = sqlx::query_scalar("SELECT id FROM departments LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        async fn filtered(
            pool: &PgPool,
            tz: &str,
            department: Option<i64>,
            order: &'static str,
        ) -> (Vec<Listed>, i64) {
            let listing = Listing {
                query: "",
                unassigned_only: false,
                window: None,
                window_days: crate::dashboard::WINDOW_DAYS,
                department,
                role: None,
                order,
                offset: 0,
                limit: 50,
            };
            page(pool, tz, &listing).await.unwrap()
        }
        let (rows, total) = filtered(&pool, tz, Some(dep), sort_clause(None, false)).await;
        assert_eq!((rows.len(), total), (2, 2), "ikisi de ayni departmanda");
        let (none, total) = filtered(&pool, tz, Some(dep + 10_000), sort_clause(None, false)).await;
        assert!(none.is_empty());
        assert_eq!(total, 0, "olmayan departman: sayac da sifir");
        // Azalan siralama: soyadi sondan basa
        let (desc, _) = filtered(&pool, tz, None, sort_clause(Some("full_name"), true)).await;
        assert_eq!(desc[0].name, "Ayşe Yılmaz");
        let (by_number, _) =
            filtered(&pool, tz, None, sort_clause(Some("employee_number"), false)).await;
        assert!(by_number[0].employee_number <= by_number[1].employee_number);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn create_opens_jobs_rejects_duplicate_national_id_and_loads_page() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        crate::test_support::seed_example_catalog(&pool).await;
        let options = form_options(&pool).await.unwrap();
        assert_eq!(options.managers.len(), 2);
        let mut f = form();
        f.department_id = options.departments[0].id.clone();
        f.primary_role_id = options.roles[0].id.clone();
        f.manager_id = ids[0].to_string();
        f.national_id = "10000000146".to_string();
        f.existing_ad_account_hint = " Mevcut.Personel ".to_string();
        let keys = Keys {
            aead: &[1u8; crate::crypto::KEY_LEN],
            blind_index: &[2u8; crate::crypto::KEY_LEN],
        };
        let tz = "Europe/Istanbul";

        let (id, first_password) = create(&pool, &keys, tz, &validate(&f).await_ok(), Some("ik"))
            .await
            .unwrap();
        let requested: Option<String> =
            sqlx::query_scalar("SELECT requested_by FROM first_passwords WHERE id = $1")
                .bind(first_password.expect("ilk parola isteği aynı transaction'da açılır"))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(requested.as_deref(), Some("ik"));
        let hint: Option<String> =
            sqlx::query_scalar("SELECT existing_ad_account_hint FROM identities WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            hint.as_deref(),
            Some("Mevcut.Personel"),
            "ipucu kırpılıp saklanır"
        );
        // Isler zaten kayitla acildi; ikinci cagri yenisini acmaz (ON CONFLICT)
        enqueue_all_targets(&pool, id, crate::jobs::Priority::Single)
            .await
            .unwrap();
        let end_at: String = sqlx::query_scalar(
            "SELECT to_char(end_at AT TIME ZONE 'Europe/Istanbul', 'YYYY-MM-DD HH24:MI') \
             FROM identities WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            end_at, "2027-01-01 00:00",
            "bitiş anı ertesi gün 00:00 (ADR-038)"
        );
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE identity_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 2, "her hedef için bir iş");

        f.given_name = "Başka".to_string();
        assert!(
            matches!(
                create(&pool, &keys, tz, &validate(&f).await_ok(), None).await,
                Err(CreateError::DuplicateNationalId)
            ),
            "aynı kimlik no ikinci kez kaydedilemez"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 3, "başarısız kayıt satır bırakmaz");
        // N-09: kayıt ve mükerrer reddi kimlik numarasını log satırına yazmaz
        assert_eq!(crate::log::printed_lines_containing("10000000146"), 0);
        assert!(similar_name_exists(&pool, "ayşe", "YILMAZ").await.unwrap());
        assert!(!similar_name_exists(&pool, "Yok", "Kimse").await.unwrap());

        let page = load_page(&pool, tz, keys.aead, id, Lang::Tr)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.person.national_id_masked, "10*******46");
        assert_eq!(page.state, "active");
        assert_eq!(page.person.manager, "Ayşe Yılmaz");
        assert_eq!(page.accounts.len(), 2);
        assert_eq!(page.accounts[0].diff, "hesap yok");
        assert_eq!(page.jobs.len(), 2);
        assert_eq!(page.jobs[0].status, "queued");
        assert_eq!(Lang::Tr.key("job", &page.jobs[0].status), "kuyrukta");
        assert!(load_page(&pool, tz, keys.aead, 999_999, Lang::Tr)
            .await
            .unwrap()
            .is_none());
        assert_eq!(recent(&pool, tz).await.unwrap().len(), 3);

        // ADR-022/042: ad istegi yalnizca ad olusmamisken; mudahale blogu isle birlikte.
        assert!(request_names(&pool, id, Some("ozel.ad"), true)
            .await
            .unwrap());
        sqlx::query("UPDATE jobs SET status = 'needs_intervention' WHERE identity_id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let page = load_page(&pool, tz, keys.aead, id, Lang::Tr)
            .await
            .unwrap()
            .unwrap();
        assert!(page.name_intervention);
        assert_eq!(page.person.requested_username, "ozel.ad");
        assert!(page.person.name_conflict_override);
        sqlx::query("UPDATE identities SET username = 'ozel.ad' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            !request_names(&pool, id, None, false).await.unwrap(),
            "ad oluştuktan sonra istek yazılmaz"
        );
        assert!(
            !load_page(&pool, tz, keys.aead, id, Lang::Tr)
                .await
                .unwrap()
                .unwrap()
                .name_intervention
        );

        // ADR-083: duzenleme formu mevcut degerlerle; update_mover tarihleri degistirmez;
        // ek rol: gecmis tarih reddi, upsert tarihi uzatir, kaldirma.
        let mut form = load_form(&pool, id).await.unwrap().unwrap();
        assert_eq!(form.given_name, "Ayşe");
        assert_eq!(form.manager_id, ids[0].to_string());
        assert_eq!(form.end_date, "2026-12-31");
        form.surname = "Demir".to_string();
        form.manager_id = id.to_string();
        update_mover(&pool, id, &validate(&form).await_ok())
            .await
            .unwrap();
        let (surname, manager, end): (String, Option<i64>, String) = sqlx::query_as(
            "SELECT surname, manager_id, to_char(end_at AT TIME ZONE 'Europe/Istanbul', 'YYYY-MM-DD HH24:MI') \
             FROM identities WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(surname, "Demir");
        assert_eq!(manager, None, "kendisi yönetici olamaz");
        assert_eq!(end, "2027-01-01 00:00", "tarihler bu yoldan değişmez");
        let additional: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name) VALUES ('additional', 'Nöbet') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!assign_role(&pool, tz, id, additional, Some("2020-01-01"))
            .await
            .unwrap());
        assert!(assign_role(&pool, tz, id, additional, None).await.unwrap());
        assert!(assign_role(&pool, tz, id, additional, Some("2099-12-31"))
            .await
            .unwrap());
        let assigned = load_additional_roles(&pool, id).await.unwrap();
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].ends_on, "2099-12-31");
        assert!(remove_role(&pool, id, additional).await.unwrap());
        assert!(!remove_role(&pool, id, additional).await.unwrap());

        // ADR-084 yasam dongusu: planli ayrilis, gecmis bitis → ayrildi, ileri tarih = geri alma,
        // acil, geri alma donus gunuyle, aski ve kaldirma, iptal (sahiplenilmis baglanti engeller).
        sqlx::query("UPDATE identities SET start_date = '2026-09-01' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            set_departure(&pool, tz, id, &dep("2026-08-01", None))
                .await
                .unwrap(),
            LifecycleChange::Rejected(_)
        ));
        // son calisma gunu gecmiste: bitis ani (ertesi gun 00:00) gecti → ayrildi
        assert_eq!(
            set_departure(&pool, tz, id, &dep("2026-09-30", Some(ids[0])))
                .await
                .unwrap(),
            LifecycleChange::Applied
        );
        assert_eq!(
            load_state(&pool, tz, id).await.unwrap(),
            Some(LifecycleState::Departed)
        );
        // ADR-111: neden ayrilis formuyla yazilir; gecersizi reddedilir, eskisi kalir
        let noted = Departure {
            note: " sözleşme bitti ",
            ..dep("2026-09-30", Some(ids[0]))
        };
        assert_eq!(
            set_departure(&pool, tz, id, &noted).await.unwrap(),
            LifecycleChange::Applied
        );
        assert_eq!(note_of(&pool, id).await.as_deref(), Some("sözleşme bitti"));
        let multi_line = Departure {
            note: "a\nb",
            ..dep("2026-09-30", None)
        };
        assert_eq!(
            set_departure(&pool, tz, id, &multi_line).await.unwrap(),
            LifecycleChange::Rejected("err.departure_note_invalid")
        );
        assert_eq!(note_of(&pool, id).await.as_deref(), Some("sözleşme bitti"));
        assert_eq!(
            set_departure(&pool, tz, id, &dep("2099-01-01", None))
                .await
                .unwrap(),
            LifecycleChange::Reverted,
            "ayrildidan cikis geri almadir (ADR-059)"
        );
        assert!(set_emergency_departure(&pool, id, Some(id)).await.is_ok());
        assert!(
            !set_emergency_departure(&pool, id, Some(id)).await.unwrap(),
            "devir yöneticisi kendisi olamaz"
        );
        assert!(set_emergency_departure(&pool, id, None).await.unwrap());
        let info = load_lifecycle(&pool, tz, id, LifecycleState::Departed)
            .await
            .unwrap();
        assert!(info.emergency && info.departed && info.can_cancel);
        // Kadrolu disi (sozlesmeli): bitis kaldirilamaz, ileri tarihli bitis = geri alma
        assert!(
            !revert_departure(&pool, id, Some("2099-05-05"))
                .await
                .unwrap(),
            "sözleşmelide bitiş kaldırılamaz"
        );
        assert_eq!(
            set_departure(&pool, tz, id, &dep("2099-05-05", None))
                .await
                .unwrap(),
            LifecycleChange::Reverted
        );
        assert_eq!(
            load_state(&pool, tz, id).await.unwrap(),
            Some(LifecycleState::Active)
        );
        assert!(!set_suspension(&pool, id, "2026-10-05", Some("2026-10-01"))
            .await
            .unwrap());
        assert!(set_suspension(&pool, id, "2026-10-05", Some("2026-10-15"))
            .await
            .unwrap());
        let info = load_lifecycle(&pool, tz, id, LifecycleState::Active)
            .await
            .unwrap();
        assert_eq!(
            (info.suspension_end.as_str(), info.return_day.as_str()),
            ("2026-10-15", "2026-10-16")
        );
        assert!(lift_suspension(&pool, id).await.unwrap());
        assert!(!lift_suspension(&pool, id).await.unwrap());
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             SELECT $1, id, 'adopted-1', 'adopted', 'observed' FROM target_systems WHERE kind = 'ad'",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            !cancel_registration(&pool, id).await.unwrap(),
            "sahiplenilmiş hesap iptal edilemez"
        );
        // Kadrolu (Ali): iptal → ayrildi; geri alma bitisi kaldirir (donus gunu ileride → bekliyor)
        assert!(cancel_registration(&pool, ids[1]).await.unwrap());
        assert_eq!(
            load_state(&pool, tz, ids[1]).await.unwrap(),
            Some(LifecycleState::Departed)
        );
        sqlx::query("UPDATE identities SET departure_note = 'istifa' WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        assert!(revert_departure(&pool, ids[1], Some("2099-05-05"))
            .await
            .unwrap());
        assert_eq!(note_of(&pool, ids[1]).await, None, "geri alma nedeni siler");
        assert_eq!(
            load_state(&pool, tz, ids[1]).await.unwrap(),
            Some(LifecycleState::Pending)
        );
        assert!(
            !revert_departure(&pool, ids[1], None).await.unwrap(),
            "geri alınacak bitiş yok"
        );

        // ADR-041/F-38: yonetici (Ayşe, ids[0]) ayrilir, devir Ali (ids[1]); astin sayfasinda not,
        // yoneticinin sayfasinda ast sayisi; devir yoksa astlar yoneticisiz uyarisi.
        sqlx::query("UPDATE identities SET manager_id = $1 WHERE id = $2")
            .bind(ids[0])
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE identities SET start_date = '2026-01-01' WHERE id = $1")
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            set_departure(&pool, tz, ids[0], &dep("2026-09-30", Some(ids[1])))
                .await
                .unwrap(),
            LifecycleChange::Applied
        );
        let info = load_lifecycle(&pool, tz, id, LifecycleState::Active)
            .await
            .unwrap();
        assert!(info.manager_departed);
        assert_eq!(info.handover_name, "Ali Kaya");
        let boss = load_lifecycle(&pool, tz, ids[0], LifecycleState::Departed)
            .await
            .unwrap();
        assert_eq!(boss.subordinates, 1);
        assert!(!boss.orphaned_subordinates);
        sqlx::query("UPDATE identities SET handover_manager_id = NULL WHERE id = $1")
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        let boss = load_lifecycle(&pool, tz, ids[0], LifecycleState::Departed)
            .await
            .unwrap();
        assert!(boss.orphaned_subordinates);
        let info = load_lifecycle(&pool, tz, id, LifecycleState::Active)
            .await
            .unwrap();
        assert!(info.manager_departed);
        assert!(info.handover_name.is_empty());

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    trait AwaitOk {
        fn await_ok(self) -> NewIdentity;
    }
    impl AwaitOk for Result<NewIdentity, &'static str> {
        fn await_ok(self) -> NewIdentity {
            self.expect("form geçerli olmalı")
        }
    }
}
