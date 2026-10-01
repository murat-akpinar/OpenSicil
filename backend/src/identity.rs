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
const INTERVENTION_STATUS: &str = "needs_intervention";
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
fn valid_e164(value: String) -> Result<String, &'static str> {
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
    let id: i64 = sqlx::query_scalar(
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
    let first_password = open_jobs(&mut tx, id, first_password_by).await?;
    tx.commit().await?;
    Ok((id, first_password))
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
    pub state_kind: &'static str,
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
            state: state.map(state_key).unwrap_or(""),
            state_kind: state.map(state_kind).unwrap_or("muted"),
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
    );
    let r: Row = sqlx::query_as(
        "SELECT to_char((end_at AT TIME ZONE $2) - interval '1 day', 'YYYY-MM-DD'), \
         handover_manager_id, to_char(suspension_start, 'YYYY-MM-DD'), \
         to_char(suspension_end, 'YYYY-MM-DD'), to_char(suspension_end + 1, 'YYYY-MM-DD'), \
         cancelled, emergency_departure, \
         NOT EXISTS (SELECT 1 FROM account_links l WHERE l.identity_id = i.id AND l.origin = 'adopted') \
         FROM identities i WHERE id = $1",
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

// Planli ayrilis: son calisma gunu → ertesi gun 00:00 (ADR-038). Kimlik `ayrildi`
// iken ileri tarih = geri alma.
pub async fn set_departure(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
    end_date: &str,
    handover: Option<i64>,
) -> Result<LifecycleChange, sqlx::Error> {
    if Date::from_iso(end_date).is_none() {
        return Ok(LifecycleChange::Rejected("err.end_date_format"));
    }
    let before = load_state(pool, time_zone, id).await?;
    let done = sqlx::query(
        "UPDATE identities SET end_at = (($2::date + 1)::timestamp AT TIME ZONE $3), \
         handover_manager_id = $4, emergency_departure = FALSE, cancelled = FALSE \
         WHERE id = $1 AND deleted_at IS NULL AND $2::date >= start_date \
         AND ($4::bigint IS NULL OR $4 <> $1)",
    )
    .bind(id)
    .bind(end_date)
    .bind(time_zone)
    .bind(handover)
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
         start_date = COALESCE($2::date, start_date) \
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
    let jobs = load_jobs(pool, time_zone, id).await?;
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
    lang: Lang,
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
            .map(|enc| masked(aead_key, enc, lang))
            .unwrap_or_default(),
        department: r.5,
        role: r.6,
        manager: r.7.unwrap_or_default(),
        employment_type: r.8,
        start_date: r.9,
        end_at: r.10.unwrap_or_default(),
        username: r.11.unwrap_or_default(),
        email: r.12.unwrap_or_default(),
        upn: r.13.unwrap_or_default(),
        requested_username: r.14.unwrap_or_default(),
        name_conflict_override: r.15,
    })
}

// Acik goruntuleme ayri yetki ve denetim kaydi ister (docs/07); burada yalnizca maske.
fn masked(aead_key: &[u8; crate::crypto::KEY_LEN], enc: &[u8], lang: Lang) -> String {
    match national_id::decrypt(aead_key, enc) {
        Ok(value) => national_id::mask(&value),
        Err(e) => {
            eprintln!("identity: kimlik numarası çözülemedi: {e}");
            lang.t("person.undecryptable").to_string()
        }
    }
}

// Hedefteki fark (3a kapsami, ADR-078): turetilen durum ↔ applied_state. Gozlem
// modunda farki motor hesaplar (ADR-018/087); son is sonucu oldugu gibi gosterilir.
async fn load_accounts(
    pool: &PgPool,
    id: i64,
    state: LifecycleState,
    lang: Lang,
) -> Result<Vec<Account>, sqlx::Error> {
    type Row = (
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT t.id, t.name, l.external_id, l.origin, l.mode, l.applied_state, \
         l.manage_requested_at IS NOT NULL, \
         (SELECT j.result FROM jobs j WHERE j.identity_id = $1 AND j.target_system_id = t.id \
         AND j.result IS NOT NULL ORDER BY j.finished_at DESC NULLS LAST, j.id DESC LIMIT 1) \
         FROM target_systems t \
         LEFT JOIN account_links l ON l.target_system_id = t.id AND l.identity_id = $1 \
         ORDER BY t.id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(target_id, target, external_id, origin, mode, applied, requested, result)| {
                let observed = mode.as_deref() == Some(OBSERVED_MODE);
                Account {
                    target_id,
                    target,
                    diff: match observed {
                        true => {
                            result.unwrap_or_else(|| lang.t("diff.observed_pending").to_string())
                        }
                        false => diff_text(lang, state, external_id.is_some(), applied.as_deref()),
                    },
                    external_id: external_id.unwrap_or_default(),
                    origin: origin.unwrap_or_default(),
                    mode: mode.unwrap_or_default(),
                    applied_state: applied.unwrap_or_default(),
                    observed,
                    manage_requested: requested,
                }
            },
        )
        .collect())
}

const OBSERVED_MODE: &str = "observed";

// ADR-018/087: operator farki gorup onaylar; backend yalnizca istegi yazar, modu
// worker cevirir. Yonetilen ya da silinmis baglantida islem yok (false).
pub async fn request_management(
    pool: &PgPool,
    id: i64,
    target_system_id: i64,
) -> Result<bool, sqlx::Error> {
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
    Ok(done.rows_affected() == 1)
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

pub(crate) async fn load_jobs(
    pool: &PgPool,
    time_zone: &str,
    id: i64,
) -> Result<Vec<Job>, sqlx::Error> {
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
                    status_kind: status_kind(&status),
                    attempts,
                    next_attempt_at: next,
                    summary,
                    detail,
                    result: result.unwrap_or_default(),
                    retryable: status == INTERVENTION_STATUS && !retry,
                    status,
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
            set_departure(&pool, tz, id, "2026-08-01", None)
                .await
                .unwrap(),
            LifecycleChange::Rejected(_)
        ));
        // son calisma gunu gecmiste: bitis ani (ertesi gun 00:00) gecti → ayrildi
        assert_eq!(
            set_departure(&pool, tz, id, "2026-09-30", Some(ids[0]))
                .await
                .unwrap(),
            LifecycleChange::Applied
        );
        assert_eq!(
            load_state(&pool, tz, id).await.unwrap(),
            Some(LifecycleState::Departed)
        );
        assert_eq!(
            set_departure(&pool, tz, id, "2099-01-01", None)
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
            set_departure(&pool, tz, id, "2099-05-05", None)
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
        assert!(revert_departure(&pool, ids[1], Some("2099-05-05"))
            .await
            .unwrap());
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
            set_departure(&pool, tz, ids[0], "2026-09-30", Some(ids[1]))
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
