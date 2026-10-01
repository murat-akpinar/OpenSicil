// --- START FEATURE: identity-registration ---
// Kimlik kayit formu ve kisi sayfasi verisi (docs/03 kimlik semasi, F-12, ADR-078).
// Dogrulama saf; kayit tek transaction; durum desired_state ile turetilir,
// saat dilimi cevirisi Postgres'te (operator reddiyle ayni yol).

use serde::Deserialize;
use sqlx::PgPool;

use crate::desired_state::{lifecycle_state, Date, LifecycleState};
use crate::national_id::{self, Keys, NationalId};
use crate::operator_guard::{timeline_from_row, TimelineRow};
use crate::timeline_sql;

pub const EMPLOYMENT_TYPES: [(&str, &str); 4] = [
    ("permanent", "Kadrolu"),
    ("contract", "Sözleşmeli"),
    ("intern", "Stajyer"),
    ("outsourced", "Dış kaynak"),
];
const PERMANENT: &str = "permanent";
const E164_MIN_DIGITS: usize = 8;
const E164_MAX_DIGITS: usize = 15;
const INTERVENTION_PREFIX: &str = "müdahale gerekiyor: ";
const RECENT_LIMIT: i64 = 50;
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
}

// Sekil kontrolu burada; ADR-011 normallestirmesi worker'da (validate_manual).
pub fn valid_requested_username(raw: &str) -> Result<Option<String>, String> {
    const MAX_LEN: usize = 20;
    let name = raw.trim().to_lowercase();
    if name.is_empty() {
        return Ok(None);
    }
    let shape_ok = name.chars().count() <= MAX_LEN
        && name.chars().all(|c| c.is_alphanumeric() || c == '.')
        && !name.starts_with('.')
        && !name.ends_with('.');
    shape_ok
        .then_some(Some(name))
        .ok_or_else(|| format!("Kullanıcı adı en fazla {MAX_LEN} karakter, harf/rakam/nokta"))
}

pub fn validate(f: &IdentityForm) -> Result<NewIdentity, String> {
    let given_name = required(&f.given_name, "Ad")?;
    let surname = required(&f.surname, "Soyad")?;
    let national_id = optional(&f.national_id)
        .map(|raw| national_id::parse(&f.national_id_country, &raw))
        .transpose()?;
    let mobile_phone = optional(&f.mobile_phone).map(valid_e164).transpose()?;
    let department_id = parse_id(&f.department_id, "Departman")?;
    let primary_role_id = parse_id(&f.primary_role_id, "Birincil rol")?;
    let manager_id = optional(&f.manager_id)
        .map(|m| parse_id(&m, "Yönetici"))
        .transpose()?;
    if !EMPLOYMENT_TYPES
        .iter()
        .any(|(k, _)| *k == f.employment_type)
    {
        return Err("Çalışma tipi seçilmeli".to_string());
    }
    let start = valid_date(&f.start_date, "Başlangıç tarihi")?;
    let end = optional(&f.end_date)
        .map(|d| valid_date(&d, "Bitiş tarihi"))
        .transpose()?;
    if f.employment_type != PERMANENT && end.is_none() {
        return Err("Kadrolu dışında bitiş tarihi zorunlu".to_string());
    }
    if end.is_some_and(|e| e < start) {
        return Err("Bitiş tarihi başlangıçtan önce olamaz".to_string());
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
    })
}

fn required(value: &str, label: &str) -> Result<String, String> {
    optional(value).ok_or_else(|| format!("{label} boş olamaz"))
}

fn optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_id(value: &str, label: &str) -> Result<i64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{label} seçilmeli"))
}

fn valid_date(value: &str, label: &str) -> Result<Date, String> {
    Date::from_iso(value.trim()).ok_or_else(|| format!("{label} YYYY-AA-GG biçiminde olmalı"))
}

// E.164: '+' ve 8–15 rakam, ilk rakam 0 degil (docs/03 cep telefonu).
fn valid_e164(value: String) -> Result<String, String> {
    let digits = value.strip_prefix('+').unwrap_or("");
    let ok = (E164_MIN_DIGITS..=E164_MAX_DIGITS).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.starts_with('0');
    ok.then_some(value)
        .ok_or_else(|| "Cep telefonu +905321234567 biçiminde olmalı".to_string())
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
pub async fn create(
    pool: &PgPool,
    keys: &Keys<'_>,
    time_zone: &str,
    new: &NewIdentity,
) -> Result<i64, CreateError> {
    // On kontrol operatore erken ve net cevap verir; yaris durumunda UNIQUE indeks yakalar.
    if let Some(nid) = &new.national_id {
        if national_id::find_identity(pool, keys.blind_index, nid)
            .await?
            .is_some()
        {
            return Err(CreateError::DuplicateNationalId);
        }
    }
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO identities (given_name, surname, employee_number, mobile_phone, \
         department_id, primary_role_id, manager_id, employment_type, start_date, end_at, \
         requested_username) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::date, \
         (($10::date + 1)::timestamp AT TIME ZONE $11), $12) RETURNING id",
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
    .fetch_one(&mut *tx)
    .await?;
    if let Some(nid) = &new.national_id {
        national_id::store(&mut *tx, keys, id, nid)
            .await
            .map_err(|e| match e.as_database_error() {
                Some(db) if db.is_unique_violation() => CreateError::DuplicateNationalId,
                _ => CreateError::Db(e),
            })?;
    }
    tx.commit().await?;
    Ok(id)
}

// Kimlik no yokken ad-soyad esleşmesi uyaridir, engel degil (docs/03).
// Katlama DB yerel ayarindan bagimsiz: Turkce buyuk harfler (I/İ dahil) once
// translate ile kucuge indirilir, lower() kalan ASCII'yi halleder.
pub async fn similar_name_exists(
    pool: &PgPool,
    given_name: &str,
    surname: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM identities WHERE deleted_at IS NULL \
         AND lower(translate(given_name, 'IİıŞĞÜÖÇ', 'iiişğüöç')) \
           = lower(translate($1, 'IİıŞĞÜÖÇ', 'iiişğüöç')) \
         AND lower(translate(surname, 'IİıŞĞÜÖÇ', 'iiişğüöç')) \
           = lower(translate($2, 'IİıŞĞÜÖÇ', 'iiişğüöç')))",
    )
    .bind(given_name)
    .bind(surname)
    .fetch_one(pool)
    .await
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

pub fn state_label(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Pending => "bekliyor",
        LifecycleState::Active => "aktif",
        LifecycleState::Suspended => "askıda",
        LifecycleState::Departed => "ayrıldı",
        LifecycleState::Deleted => "silindi",
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

fn label_of_key(key: &str) -> &str {
    [
        LifecycleState::Pending,
        LifecycleState::Active,
        LifecycleState::Suspended,
        LifecycleState::Departed,
        LifecycleState::Deleted,
    ]
    .into_iter()
    .find(|s| state_key(*s) == key)
    .map(state_label)
    .unwrap_or(key)
}

pub async fn load_state(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
) -> Result<Option<LifecycleState>, sqlx::Error> {
    let row: Option<TimelineRow> = sqlx::query_as(timeline_sql!("id = $1"))
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
    pub employee_number: String,
    pub state: &'static str,
}

// ponytail: kimlik basina bir durum sorgusu, liste 50 ile sinirli; arama
// ekrani (3b) tek sorguya alir.
pub async fn recent(pool: &PgPool, time_zone: &str) -> Result<Vec<Listed>, sqlx::Error> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, given_name || ' ' || surname, employee_number FROM identities \
         WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC LIMIT $1",
    )
    .bind(RECENT_LIMIT)
    .fetch_all(pool)
    .await?;
    let mut listed = Vec::with_capacity(rows.len());
    for (id, name, employee_number) in rows {
        let state = load_state(pool, time_zone, id).await?;
        listed.push(Listed {
            id,
            name,
            employee_number: employee_number.unwrap_or_default(),
            state: state.map(state_label).unwrap_or(""),
        });
    }
    Ok(listed)
}

pub struct Person {
    pub id: i64,
    pub name: String,
    pub employee_number: String,
    pub mobile_phone: String,
    pub national_id_masked: String,
    pub department: String,
    pub role: String,
    pub manager: String,
    pub employment_type: String,
    pub start_date: String,
    pub end_at: String,
    pub username: String,
    pub email: String,
    pub upn: String,
    pub requested_username: String,
    pub name_conflict_override: bool,
    pub state: &'static str,
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
    pub target: String,
    pub external_id: String,
    pub origin: String,
    pub mode: String,
    pub applied_state: String,
    pub diff: String,
}

pub struct Job {
    pub id: i64,
    pub target: String,
    pub status: &'static str,
    pub attempts: i32,
    pub next_attempt_at: String,
    pub summary: String,
    pub detail: String,
    pub result: String,
    pub retryable: bool,
}

pub struct Event {
    pub at: String,
    pub event_type: String,
    pub outcome: String,
    pub actor: String,
}

pub struct PersonPage {
    pub person: Person,
    pub accounts: Vec<Account>,
    pub jobs: Vec<Job>,
    pub events: Vec<Event>,
    /// Ad henuz yok ve bir is mudahalede: ADR-022/042 secenekleri gosterilir
    pub name_intervention: bool,
    pub additional_roles: Vec<AssignedRole>,
    pub role_options: Vec<Choice>,
}

pub async fn load_page(
    pool: &PgPool,
    time_zone: &str,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
) -> Result<Option<PersonPage>, sqlx::Error> {
    let Some(state) = load_state(pool, time_zone, id).await? else {
        return Ok(None);
    };
    let person = load_person(pool, time_zone, aead_key, id, state).await?;
    let jobs = load_jobs(pool, time_zone, id).await?;
    let name_intervention =
        person.username.is_empty() && jobs.iter().any(|j| j.status == INTERVENTION_LABEL);
    Ok(Some(PersonPage {
        accounts: load_accounts(pool, id, state).await?,
        events: load_events(pool, time_zone, id).await?,
        additional_roles: load_additional_roles(pool, id).await?,
        role_options: choices(
            pool,
            "SELECT id, name FROM roles WHERE kind = 'additional' ORDER BY name",
        )
        .await?,
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
}

pub async fn load_additional_roles(
    pool: &PgPool,
    id: i64,
) -> Result<Vec<AssignedRole>, sqlx::Error> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT a.role_id, r.name, to_char(a.ends_on, 'YYYY-MM-DD') \
         FROM identity_additional_roles a JOIN roles r ON r.id = a.role_id \
         WHERE a.identity_id = $1 ORDER BY r.name",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(role_id, name, ends_on)| AssignedRole {
            role_id,
            name,
            ends_on: ends_on.unwrap_or_default(),
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

type PersonRow = (
    String,
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
);

async fn load_person(
    pool: &PgPool,
    time_zone: &str,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
    state: LifecycleState,
) -> Result<Person, sqlx::Error> {
    let r: PersonRow = sqlx::query_as(
        "SELECT i.given_name, i.surname, i.employee_number, i.mobile_phone, i.national_id_enc, \
         d.name, r.name, m.given_name || ' ' || m.surname, i.employment_type, \
         to_char(i.start_date, 'YYYY-MM-DD'), \
         to_char(i.end_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), i.username, i.email, i.upn, \
         i.requested_username, i.name_conflict_override \
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
        name: format!("{} {}", r.0, r.1),
        employee_number: r.2.unwrap_or_default(),
        mobile_phone: r.3.unwrap_or_default(),
        national_id_masked: r
            .4
            .as_deref()
            .map(|enc| masked(aead_key, enc))
            .unwrap_or_default(),
        department: r.5,
        role: r.6,
        manager: r.7.unwrap_or_default(),
        employment_type: EMPLOYMENT_TYPES
            .iter()
            .find(|(k, _)| *k == r.8)
            .map(|(_, l)| l.to_string())
            .unwrap_or(r.8),
        start_date: r.9,
        end_at: r.10.unwrap_or_default(),
        username: r.11.unwrap_or_default(),
        email: r.12.unwrap_or_default(),
        upn: r.13.unwrap_or_default(),
        requested_username: r.14.unwrap_or_default(),
        name_conflict_override: r.15,
        state: state_label(state),
    })
}

// Acik goruntuleme ayri yetki ve denetim kaydi ister (docs/07); burada yalnizca maske.
fn masked(aead_key: &[u8; crate::crypto::KEY_LEN], enc: &[u8]) -> String {
    match national_id::decrypt(aead_key, enc) {
        Ok(value) => national_id::mask(&value),
        Err(e) => {
            eprintln!("identity: kimlik numarası çözülemedi: {e}");
            "çözülemedi".to_string()
        }
    }
}

// Hedefteki fark (3a kapsami, ADR-078): turetilen durum ↔ applied_state.
async fn load_accounts(
    pool: &PgPool,
    id: i64,
    state: LifecycleState,
) -> Result<Vec<Account>, sqlx::Error> {
    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.name, l.external_id, l.origin, l.mode, l.applied_state FROM target_systems t \
         LEFT JOIN account_links l ON l.target_system_id = t.id AND l.identity_id = $1 \
         ORDER BY t.id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(target, external_id, origin, mode, applied)| Account {
            target,
            diff: diff_text(state, external_id.is_some(), applied.as_deref()),
            external_id: external_id.unwrap_or_default(),
            origin: origin.as_deref().map(link_label).unwrap_or_default(),
            mode: mode.as_deref().map(link_label).unwrap_or_default(),
            applied_state: applied
                .as_deref()
                .map(label_of_key)
                .unwrap_or("")
                .to_string(),
        })
        .collect())
}

// account_links koken/mod anahtarlarinin ekran karsiligi (docs/03 hesap baglantisi).
pub fn link_label(key: &str) -> String {
    match key {
        "provisioned" => "açıldı",
        "adopted" => "sahiplenildi",
        "managed" => "yönetiliyor",
        "observed" => "gözlem",
        other => other,
    }
    .to_string()
}

pub fn diff_text(state: LifecycleState, linked: bool, applied: Option<&str>) -> String {
    let desired = state_key(state);
    match applied {
        _ if !linked => "hesap yok".to_string(),
        Some(a) if a == desired => "uyumlu".to_string(),
        Some(a) => format!(
            "hedefte {}, olması gereken {}",
            label_of_key(a),
            state_label(state)
        ),
        None => format!("henüz uygulanmadı, olması gereken {}", state_label(state)),
    }
}

const INTERVENTION_LABEL: &str = "müdahale gerekiyor";

fn status_label(status: &str) -> &'static str {
    match status {
        "queued" => "kuyrukta",
        "running" => "çalışıyor",
        "succeeded" => "tamamlandı",
        "needs_intervention" => INTERVENTION_LABEL,
        _ => "bilinmiyor",
    }
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

async fn load_jobs(pool: &PgPool, time_zone: &str, id: i64) -> Result<Vec<Job>, sqlx::Error> {
    type Row = (
        i64,
        String,
        String,
        i32,
        String,
        Option<String>,
        Option<String>,
        bool,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT j.id, t.name, j.status, j.attempts, \
         to_char(j.next_attempt_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI'), \
         j.last_error, j.result, j.retry_requested FROM jobs j \
         JOIN target_systems t ON t.id = j.target_system_id \
         WHERE j.identity_id = $1 ORDER BY j.created_at DESC, j.id DESC",
    )
    .bind(id)
    .bind(time_zone)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, target, status, attempts, next, error, result, retry)| {
                let (summary, detail) = error.as_deref().map(split_error).unwrap_or_default();
                Job {
                    id,
                    target,
                    status: status_label(&status),
                    attempts,
                    next_attempt_at: next,
                    summary,
                    detail,
                    result: result.unwrap_or_default(),
                    retryable: status == "needs_intervention" && !retry,
                }
            },
        )
        .collect())
}

async fn load_events(pool: &PgPool, time_zone: &str, id: i64) -> Result<Vec<Event>, sqlx::Error> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT to_char(occurred_at AT TIME ZONE $2, 'YYYY-MM-DD HH24:MI:SS'), event_type, \
         coalesce(outcome, ''), coalesce(actor_username, performed_by::text) FROM audit_log \
         WHERE identity_id = $1 ORDER BY occurred_at DESC, id DESC LIMIT $3",
    )
    .bind(id)
    .bind(time_zone)
    .bind(EVENT_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(at, event_type, outcome, actor)| Event {
            at,
            event_type,
            outcome,
            actor,
        })
        .collect())
}
// --- END FEATURE: identity-registration ---

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn validate_rejects_bad_input_with_operator_messages() {
        let check = |mutate: fn(&mut IdentityForm), expected: &str| {
            let mut f = form();
            mutate(&mut f);
            let err = validate(&f).unwrap_err();
            assert!(err.contains(expected), "{err}");
        };
        check(|f| f.given_name = "  ".to_string(), "Ad boş");
        check(|f| f.department_id = String::new(), "Departman");
        check(|f| f.employment_type = "x".to_string(), "Çalışma tipi");
        check(|f| f.start_date = "01.10.2026".to_string(), "Başlangıç");
        check(|f| f.end_date = String::new(), "Kadrolu dışında");
        check(
            |f| f.end_date = "2026-09-01".to_string(),
            "başlangıçtan önce",
        );
        check(
            |f| f.mobile_phone = "05321234567".to_string(),
            "Cep telefonu",
        );
        check(|f| f.mobile_phone = "+0532".to_string(), "Cep telefonu");
        check(
            |f| f.national_id = "10000000147".to_string(),
            "kontrol hanesi",
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
        assert_eq!(diff_text(Active, false, None), "hesap yok");
        assert_eq!(diff_text(Active, true, Some("active")), "uyumlu");
        assert_eq!(
            diff_text(Active, true, Some("pending")),
            "hedefte bekliyor, olması gereken aktif"
        );
        assert!(diff_text(Departed, true, None).contains("henüz uygulanmadı"));
        assert_eq!(link_label("provisioned"), "açıldı");
        assert_eq!(link_label("x"), "x");
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
        let keys = Keys {
            aead: &[1u8; crate::crypto::KEY_LEN],
            blind_index: &[2u8; crate::crypto::KEY_LEN],
        };
        let tz = "Europe/Istanbul";

        let id = create(&pool, &keys, tz, &validate(&f).await_ok())
            .await
            .unwrap();
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
                create(&pool, &keys, tz, &validate(&f).await_ok()).await,
                Err(CreateError::DuplicateNationalId)
            ),
            "aynı kimlik no ikinci kez kaydedilemez"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 3, "başarısız kayıt satır bırakmaz");
        assert!(similar_name_exists(&pool, "ayşe", "YILMAZ").await.unwrap());
        assert!(!similar_name_exists(&pool, "Yok", "Kimse").await.unwrap());

        let page = load_page(&pool, tz, keys.aead, id).await.unwrap().unwrap();
        assert_eq!(page.person.national_id_masked, "10*******46");
        assert_eq!(page.person.state, "aktif");
        assert_eq!(page.person.manager, "Ayşe Yılmaz");
        assert_eq!(page.accounts.len(), 2);
        assert_eq!(page.accounts[0].diff, "hesap yok");
        assert_eq!(page.jobs.len(), 2);
        assert_eq!(page.jobs[0].status, "kuyrukta");
        assert!(load_page(&pool, tz, keys.aead, 999_999)
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
        let page = load_page(&pool, tz, keys.aead, id).await.unwrap().unwrap();
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
            !load_page(&pool, tz, keys.aead, id)
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

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    trait AwaitOk {
        fn await_ok(self) -> NewIdentity;
    }
    impl AwaitOk for Result<NewIdentity, String> {
        fn await_ok(self) -> NewIdentity {
            self.expect("form geçerli olmalı")
        }
    }
}
