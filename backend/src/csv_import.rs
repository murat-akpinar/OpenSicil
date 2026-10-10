// --- START FEATURE: csv-import ---
// CSV ile toplu kimlik ice aktarma (F-17; ADR-018, 023, 030, 042, 055).
// Dosya saklanmaz: tarayici dosyayi okuyup metin alanina koyar, sunucuya siradan
// form gelir (multipart yok). Metin RFC 4180 kurallariyla cozulur, satirlar
// dogrulanir (tek hatali satir dosyayi dusurur), etki onizlenir ve ya tek
// transaction'da uygulanir ya da degisiklik seti esigini asiyorsa sifreli
// satirlarla Sistem yoneticisinin onayina birakilir; onay aninda plan yeniden
// hesaplanir (ADR-055 madde 1). Dosyada olmayan kimlige dokunulmaz; basligi
// olmayan kolon dokunmaz, bos hucre istege bagli alani temizler (ADR-023).

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::extract::{DefaultBodyLimit, Form, Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};

use crate::i18n::Lang;
use crate::identity::{self, IdentityForm, NewIdentity};
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::national_id::{self, Keys, NationalId};
use crate::operator_session::Operator;
use crate::org_web::Notice;
use crate::shell::Shell;
use crate::web::{render, AppState};

/// Yukleme ve uygulama (ADR-018: IK operatoru); onay yalnizca Sistem yoneticisi (ADR-026).
pub const AUTHORITIES: &[&str] = &["hr", "admin"];
pub const APPROVE_AUTHORITIES: &[&str] = &["admin"];
/// N-03 olcegi (20.000) + pay; daha buyuk dosya parcalanir.
pub const MAX_ROWS: usize = 25_000;
/// Form govdesi: 25.000 satir URL kodlamasiyla birkac MB (nginx siniri 10 MB).
const BODY_LIMIT: usize = 16 * 1024 * 1024;
const PREVIEW_ROWS: usize = 50;
const ROLE_SEPARATORS: [char; 2] = [';', '|'];

/// Baslik ve secenek karsilastirmasi: Turkce harfler ASCII'ye, kucuk harf;
/// '_', '-' ve ardisik bosluklar tek bosluk.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.trim().chars() {
        let mapped = match c {
            'ç' | 'Ç' => 'c',
            'ğ' | 'Ğ' => 'g',
            'ı' | 'I' | 'İ' => 'i',
            'ö' | 'Ö' => 'o',
            'ş' | 'Ş' => 's',
            'ü' | 'Ü' => 'u',
            '_' | '-' | ' ' | '\t' => ' ',
            other => other.to_ascii_lowercase(),
        };
        if mapped == ' ' {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(mapped);
            space = false;
        }
    }
    out.trim_end().to_string()
}

/// Dosya kolonlari: kimlik semasinin alanlari (docs/03) + departman kodu, rol adlari,
/// yoneticinin sicil nosu ve mevcut hesap ipucu (ADR-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Column {
    EmployeeNumber,
    GivenName,
    Surname,
    NationalId,
    NationalIdCountry,
    MobilePhone,
    DepartmentCode,
    PrimaryRole,
    AdditionalRoles,
    ManagerEmployeeNumber,
    EmploymentType,
    StartDate,
    EndDate,
    AdAccountHint,
}

impl Column {
    pub const ALL: [Column; 14] = [
        Column::EmployeeNumber,
        Column::GivenName,
        Column::Surname,
        Column::NationalId,
        Column::NationalIdCountry,
        Column::MobilePhone,
        Column::DepartmentCode,
        Column::PrimaryRole,
        Column::AdditionalRoles,
        Column::ManagerEmployeeNumber,
        Column::EmploymentType,
        Column::StartDate,
        Column::EndDate,
        Column::AdAccountHint,
    ];

    /// Baslik satirindaki kanonik ad; ornek dosya ve ekran rehberi bunu kullanir.
    pub fn name(self) -> &'static str {
        match self {
            Column::EmployeeNumber => "employee_number",
            Column::GivenName => "given_name",
            Column::Surname => "surname",
            Column::NationalId => "national_id",
            Column::NationalIdCountry => "national_id_country",
            Column::MobilePhone => "mobile_phone",
            Column::DepartmentCode => "department_code",
            Column::PrimaryRole => "primary_role",
            Column::AdditionalRoles => "additional_roles",
            Column::ManagerEmployeeNumber => "manager_employee_number",
            Column::EmploymentType => "employment_type",
            Column::StartDate => "start_date",
            Column::EndDate => "end_date",
            Column::AdAccountHint => "ad_account_hint",
        }
    }

    pub fn description_key(self) -> &'static str {
        match self {
            Column::EmployeeNumber => "import.col.employee_number",
            Column::GivenName => "import.col.given_name",
            Column::Surname => "import.col.surname",
            Column::NationalId => "import.col.national_id",
            Column::NationalIdCountry => "import.col.national_id_country",
            Column::MobilePhone => "import.col.mobile_phone",
            Column::DepartmentCode => "import.col.department_code",
            Column::PrimaryRole => "import.col.primary_role",
            Column::AdditionalRoles => "import.col.additional_roles",
            Column::ManagerEmployeeNumber => "import.col.manager_employee_number",
            Column::EmploymentType => "import.col.employment_type",
            Column::StartDate => "import.col.start_date",
            Column::EndDate => "import.col.end_date",
            Column::AdAccountHint => "import.col.ad_account_hint",
        }
    }

    /// Ornek dosyanin tek satiri.
    pub fn example(self) -> &'static str {
        match self {
            Column::EmployeeNumber => "1001",
            Column::GivenName => "Ayşe",
            Column::Surname => "Yılmaz",
            Column::NationalId => "",
            Column::NationalIdCountry => "TR",
            Column::MobilePhone => "+905321234567",
            Column::DepartmentCode => "BT",
            Column::PrimaryRole => "Sistem Uzmanı",
            Column::AdditionalRoles => "VPN Kullanıcısı;Nöbetçi",
            Column::ManagerEmployeeNumber => "1000",
            Column::EmploymentType => "permanent",
            Column::StartDate => "2026-01-15",
            Column::EndDate => "",
            Column::AdAccountHint => "ayse.yilmaz",
        }
    }

    /// Turkce basliklar (`fold` sonrasi).
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Column::EmployeeNumber => &["sicil no", "sicil"],
            Column::GivenName => &["ad", "adi"],
            Column::Surname => &["soyad", "soyadi"],
            Column::NationalId => &["kimlik no", "tc kimlik no", "tc"],
            Column::NationalIdCountry => &["ulke", "kimlik ulkesi"],
            Column::MobilePhone => &["cep telefonu", "telefon", "cep"],
            Column::DepartmentCode => &["departman kodu", "departman"],
            Column::PrimaryRole => &["birincil rol", "rol"],
            Column::AdditionalRoles => &["ek roller", "ek rol"],
            Column::ManagerEmployeeNumber => &["yonetici sicil no", "yonetici"],
            Column::EmploymentType => &["calisma tipi", "tip"],
            Column::StartDate => &["baslangic tarihi", "baslangic", "ise giris"],
            Column::EndDate => &["bitis tarihi", "bitis", "ayrilis"],
            Column::AdAccountHint => &["ad hesabi", "ad kullanici adi", "ipucu", "hesap ipucu"],
        }
    }

    pub fn from_header(raw: &str) -> Option<Column> {
        let folded = fold(raw);
        Column::ALL
            .into_iter()
            .find(|c| fold(c.name()) == folded || c.aliases().contains(&folded.as_str()))
    }
}

// ---- CSV cozumleme (RFC 4180: tirnak, cift tirnak kacisi, CRLF; ayrac , ; ya da sekme) ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    /// Dosyadaki satir numarasi (baslik 1)
    pub line: usize,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl Table {
    pub fn has(&self, column: Column) -> bool {
        self.columns.contains(&column)
    }

    fn cell<'a>(&self, row: &'a Row, column: Column) -> Option<&'a str> {
        let i = self.columns.iter().position(|c| *c == column)?;
        row.cells.get(i).map(String::as_str)
    }
}

/// Dosya duzeyinde hata: i18n anahtari + (varsa) baslik adi ya da satir numarasi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub key: &'static str,
    pub detail: String,
}

fn problem(key: &'static str, detail: impl ToString) -> Problem {
    Problem {
        key,
        detail: detail.to_string(),
    }
}

pub fn parse(text: &str) -> Result<Table, Problem> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delim = delimiter(text.lines().next().unwrap_or(""));
    // Kayitlar tek tek okunur: bos satir tutulmaz, satir siniri asilinca dosyanin
    // gerisi ayristirilmaz (guvenlik denetimi OS-12: once hepsini Vec'e almak
    // girdinin ~140 kati bellek ayiriyordu).
    let mut filled = Records::new(text, delim)
        .enumerate()
        .map(|(i, record)| {
            record.map(|cells| (i + 1, cells)).map_err(|key| {
                let line = if key == RAGGED_ROW {
                    (i + 1).to_string()
                } else {
                    String::new()
                };
                problem(key, line)
            })
        })
        .filter(|r| {
            r.as_ref().map_or(true, |(_, cells)| {
                cells.iter().any(|c| !c.trim().is_empty())
            })
        });
    let (_, header) = filled
        .next()
        .ok_or_else(|| problem("err.import_empty", ""))??;
    let columns = columns_of(&header)?;
    let mut rows = Vec::new();
    for record in filled {
        let (line, cells) = record?;
        if cells.len() != columns.len() {
            return Err(problem(RAGGED_ROW, line));
        }
        if rows.len() == MAX_ROWS {
            return Err(problem("err.import_too_many_rows", MAX_ROWS));
        }
        let cells = cells.iter().map(|c| c.trim().to_string()).collect();
        rows.push(Row { line, cells });
    }
    if rows.is_empty() {
        return Err(problem("err.import_empty", ""));
    }
    Ok(Table { columns, rows })
}

/// Ilk satirda en cok gecen ayrac; Excel Turkce yerel ayarda ';' yazar.
fn delimiter(first_line: &str) -> char {
    [',', ';', '\t']
        .into_iter()
        .max_by_key(|d| first_line.matches(*d).count())
        .unwrap_or(',')
}

const RAGGED_ROW: &str = "err.import_ragged_row";
/// Gecerli bir satirda bundan fazla hucre olamaz (her sutun en cok bir kez);
/// sinir, tek satirlik ayrac yiginini hucre hucre bellege almayi keser.
const MAX_CELLS: usize = Column::ALL.len();

struct Records<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    delim: char,
}

impl<'a> Records<'a> {
    fn new(text: &'a str, delim: char) -> Self {
        Records {
            chars: text.chars().peekable(),
            delim,
        }
    }
}

impl Iterator for Records<'_> {
    type Item = Result<Vec<String>, &'static str>;

    fn next(&mut self) -> Option<Self::Item> {
        self.chars.peek()?;
        let (mut row, mut cell) = (Vec::new(), String::new());
        let mut quoted = false;
        while let Some(c) = self.chars.next() {
            match (quoted, c) {
                (true, '"') if self.chars.peek() == Some(&'"') => {
                    self.chars.next();
                    cell.push('"');
                }
                (true, '"') => quoted = false,
                (true, other) => cell.push(other),
                (false, '"') if cell.is_empty() => quoted = true,
                (false, '\r') => {}
                (false, '\n') => break,
                (false, other) if other == self.delim => {
                    row.push(std::mem::take(&mut cell));
                    if row.len() >= MAX_CELLS {
                        return Some(Err(RAGGED_ROW));
                    }
                }
                (false, other) => cell.push(other),
            }
        }
        if quoted {
            return Some(Err("err.import_unclosed_quote"));
        }
        row.push(cell);
        Some(Ok(row))
    }
}

fn columns_of(header: &[String]) -> Result<Vec<Column>, Problem> {
    let mut columns = Vec::with_capacity(header.len());
    for raw in header {
        let column =
            Column::from_header(raw).ok_or_else(|| problem("err.import_unknown_column", raw))?;
        if columns.contains(&column) {
            return Err(problem("err.import_duplicate_column", raw));
        }
        columns.push(column);
    }
    if !columns.contains(&Column::EmployeeNumber) {
        return Err(problem(
            "err.import_no_key_column",
            Column::EmployeeNumber.name(),
        ));
    }
    Ok(columns)
}

// ---- hucre degerleri ----

/// Ingilizce anahtar ya da Turkce etiket; taninmayan deger oldugu gibi kalir ve
/// `identity::validate` reddeder.
fn employment_type(raw: &str) -> String {
    match fold(raw).as_str() {
        "permanent" | "kadrolu" => "permanent",
        "contract" | "sozlesmeli" => "contract",
        "intern" | "stajyer" => "intern",
        "outsourced" | "dis kaynak" | "taseron" => "outsourced",
        _ => return raw.trim().to_string(),
    }
    .to_string()
}

/// `YYYY-MM-DD` oldugu gibi; `GG.AA.YYYY` ve `GG/AA/YYYY` ISO'ya cevrilir.
fn iso_date(raw: &str) -> String {
    let text = raw.trim();
    for sep in ['.', '/'] {
        if let Some((day, rest)) = text.split_once(sep) {
            if let Some((month, year)) = rest.split_once(sep) {
                if day.len() <= 2 && month.len() <= 2 && year.len() == 4 {
                    return format!("{year}-{month:0>2}-{day:0>2}");
                }
            }
        }
    }
    text.to_string()
}

fn role_names(raw: &str) -> Vec<String> {
    raw.split(ROLE_SEPARATORS)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// ---- veritabanindaki karsiliklar ----

struct Existing {
    id: i64,
    form: IdentityForm,
    end_date: Option<String>,
    departed: bool,
    bidx: Option<Vec<u8>>,
    undated_roles: Vec<i64>,
}

#[derive(Default)]
struct Refs {
    departments: HashMap<String, i64>,
    primary_roles: HashMap<String, i64>,
    additional_roles: HashMap<String, i64>,
    existing: HashMap<String, Existing>,
}

type ExistingRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    i64,
    i64,
    Option<i64>,
    String,
    String,
    Option<String>,
    bool,
    Option<String>,
    Option<Vec<u8>>,
    Option<String>,
    Option<Vec<i64>>,
);

macro_rules! existing_sql {
    ($filter:literal) => {
        concat!(
            "SELECT i.id, i.employee_number, i.given_name, i.surname, \
             i.mobile_phone, i.department_id, i.primary_role_id, i.manager_id, i.employment_type, \
             to_char(i.start_date, 'YYYY-MM-DD'), to_char(i.end_at - interval '1 second', 'YYYY-MM-DD'), \
             i.end_at IS NOT NULL AND i.end_at <= now(), i.existing_ad_account_hint, i.national_id_bidx, \
             i.national_id_country, \
             (SELECT array_agg(a.role_id) FROM identity_additional_roles a \
                WHERE a.identity_id = i.id AND a.ends_on IS NULL) \
             FROM identities i WHERE i.deleted_at IS NULL AND ",
            $filter
        )
    };
}

const EXISTING_SQL: &str = existing_sql!("i.employee_number = ANY($1)");

/// ADR-055 madde 3: sicil nosu dosyada olmayan ama kimlik numarasi tutan kimlik.
async fn existing_by_id(pool: &PgPool, id: i64) -> Result<Option<Existing>, sqlx::Error> {
    let row: Option<ExistingRow> = sqlx::query_as(existing_sql!("i.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(existing_from))
}

impl Refs {
    async fn load(pool: &PgPool, numbers: &[String]) -> Result<Refs, sqlx::Error> {
        let mut refs = Refs::default();
        let departments: Vec<(i64, Option<String>, String)> =
            sqlx::query_as("SELECT id, code, name FROM departments")
                .fetch_all(pool)
                .await?;
        for (id, code, name) in departments {
            // kod oncelikli; ad yalnizca kodla taninmayan bir baslik degilse
            if let Some(code) = code.filter(|c| !c.trim().is_empty()) {
                refs.departments.insert(fold(&code), id);
            }
            refs.departments.entry(fold(&name)).or_insert(id);
        }
        let roles: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, kind, name FROM roles WHERE NOT placeholder")
                .fetch_all(pool)
                .await?;
        for (id, kind, name) in roles {
            match kind.as_str() {
                "primary" => refs.primary_roles.insert(fold(&name), id),
                "additional" => refs.additional_roles.insert(fold(&name), id),
                _ => None,
            };
        }
        let rows: Vec<ExistingRow> = sqlx::query_as(EXISTING_SQL)
            .bind(numbers)
            .fetch_all(pool)
            .await?;
        for r in rows {
            refs.existing.insert(r.1.clone(), existing_from(r));
        }
        Ok(refs)
    }
}

fn existing_from(r: ExistingRow) -> Existing {
    Existing {
        id: r.0,
        form: IdentityForm {
            given_name: r.2,
            surname: r.3,
            employee_number: r.1,
            mobile_phone: r.4.unwrap_or_default(),
            department_id: r.5.to_string(),
            primary_role_id: r.6.to_string(),
            manager_id: r.7.map(|m| m.to_string()).unwrap_or_default(),
            employment_type: r.8,
            start_date: r.9,
            end_date: r.10.clone().unwrap_or_default(),
            existing_ad_account_hint: r.12.unwrap_or_default(),
            national_id_country: r.14.unwrap_or_else(|| "TR".to_string()),
            ..IdentityForm::default()
        },
        end_date: r.10,
        departed: r.11,
        bidx: r.13,
        undated_roles: r.15.unwrap_or_default(),
    }
}

// ---- plan ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    New,
    Update,
    Unchanged,
}

impl Action {
    pub fn key(self) -> &'static str {
        match self {
            Action::New => "import.action.new",
            Action::Update => "import.action.update",
            Action::Unchanged => "import.action.unchanged",
        }
    }
}

pub struct RowPlan {
    pub line: usize,
    pub employee_number: String,
    pub person: String,
    pub action: Action,
    pub identity_id: Option<i64>,
    /// Degisen kolonlarin adlari (gosterim)
    pub changes: Vec<&'static str>,
    pub cleared: usize,
    new: NewIdentity,
    /// Some(Some(gun)) bitisi yaz, Some(None) temizle, None dokunma
    end: Option<Option<String>>,
    /// Tarihsiz ek rollerin yeni kumesi; None = kolon yok, dokunma
    roles: Option<Vec<i64>>,
    /// Yazilacak kimlik no (yeni kimlik ya da numarasi olmayan mevcut kimlik)
    national_id: Option<NationalId>,
    /// Yonetici bu dosyada acilacak bir kimlik: id'si uygulamada baglanir
    manager_ref: Option<String>,
    touch_manager: bool,
    /// ADR-055 madde 3: satir, kimlik numarasi tutan mevcut kimligin sicil nosunu
    /// degistiriyor; eski numara (gosterim ve denetim)
    pub renumber: Option<String>,
}

impl RowPlan {
    pub fn changes_text(&self) -> String {
        self.changes.join(", ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    pub line: usize,
    /// Ilgili kolonun adi; dosya duzeyinde ya da bilinmiyorsa bos
    pub column: &'static str,
    pub key: &'static str,
    pub detail: String,
}

fn row_error(line: usize, column: Option<Column>, key: &'static str) -> RowError {
    RowError {
        line,
        column: column.map_or("", Column::name),
        key,
        detail: String::new(),
    }
}

pub struct Duplicate {
    pub line: usize,
    pub employee_number: String,
    pub person: String,
    pub existing_id: i64,
    pub existing_person: String,
    pub existing_number: String,
}

/// Sicil no degisimi onerisi (ADR-042 madde 3 / ADR-055 madde 3): ayni kimlik no,
/// farkli sicil no; onaylanirsa mevcut kimligin sicil nosu guncellenir, yeni kimlik acilmaz.
pub struct Renumber {
    pub line: usize,
    pub person: String,
    pub identity_id: i64,
    pub old_number: String,
    pub new_number: String,
}

/// Dosya duzeyindeki iki onay kutusu; sahnelenen partiyle birlikte saklanir.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Confirmed {
    pub duplicates: bool,
    pub renumber: bool,
}

#[derive(Default)]
pub struct Plan {
    pub rows: Vec<RowPlan>,
    pub errors: Vec<RowError>,
    pub duplicates: Vec<Duplicate>,
    pub renumbers: Vec<Renumber>,
    pub new: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub cleared: usize,
}

impl Plan {
    /// Degisiklik seti esiginin saydigi: yeni ya da degisen kimlik (ADR-037).
    pub fn affected(&self) -> usize {
        self.new + self.updated
    }

    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Dosya duzeyinde durum; veritabani karsiliklari (`Refs`) ayri tutulur ki satir
/// planlanirken mevcut kimlik odunc alinmisken dosya kumeleri yazilabilsin.
struct Ctx<'a> {
    pool: &'a PgPool,
    keys: &'a Keys<'a>,
    ownership: bool,
    /// Yukleyen ya da onaylayan operator (ADR-005 kendi kaydi kurali)
    operator: &'a str,
    file_numbers: HashSet<String>,
    numbers_seen: HashSet<String>,
    bidx_seen: HashSet<Vec<u8>>,
}

/// Dosyayi veritabanina karsi degerlendirir; hicbir sey yazmaz.
pub async fn plan(
    pool: &PgPool,
    keys: &Keys<'_>,
    ownership: bool,
    operator: &str,
    table: &Table,
) -> Result<Plan, sqlx::Error> {
    let numbers: Vec<String> = table
        .rows
        .iter()
        .filter_map(|r| table.cell(r, Column::EmployeeNumber))
        .map(str::to_string)
        .collect();
    let refs = Refs::load(pool, &numbers).await?;
    let mut ctx = Ctx {
        pool,
        keys,
        ownership,
        operator,
        file_numbers: numbers.into_iter().collect(),
        numbers_seen: HashSet::new(),
        bidx_seen: HashSet::new(),
    };
    let mut plan = Plan::default();
    for row in &table.rows {
        match plan_row(&mut ctx, &refs, table, row).await? {
            Ok(planned) => {
                match planned.action {
                    Action::New => plan.new += 1,
                    Action::Update => plan.updated += 1,
                    Action::Unchanged => plan.unchanged += 1,
                }
                plan.cleared += planned.cleared;
                plan.rows.push(planned);
            }
            Err(e) => plan.errors.push(e),
        }
    }
    plan.duplicates = possible_duplicates(pool, &plan.rows).await?;
    plan.renumbers = renumbers_of(&plan.rows);
    Ok(plan)
}

fn renumbers_of(rows: &[RowPlan]) -> Vec<Renumber> {
    rows.iter()
        .filter_map(|r| {
            Some(Renumber {
                line: r.line,
                person: r.person.clone(),
                identity_id: r.identity_id?,
                old_number: r.renumber.clone()?,
                new_number: r.employee_number.clone(),
            })
        })
        .collect()
}

type RowOutcome = Result<Result<RowPlan, RowError>, sqlx::Error>;

/// Sicil no: zorunlu ve dosyada tekil.
fn row_number(ctx: &mut Ctx<'_>, table: &Table, row: &Row) -> Result<String, RowError> {
    let number = table
        .cell(row, Column::EmployeeNumber)
        .unwrap_or_default()
        .to_string();
    if number.is_empty() {
        let key = "err.import_employee_number_blank";
        return Err(row_error(row.line, Some(Column::EmployeeNumber), key));
    }
    if !ctx.numbers_seen.insert(number.clone()) {
        let key = "err.import_duplicate_employee_number";
        return Err(row_error(row.line, Some(Column::EmployeeNumber), key));
    }
    Ok(number)
}

async fn plan_row(ctx: &mut Ctx<'_>, refs: &Refs, table: &Table, row: &Row) -> RowOutcome {
    let number = match row_number(ctx, table, row) {
        Ok(number) => number,
        Err(e) => return Ok(Err(e)),
    };
    let by_national_id = match renumber_candidate(ctx, refs, table, row, &number).await? {
        Ok(found) => found,
        Err(e) => return Ok(Err(e)),
    };
    let renumber = by_national_id
        .as_ref()
        .map(|e| e.form.employee_number.clone());
    let existing = refs.existing.get(&number).or(by_national_id.as_ref());
    let (form, manager_ref) = match merged_form(ctx, refs, table, row, existing) {
        Ok(merged) => merged,
        Err(e) => return Ok(Err(e)),
    };
    let new = match identity::validate(&form) {
        Ok(new) => new,
        Err(key) => return Ok(Err(row_error(row.line, column_of(key), key))),
    };
    // ADR-023 madde 2: sahiplenme acikken yeni kimlik ipucusuz acilmaz
    if ctx.ownership && existing.is_none() && new.existing_ad_account_hint.is_none() {
        let key = "err.import_hint_required";
        return Ok(Err(row_error(row.line, Some(Column::AdAccountHint), key)));
    }
    let end = match end_change(table, row, existing) {
        Ok(end) => end,
        Err(e) => return Ok(Err(e)),
    };
    let roles = match roles_for(refs, table, row) {
        Ok(roles) => roles,
        Err(e) => return Ok(Err(e)),
    };
    let national_id = match national_id_for(ctx, row.line, &new, existing).await? {
        Ok(national_id) => national_id,
        Err(e) => return Ok(Err(e)),
    };
    let parts = RowParts {
        new,
        end,
        roles,
        national_id,
        manager_ref,
        renumber,
    };
    let planned = finish_row(table, row, number, existing, parts);
    if let Some(id) = planned.identity_id.filter(|_| touches_own_access(&planned)) {
        if crate::operator_guard::is_own_record(ctx.pool, id, ctx.operator).await? {
            return Ok(Err(row_error(row.line, None, "err.import_own_record")));
        }
    }
    Ok(Ok(planned))
}

/// ADR-005: operator kendi kaydinda rolunu, departmanini ve ayrilisini
/// degistiremez; formlar 403 verir, dosya da ayni kurala uyar.
const OWN_ACCESS_COLUMNS: [Column; 4] = [
    Column::DepartmentCode,
    Column::PrimaryRole,
    Column::AdditionalRoles,
    Column::EndDate,
];

fn touches_own_access(planned: &RowPlan) -> bool {
    OWN_ACCESS_COLUMNS
        .iter()
        .any(|column| planned.changes.contains(&column.name()))
}

/// Sicil nosu kayitli olmayan satirin kimlik numarasi baska bir kimlikte kayitliysa
/// satir o kimligin guncellemesi sayilir (sicil no degisimi onerisi, ADR-055 madde 3).
/// Numara bicimi bozuksa burada susulur, `identity::validate` raporlar.
async fn renumber_candidate(
    ctx: &Ctx<'_>,
    refs: &Refs,
    table: &Table,
    row: &Row,
    number: &str,
) -> Result<Result<Option<Existing>, RowError>, sqlx::Error> {
    if refs.existing.contains_key(number) {
        return Ok(Ok(None));
    }
    let raw = table
        .cell(row, Column::NationalId)
        .filter(|v| !v.is_empty());
    let Some(raw) = raw else {
        return Ok(Ok(None));
    };
    let country = table
        .cell(row, Column::NationalIdCountry)
        .filter(|v| !v.is_empty())
        .unwrap_or("TR");
    let Ok(nid) = national_id::parse(country, raw) else {
        return Ok(Ok(None));
    };
    let Some(id) = national_id::find_identity(ctx.pool, ctx.keys.blind_index, &nid).await? else {
        return Ok(Ok(None));
    };
    Ok(Ok(existing_by_id(ctx.pool, id).await?))
}

/// Dogrulanmis parcalar; satir plani bunlardan kurulur.
struct RowParts {
    new: NewIdentity,
    end: Option<Option<String>>,
    roles: Option<Vec<i64>>,
    national_id: Option<NationalId>,
    manager_ref: Option<String>,
    renumber: Option<String>,
}

fn finish_row(
    table: &Table,
    row: &Row,
    number: String,
    existing: Option<&Existing>,
    parts: RowParts,
) -> RowPlan {
    let mut planned = RowPlan {
        line: row.line,
        employee_number: number,
        person: format!("{} {}", parts.new.given_name, parts.new.surname),
        action: Action::New,
        identity_id: existing.map(|e| e.id),
        changes: Vec::new(),
        cleared: 0,
        touch_manager: table.has(Column::ManagerEmployeeNumber) && parts.manager_ref.is_none(),
        new: parts.new,
        end: parts.end,
        roles: parts.roles,
        national_id: parts.national_id,
        manager_ref: parts.manager_ref,
        renumber: parts.renumber,
    };
    if let Some(existing) = existing {
        let (changes, cleared) = changes_of(table, &planned, existing);
        planned.action = if changes.is_empty() {
            Action::Unchanged
        } else {
            Action::Update
        };
        planned.changes = changes;
        planned.cleared = cleared;
    }
    planned
}

/// Mevcut kimligin alanlari (yoksa bos form) uzerine dosyadaki kolonlar yazilir;
/// kolon yoksa alan oldugu gibi kalir (ADR-023 madde 3).
fn merged_form(
    ctx: &Ctx<'_>,
    refs: &Refs,
    table: &Table,
    row: &Row,
    existing: Option<&Existing>,
) -> Result<(IdentityForm, Option<String>), RowError> {
    let mut form = existing
        .map(|e| e.form.clone())
        .unwrap_or_else(|| IdentityForm {
            national_id_country: "TR".to_string(),
            ..IdentityForm::default()
        });
    let mut manager_ref = None;
    for (column, value) in table.columns.iter().zip(&row.cells) {
        let value = value.as_str();
        match column {
            Column::EmployeeNumber => form.employee_number = value.to_string(),
            Column::GivenName => form.given_name = value.to_string(),
            Column::Surname => form.surname = value.to_string(),
            Column::NationalId => form.national_id = value.to_string(),
            Column::NationalIdCountry if !value.is_empty() => {
                form.national_id_country = value.to_string()
            }
            Column::NationalIdCountry => {}
            Column::MobilePhone => form.mobile_phone = value.to_string(),
            Column::EmploymentType => form.employment_type = employment_type(value),
            Column::StartDate => form.start_date = iso_date(value),
            Column::EndDate => form.end_date = iso_date(value),
            Column::AdAccountHint => form.existing_ad_account_hint = value.to_string(),
            Column::DepartmentCode => {
                let key = "err.import_department_unknown";
                form.department_id = lookup(&refs.departments, value, row.line, key)?;
            }
            Column::PrimaryRole => {
                let key = "err.import_role_unknown";
                form.primary_role_id = lookup(&refs.primary_roles, value, row.line, key)?;
            }
            Column::ManagerEmployeeNumber => {
                let (id, reference) = manager_of(ctx, refs, value, row.line)?;
                form.manager_id = id;
                manager_ref = reference;
            }
            Column::AdditionalRoles => {}
        }
    }
    Ok((form, manager_ref))
}

/// Bos hucre bos kalir (zorunlu alanda `validate` yakalar); dolu hucre tabloda bulunmali.
fn lookup(
    map: &HashMap<String, i64>,
    value: &str,
    line: usize,
    key: &'static str,
) -> Result<String, RowError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    match map.get(&fold(value)) {
        Some(id) => Ok(id.to_string()),
        None => Err(RowError {
            line,
            column: "",
            key,
            detail: value.to_string(),
        }),
    }
}

/// Yonetici: veritabanindaki kimlik (id), bu dosyada acilacak kimlik (referans)
/// ya da bilinmeyen sicil no (hata).
fn manager_of(
    ctx: &Ctx<'_>,
    refs: &Refs,
    value: &str,
    line: usize,
) -> Result<(String, Option<String>), RowError> {
    if value.is_empty() {
        return Ok((String::new(), None));
    }
    if let Some(existing) = refs.existing.get(value) {
        return Ok((existing.id.to_string(), None));
    }
    if ctx.file_numbers.contains(value) {
        return Ok((String::new(), Some(value.to_string())));
    }
    Err(RowError {
        line,
        column: Column::ManagerEmployeeNumber.name(),
        key: "err.import_manager_unknown",
        detail: value.to_string(),
    })
}

/// `identity::validate` anahtarlarinin kolon karsiligi (ekranda satir + kolon gosterilir).
fn column_of(key: &str) -> Option<Column> {
    Some(match key {
        "err.given_name_blank" => Column::GivenName,
        "err.surname_blank" => Column::Surname,
        "err.department_required" => Column::DepartmentCode,
        "err.primary_role_required" => Column::PrimaryRole,
        "err.manager_invalid" => Column::ManagerEmployeeNumber,
        "err.employment_type_required" => Column::EmploymentType,
        "err.start_date_format" => Column::StartDate,
        "err.end_date_format" | "err.end_required_non_permanent" | "err.end_before_start" => {
            Column::EndDate
        }
        "err.mobile_format" => Column::MobilePhone,
        "err.account_hint_shape" => Column::AdAccountHint,
        k if k.contains("national_id") => Column::NationalId,
        _ => return None,
    })
}

/// ADR-030: `ayrildi` kimligin bitisi bosaltilamaz ve ileri alinamaz; ayni tarih dokunmaz.
fn end_change(
    table: &Table,
    row: &Row,
    existing: Option<&Existing>,
) -> Result<Option<Option<String>>, RowError> {
    let Some(raw) = table.cell(row, Column::EndDate) else {
        return Ok(None);
    };
    let wanted = Some(iso_date(raw)).filter(|d| !d.is_empty());
    let Some(existing) = existing else {
        return Ok(None); // yeni kimlikte bitis INSERT'in parcasi
    };
    if wanted == existing.end_date {
        return Ok(None);
    }
    let later = match (&wanted, &existing.end_date) {
        (None, _) => true,
        (Some(w), Some(current)) => w > current,
        (Some(_), None) => false,
    };
    if existing.departed && later {
        let key = "err.import_departed_end";
        return Err(row_error(row.line, Some(Column::EndDate), key));
    }
    Ok(Some(wanted))
}

fn roles_for(refs: &Refs, table: &Table, row: &Row) -> Result<Option<Vec<i64>>, RowError> {
    let Some(raw) = table.cell(row, Column::AdditionalRoles) else {
        return Ok(None);
    };
    let mut ids = Vec::new();
    for name in role_names(raw) {
        let key = "err.import_additional_role_unknown";
        let id: i64 = lookup(&refs.additional_roles, &name, row.line, key)?
            .parse()
            .unwrap_or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(Some(ids))
}

/// Kimlik no: dosyada iki kez ayni numara hata; mevcut kimligin numarasi varsa
/// degistirilemez (sicil no degisimi onerisi ayri kutucuk, ADR-055 madde 3);
/// baska bir kimligin numarasi hata (ADR-042 madde 2). Bos hucre dokunmaz.
async fn national_id_for(
    ctx: &mut Ctx<'_>,
    line: usize,
    new: &NewIdentity,
    existing: Option<&Existing>,
) -> Result<Result<Option<NationalId>, RowError>, sqlx::Error> {
    let Some(nid) = &new.national_id else {
        return Ok(Ok(None));
    };
    let bidx = national_id::blind_index(ctx.keys.blind_index, nid);
    if !ctx.bidx_seen.insert(bidx.clone()) {
        let key = "err.import_duplicate_national_id";
        return Ok(Err(row_error(line, Some(Column::NationalId), key)));
    }
    if let Some(current) = existing.and_then(|e| e.bidx.as_ref()) {
        return Ok(if *current == bidx {
            Ok(None)
        } else {
            let key = "err.import_national_id_mismatch";
            Err(row_error(line, Some(Column::NationalId), key))
        });
    }
    let other = national_id::find_identity(ctx.pool, ctx.keys.blind_index, nid).await?;
    Ok(match other {
        Some(id) if existing.is_none_or(|e| e.id != id) => {
            let key = "err.import_national_id_other";
            Err(row_error(line, Some(Column::NationalId), key))
        }
        _ => Ok(Some(nid.clone())),
    })
}

/// Mevcut kimlikte dosyadaki kolonlardan hangileri degisiyor ve kaci temizleniyor
/// (ADR-023 madde 5: onizleme temizlemeleri ayrica sayar).
fn changes_of(table: &Table, planned: &RowPlan, existing: &Existing) -> (Vec<&'static str>, usize) {
    let mut changes = Vec::new();
    let mut cleared = 0;
    for column in &table.columns {
        let (changed, emptied) = change_of(*column, planned, existing);
        if changed {
            changes.push(column.name());
            cleared += usize::from(emptied);
        }
    }
    (changes, cleared)
}

/// Tek kolon icin (degisti mi, bosaltildi mi).
fn change_of(column: Column, planned: &RowPlan, existing: &Existing) -> (bool, bool) {
    let (new, old) = (&planned.new, &existing.form);
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    match column {
        Column::EmployeeNumber => (planned.renumber.is_some(), false),
        Column::NationalIdCountry => (false, false),
        Column::GivenName => (new.given_name != old.given_name, false),
        Column::Surname => (new.surname != old.surname, false),
        Column::NationalId => (planned.national_id.is_some(), false),
        Column::MobilePhone => (
            new.mobile_phone != opt(&old.mobile_phone),
            new.mobile_phone.is_none(),
        ),
        Column::DepartmentCode => (new.department_id.to_string() != old.department_id, false),
        Column::PrimaryRole => (
            new.primary_role_id.to_string() != old.primary_role_id,
            false,
        ),
        Column::ManagerEmployeeNumber => {
            let wanted = new.manager_id.map(|m| m.to_string());
            let changed = planned.manager_ref.is_some() || wanted != opt(&old.manager_id);
            (changed, wanted.is_none() && planned.manager_ref.is_none())
        }
        Column::EmploymentType => (new.employment_type != old.employment_type, false),
        Column::StartDate => (new.start_date != old.start_date, false),
        Column::EndDate => (planned.end.is_some(), planned.end == Some(None)),
        Column::AdditionalRoles => {
            let mut current = existing.undated_roles.clone();
            current.sort_unstable();
            let wanted = planned.roles.as_deref().unwrap_or_default();
            (
                wanted != current.as_slice(),
                wanted.is_empty() && !current.is_empty(),
            )
        }
        Column::AdAccountHint => (
            new.existing_ad_account_hint != opt(&old.existing_ad_account_hint),
            new.existing_ad_account_hint.is_none(),
        ),
    }
}

/// ADR-042 madde 2: yeni satirin ad-soyadi silinmemis bir kimlikle ayni, sicil nosu farkli.
async fn possible_duplicates(
    pool: &PgPool,
    rows: &[RowPlan],
) -> Result<Vec<Duplicate>, sqlx::Error> {
    let new: Vec<&RowPlan> = rows.iter().filter(|r| r.action == Action::New).collect();
    let names: Vec<(String, String)> = new
        .iter()
        .map(|r| (r.new.given_name.clone(), r.new.surname.clone()))
        .collect();
    let similar = identity::similar_people(pool, &names).await?;
    Ok(new
        .into_iter()
        .zip(similar)
        .filter_map(|(row, similar)| {
            let (existing_id, existing_person, existing_number) = similar?;
            Some(Duplicate {
                line: row.line,
                employee_number: row.employee_number.clone(),
                person: row.person.clone(),
                existing_id,
                existing_person,
                existing_number,
            })
        })
        .collect())
}

// ---- uygulama: tek transaction (ADR-018 "hatali satir varsa hicbiri"nin yazma yarisi) ----

#[derive(Default, Debug)]
pub struct Applied {
    pub created: Vec<i64>,
    pub updated: Vec<i64>,
}

pub async fn apply(
    pool: &PgPool,
    keys: &Keys<'_>,
    time_zone: &str,
    plan: &Plan,
) -> Result<Applied, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mut ids: HashMap<&str, i64> = plan
        .rows
        .iter()
        .filter_map(|r| r.identity_id.map(|id| (r.employee_number.as_str(), id)))
        .collect();
    let mut applied = Applied::default();
    for row in &plan.rows {
        match (row.action, row.identity_id) {
            (Action::New, _) => {
                let id = insert_row(&mut tx, keys, time_zone, row).await?;
                ids.insert(&row.employee_number, id);
                applied.created.push(id);
            }
            (Action::Update, Some(id)) => {
                update_row(&mut tx, keys, time_zone, row, id).await?;
                applied.updated.push(id);
            }
            _ => {}
        }
    }
    link_file_managers(&mut tx, plan, &ids).await?;
    let targets: Vec<i64> = sqlx::query_scalar("SELECT id FROM target_systems ORDER BY id")
        .fetch_all(&mut *tx)
        .await?;
    for id in applied.created.iter().chain(&applied.updated) {
        for target in &targets {
            crate::jobs::enqueue(&mut *tx, *id, *target, crate::jobs::Priority::Bulk).await?;
        }
    }
    tx.commit().await?;
    Ok(applied)
}

async fn insert_row(
    tx: &mut Transaction<'_, Postgres>,
    keys: &Keys<'_>,
    time_zone: &str,
    row: &RowPlan,
) -> Result<i64, sqlx::Error> {
    let id = identity::insert_identity(tx, time_zone, &row.new).await?;
    if let Some(nid) = &row.national_id {
        national_id::store(&mut **tx, keys, id, nid).await?;
    }
    if let Some(roles) = &row.roles {
        sync_roles(tx, id, roles).await?;
    }
    Ok(id)
}

const UPDATE_SQL: &str = "UPDATE identities SET given_name = $2, surname = $3, mobile_phone = $4, \
     department_id = $5, primary_role_id = $6, employment_type = $7, start_date = $8::date, \
     existing_ad_account_hint = $9, \
     manager_id = CASE WHEN $10 THEN $11 ELSE manager_id END, \
     end_at = CASE WHEN $12 THEN (($13::date + 1)::timestamp AT TIME ZONE $14) ELSE end_at END, \
     employee_number = $15 \
     WHERE id = $1 AND deleted_at IS NULL";

async fn update_row(
    tx: &mut Transaction<'_, Postgres>,
    keys: &Keys<'_>,
    time_zone: &str,
    row: &RowPlan,
    id: i64,
) -> Result<(), sqlx::Error> {
    let new = &row.new;
    sqlx::query(UPDATE_SQL)
        .bind(id)
        .bind(&new.given_name)
        .bind(&new.surname)
        .bind(&new.mobile_phone)
        .bind(new.department_id)
        .bind(new.primary_role_id)
        .bind(&new.employment_type)
        .bind(&new.start_date)
        .bind(&new.existing_ad_account_hint)
        .bind(row.touch_manager)
        .bind(new.manager_id.filter(|m| *m != id))
        .bind(row.end.is_some())
        .bind(row.end.clone().flatten())
        .bind(time_zone)
        .bind(&new.employee_number)
        .execute(&mut **tx)
        .await?;
    if let Some(nid) = &row.national_id {
        national_id::store(&mut **tx, keys, id, nid).await?;
    }
    if let Some(roles) = &row.roles {
        sync_roles(tx, id, roles).await?;
    }
    Ok(())
}

/// ADR-055 madde 2: yalnizca tarihsiz atamalar yonetilir; listede olan tarihli
/// atama tarihini korur (DO NOTHING), listede olmayan tarihli atama kalir.
async fn sync_roles(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    roles: &[i64],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM identity_additional_roles WHERE identity_id = $1 AND ends_on IS NULL \
         AND NOT (role_id = ANY($2))",
    )
    .bind(id)
    .bind(roles)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO identity_additional_roles (identity_id, role_id) \
         SELECT $1, r FROM unnest($2::bigint[]) AS r ON CONFLICT (identity_id, role_id) DO NOTHING",
    )
    .bind(id)
    .bind(roles)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Yoneticisi ayni dosyada acilan kimlikler: id'ler artik belli.
async fn link_file_managers(
    tx: &mut Transaction<'_, Postgres>,
    plan: &Plan,
    ids: &HashMap<&str, i64>,
) -> Result<(), sqlx::Error> {
    for row in plan.rows.iter().filter(|r| r.manager_ref.is_some()) {
        let reference = row.manager_ref.as_deref().unwrap_or_default();
        let (Some(id), Some(manager)) = (ids.get(row.employee_number.as_str()), ids.get(reference))
        else {
            continue;
        };
        sqlx::query("UPDATE identities SET manager_id = $2 WHERE id = $1 AND $2 <> $1")
            .bind(id)
            .bind(manager)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

// ---- esigi asan dosya: sifreli sahneleme ve onay (ADR-018, ADR-026, ADR-055) ----

pub struct Batch {
    pub id: i64,
    pub table: Table,
    pub by_subject: String,
    pub by_username: String,
    pub age_seconds: i64,
    pub confirmed: Confirmed,
    pub affected_at_stage: i32,
}

pub struct BatchSummary {
    pub id: i64,
    pub by_username: String,
    pub row_count: i32,
    pub age_minutes: i64,
}

pub async fn stage(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    table: &Table,
    affected: usize,
    by: (&str, &str, Confirmed),
) -> Result<i64, sqlx::Error> {
    let json = serde_json::to_vec(table).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query_scalar(
        "INSERT INTO import_batches (rows_enc, row_count, affected, duplicates_confirmed, \
         renumber_confirmed, by_subject, by_username) VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id",
    )
    .bind(crate::crypto::encrypt_versioned(aead_key, &json))
    .bind(table.rows.len() as i32)
    .bind(affected as i32)
    .bind(by.2.duplicates)
    .bind(by.2.renumber)
    .bind(by.0)
    .bind(by.1)
    .fetch_one(pool)
    .await
}

pub async fn pending(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    id: i64,
) -> Result<Option<Batch>, sqlx::Error> {
    type Row = (Vec<u8>, i32, bool, bool, String, String, i64);
    let row: Option<Row> = sqlx::query_as(
        "SELECT rows_enc, affected, duplicates_confirmed, renumber_confirmed, by_subject, \
         by_username, EXTRACT(EPOCH FROM now() - created_at)::bigint \
         FROM import_batches WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((enc, affected, duplicates, renumber, by_subject, by_username, age_seconds)) = row
    else {
        return Ok(None);
    };
    let json = crate::crypto::decrypt_versioned(aead_key, &enc)
        .map_err(|e| sqlx::Error::Decode(e.into()))?;
    let table: Table =
        serde_json::from_slice(&json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    Ok(Some(Batch {
        id,
        table,
        by_subject,
        by_username,
        age_seconds,
        confirmed: Confirmed {
            duplicates,
            renumber,
        },
        affected_at_stage: affected,
    }))
}

pub async fn list_pending(pool: &PgPool) -> Result<Vec<BatchSummary>, sqlx::Error> {
    let rows: Vec<(i64, String, i32, i64)> = sqlx::query_as(
        "SELECT id, by_username, row_count, \
         (EXTRACT(EPOCH FROM now() - created_at) / 60)::bigint \
         FROM import_batches ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, by_username, row_count, age_minutes)| BatchSummary {
            id,
            by_username,
            row_count,
            age_minutes,
        })
        .collect())
}

pub async fn discard(pool: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM import_batches WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() == 1)
}

// ---- web ----

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/imports", get(page).post(upload))
        .route("/imports/sample.csv", get(sample))
        .route("/imports/preview", post(preview))
        .route("/imports/{id}", get(batch_page))
        .route("/imports/{id}/approve", post(approve))
        .route("/imports/{id}/reject", post(reject))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

pub struct ColumnGuide {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Template)]
#[template(path = "import.html")]
struct ImportTemplate {
    tabs: crate::shell::Tabs,
    lang: Lang,
    shell: Shell,
    notice: Notice,
    can_import: bool,
    ownership: bool,
    pending: Vec<BatchSummary>,
    columns: Vec<ColumnGuide>,
}

/// Onizleme ve onay sayfalarinin ortak govdesi (`import_plan.html`).
#[derive(Default, Clone, Copy)]
pub struct Summary {
    pub new: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub cleared: usize,
    pub affected: usize,
}

impl Summary {
    fn of(plan: &Plan) -> Summary {
        Summary {
            new: plan.new,
            updated: plan.updated,
            unchanged: plan.unchanged,
            cleared: plan.cleared,
            affected: plan.affected(),
        }
    }
}

#[derive(Template)]
#[template(path = "import_preview.html")]
struct PreviewTemplate {
    lang: Lang,
    shell: Shell,
    notice: Notice,
    csv: String,
    summary: Summary,
    rows: Vec<RowPlan>,
    errors: Vec<RowError>,
    duplicates: Vec<Duplicate>,
    renumbers: Vec<Renumber>,
    /// Esik asiliyor: uygulama yerine onaya gider
    will_stage: bool,
    stage_text: String,
}

#[derive(Template)]
#[template(path = "import_batch.html")]
struct BatchTemplate {
    lang: Lang,
    shell: Shell,
    notice: Notice,
    batch_id: i64,
    batch_sub: String,
    /// ADR-055 madde 1: onay anindaki etkilenen sayisi sahnelendigindekinden farkli
    changed_since: bool,
    changed_text: String,
    confirmed: Confirmed,
    summary: Summary,
    rows: Vec<RowPlan>,
    errors: Vec<RowError>,
    duplicates: Vec<Duplicate>,
    renumbers: Vec<Renumber>,
    can_approve: bool,
    can_reject: bool,
}

#[derive(Deserialize)]
struct UploadForm {
    #[serde(default)]
    csv: String,
    #[serde(default)]
    confirm_duplicates: Option<String>,
    #[serde(default)]
    confirm_renumber: Option<String>,
}

/// ADR-023 madde 1 → ADR-131: sahiplenme ortak ayar, her istekte tablodan okunur;
/// okunamazsa kapali sayilir (ipuclu satir sahiplenilmez, yalniz yeni kayit acilir).
async fn ownership_enabled(state: &AppState) -> bool {
    state.common().await.is_ok_and(|c| c.ownership_mode_enabled)
}

fn keys_of(state: &AppState) -> Keys<'_> {
    Keys {
        aead: &state.aead_key,
        blind_index: &state.blind_index_key,
    }
}

async fn render_page(state: &AppState, op: &Operator, notice: Notice) -> Response {
    match tokio::try_join!(
        list_pending(&state.pool),
        crate::identity_web::personnel_tabs(&state.pool, op.lang, "/imports")
    ) {
        Ok((pending, tabs)) => render(&ImportTemplate {
            tabs,
            lang: op.lang,
            shell: Shell::of(op),
            notice,
            can_import: allowed(op, AUTHORITIES),
            ownership: ownership_enabled(state).await,
            pending,
            columns: Column::ALL
                .into_iter()
                .map(|c| ColumnGuide {
                    name: c.name(),
                    description: c.description_key(),
                })
                .collect(),
        }),
        Err(e) => internal("içe aktarma sayfası okunamadı", e),
    }
}

/// ADR-126 madde 1: uygula/onay/red basari yolu buraya yonlendirir, mesaj flash'ta.
const IMPORTS: &str = "/imports";

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    let notice = Notice::take(&state.pool, &op.username).await;
    render_page(&state, &op, notice).await
}

/// Ornek dosya: butun kanonik basliklar ve tek satir.
async fn sample(OperatorSession(_op): OperatorSession) -> Response {
    let header: Vec<&str> = Column::ALL.iter().map(|c| c.name()).collect();
    let example: Vec<&str> = Column::ALL.iter().map(|c| c.example()).collect();
    let body = format!("{}\n{}\n", header.join(","), example.join(","));
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"opensicil-ornek.csv\"",
            ),
        ],
        body,
    )
        .into_response()
}

/// Metni cozup planlar; dosya duzeyinde hata sayfaya bildirim olarak doner.
async fn parse_and_plan(
    state: &AppState,
    op: &Operator,
    csv: &str,
) -> Result<(Table, Plan, usize), Box<Response>> {
    let table = match parse(csv) {
        Ok(table) => table,
        Err(problem) => {
            let text = format!("{} {}", op.lang.t(problem.key), problem.detail);
            let page = render_page(state, op, Notice::err(text.trim().to_string())).await;
            return Err(Box::new(page));
        }
    };
    let threshold = crate::change_set::threshold(&state.pool)
        .await
        .map_err(|e| Box::new(internal("değişiklik seti eşiği okunamadı", e)))?;
    let ownership = ownership_enabled(state).await;
    match plan(
        &state.pool,
        &keys_of(state),
        ownership,
        &op.username,
        &table,
    )
    .await
    {
        Ok(plan) => Ok((table, plan, threshold)),
        Err(e) => Err(Box::new(internal("içe aktarma planlanamadı", e))),
    }
}

fn render_preview(
    op: &Operator,
    csv: String,
    (plan, threshold): (Plan, usize),
    notice: Notice,
) -> Response {
    let will_stage = plan.affected() > threshold;
    let stage_text = op.lang.tn(
        "import.will_stage",
        &[&plan.affected().to_string(), &threshold.to_string()],
    );
    render(&PreviewTemplate {
        lang: op.lang,
        shell: Shell::of(op),
        notice,
        csv,
        summary: Summary::of(&plan),
        rows: plan.rows.into_iter().take(PREVIEW_ROWS).collect(),
        errors: plan.errors,
        duplicates: plan.duplicates,
        renumbers: plan.renumbers,
        will_stage,
        stage_text,
    })
}

async fn preview(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<UploadForm>,
) -> Response {
    if !allowed(&op, AUTHORITIES) {
        return forbidden(op.lang);
    }
    match parse_and_plan(&state, &op, &form.csv).await {
        Ok((_, plan, threshold)) => {
            render_preview(&op, form.csv, (plan, threshold), Notice::default())
        }
        Err(response) => *response,
    }
}

/// "Uygula": hatali dosya onizlemeye doner; mukerrer uyarisi onaysiz gecmez;
/// esigi asan dosya sahnelenir, asmayan hemen uygulanir.
async fn upload(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<UploadForm>,
) -> Response {
    if !allowed(&op, AUTHORITIES) {
        return forbidden(op.lang);
    }
    let (table, plan, threshold) = match parse_and_plan(&state, &op, &form.csv).await {
        Ok(parsed) => parsed,
        Err(response) => return *response,
    };
    let confirmed = Confirmed {
        duplicates: form.confirm_duplicates.is_some(),
        renumber: form.confirm_renumber.is_some(),
    };
    if let Some(key) = blocking_key(&plan, confirmed) {
        let notice = Notice::err(op.lang.t(key).to_string());
        return render_preview(&op, form.csv, (plan, threshold), notice);
    }
    if plan.affected() > threshold {
        let by = (op.subject.as_str(), op.username.as_str(), confirmed);
        let id = match stage(&state.pool, &state.aead_key, &table, plan.affected(), by).await {
            Ok(id) => id,
            Err(e) => return internal("içe aktarma sahnelenemedi", e),
        };
        let detail = serde_json::json!({ "batch_id": id, "rows": table.rows.len(),
            "affected": plan.affected(), "duplicates_confirmed": confirmed.duplicates,
            "renumber_confirmed": confirmed.renumber });
        audit_operator(&state, &op, crate::audit::IMPORT_STAGED, None, detail).await;
        return Redirect::to(&format!("/imports/{id}")).into_response();
    }
    match apply_and_audit(&state, &op, &plan, table.rows.len(), confirmed).await {
        Ok(applied) => {
            let text = op.lang.tn(
                "import.applied",
                &[
                    &applied.created.len().to_string(),
                    &applied.updated.len().to_string(),
                ],
            );
            Notice::info(text).redirect(&state.pool, &op, IMPORTS).await
        }
        Err(e) => internal("içe aktarma uygulanamadı", e),
    }
}

/// Uygulamayi engelleyen durum: hatali satir, onaylanmamis mukerrer uyarisi ya da
/// onaylanmamis sicil no degisimi onerisi.
fn blocking_key(plan: &Plan, confirmed: Confirmed) -> Option<&'static str> {
    if !plan.valid() {
        Some("import.has_errors")
    } else if !plan.duplicates.is_empty() && !confirmed.duplicates {
        Some("import.confirm_needed")
    } else if !plan.renumbers.is_empty() && !confirmed.renumber {
        Some("import.confirm_renumber_needed")
    } else {
        None
    }
}

async fn apply_and_audit(
    state: &AppState,
    op: &Operator,
    plan: &Plan,
    rows: usize,
    confirmed: Confirmed,
) -> Result<Applied, sqlx::Error> {
    let time_zone = state
        .common()
        .await
        .map_err(sqlx::Error::Protocol)?
        .time_zone;
    let applied = apply(&state.pool, &keys_of(state), &time_zone, plan).await?;
    // ADR-018: dosya icerigi degil; kim, ne zaman, kac satir (ve onaylar, ADR-042/055)
    let detail = serde_json::json!({ "rows": rows, "created": applied.created.len(),
        "updated": applied.updated.len(), "cleared": plan.cleared,
        "renumbered": plan.renumbers.len(), "duplicates_confirmed": confirmed.duplicates,
        "renumber_confirmed": confirmed.renumber });
    audit_operator(state, op, crate::audit::IMPORT_APPLIED, None, detail).await;
    // ADR-042 madde 3: sicil no degisimi denetimde once/sonra ile
    let renumbered: HashMap<i64, (&str, &str)> = plan
        .renumbers
        .iter()
        .map(|r| {
            (
                r.identity_id,
                (r.old_number.as_str(), r.new_number.as_str()),
            )
        })
        .collect();
    for (ids, action) in [(&applied.created, "created"), (&applied.updated, "updated")] {
        for id in ids {
            let detail = match renumbered.get(id) {
                Some((before, after)) => serde_json::json!({ "action": action,
                    "employee_number_before": before, "employee_number_after": after }),
                None => serde_json::json!({ "action": action }),
            };
            audit_operator(
                state,
                op,
                crate::audit::IDENTITY_IMPORTED,
                Some(*id),
                detail,
            )
            .await;
        }
    }
    Ok(applied)
}

async fn load_batch(
    state: &AppState,
    op: &Operator,
    id: i64,
) -> Result<(Batch, Plan), Box<Response>> {
    let batch = match pending(&state.pool, &state.aead_key, id).await {
        Ok(Some(batch)) => batch,
        Ok(None) => {
            let notice = Notice::err(op.lang.t("err.import_batch_not_found").to_string());
            return Err(Box::new(render_page(state, op, notice).await));
        }
        Err(e) => return Err(Box::new(internal("içe aktarma partisi okunamadı", e))),
    };
    // ADR-055 madde 1: onay anindaki fark
    match plan(
        &state.pool,
        &keys_of(state),
        ownership_enabled(state).await,
        &op.username,
        &batch.table,
    )
    .await
    {
        Ok(plan) => Ok((batch, plan)),
        Err(e) => Err(Box::new(internal("içe aktarma partisi planlanamadı", e))),
    }
}

fn render_batch(op: &Operator, batch: Batch, plan: Plan, notice: Notice) -> Response {
    let can_approve =
        allowed(op, APPROVE_AUTHORITIES) && blocking_key(&plan, batch.confirmed).is_none();
    let staged = usize::try_from(batch.affected_at_stage).unwrap_or_default();
    let batch_sub = op.lang.tn(
        "import.batch_sub",
        &[&batch.by_username, &(batch.age_seconds / 60).to_string()],
    );
    let changed_text = op.lang.tn(
        "import.changed_since",
        &[&staged.to_string(), &plan.affected().to_string()],
    );
    render(&BatchTemplate {
        lang: op.lang,
        shell: Shell::of(op),
        notice,
        batch_id: batch.id,
        batch_sub,
        changed_since: plan.affected() != staged,
        changed_text,
        confirmed: batch.confirmed,
        summary: Summary::of(&plan),
        rows: plan.rows.into_iter().take(PREVIEW_ROWS).collect(),
        errors: plan.errors,
        duplicates: plan.duplicates,
        renumbers: plan.renumbers,
        can_approve,
        can_reject: allowed(op, APPROVE_AUTHORITIES) || batch.by_subject == op.subject,
    })
}

async fn batch_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match load_batch(&state, &op, id).await {
        Ok((batch, plan)) => render_batch(&op, batch, plan, Notice::default()),
        Err(response) => *response,
    }
}

async fn approve(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, APPROVE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let (batch, plan) = match load_batch(&state, &op, id).await {
        Ok(loaded) => loaded,
        Err(response) => return *response,
    };
    if let Some(key) = blocking_key(&plan, batch.confirmed) {
        let notice = Notice::err(op.lang.t(key).to_string());
        return render_batch(&op, batch, plan, notice);
    }
    let rows = batch.table.rows.len();
    let applied = match apply_and_audit(&state, &op, &plan, rows, batch.confirmed).await {
        Ok(applied) => applied,
        Err(e) => return internal("içe aktarma partisi uygulanamadı", e),
    };
    if let Err(e) = discard(&state.pool, id).await {
        log_error!("web: uygulanan içe aktarma partisi silinemedi: {e}");
    }
    let detail = serde_json::json!({ "batch_id": id, "by": batch.by_username });
    audit_operator(&state, &op, crate::audit::IMPORT_APPROVED, None, detail).await;
    let text = op.lang.tn(
        "import.applied",
        &[
            &applied.created.len().to_string(),
            &applied.updated.len().to_string(),
        ],
    );
    Notice::info(text).redirect(&state.pool, &op, IMPORTS).await
}

/// Red: Sistem yoneticisi ya da partiyi yukleyen; parti silinir, hicbir sey uygulanmaz.
async fn reject(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    let batch = match pending(&state.pool, &state.aead_key, id).await {
        Ok(Some(batch)) => batch,
        Ok(None) => {
            let notice = Notice::err(op.lang.t("err.import_batch_not_found").to_string());
            return render_page(&state, &op, notice).await;
        }
        Err(e) => return internal("içe aktarma partisi okunamadı", e),
    };
    if !allowed(&op, APPROVE_AUTHORITIES) && batch.by_subject != op.subject {
        return forbidden(op.lang);
    }
    if let Err(e) = discard(&state.pool, id).await {
        return internal("içe aktarma partisi silinemedi", e);
    }
    let detail = serde_json::json!({ "batch_id": id, "by": batch.by_username });
    audit_operator(&state, &op, crate::audit::IMPORT_REJECTED, None, detail).await;
    Notice::info(op.lang.t("import.rejected").to_string())
        .redirect(&state.pool, &op, IMPORTS)
        .await
}
// --- END FEATURE: csv-import ---

#[cfg(test)]
mod tests {
    use super::*;

    fn op_lang() -> crate::i18n::Lang {
        crate::i18n::DEFAULT
    }

    fn table_of(text: &str) -> Table {
        parse(text).expect("dosya çözülmeli")
    }

    #[test]
    fn parses_quotes_delimiters_bom_and_turkish_headers() {
        let text =
            "\u{feff}Sicil No;Ad;Soyad;Departman Kodu\r\n\"10;1\";\"Ay\"\"şe\";Yılmaz;BT\r\n\r\n";
        let table = table_of(text);
        assert_eq!(
            table.columns,
            vec![
                Column::EmployeeNumber,
                Column::GivenName,
                Column::Surname,
                Column::DepartmentCode
            ]
        );
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].line, 2);
        assert_eq!(table.rows[0].cells, vec!["10;1", "Ay\"şe", "Yılmaz", "BT"]);

        let comma = table_of("employee_number,given_name\n1,Ali\n2,Veli\n");
        assert_eq!(comma.rows.len(), 2);
        assert_eq!(comma.rows[1].line, 3);
    }

    #[test]
    fn rejects_unknown_duplicate_missing_key_and_ragged() {
        let unknown = parse("employee_number,maas\n1,100\n").unwrap_err();
        assert_eq!(
            (unknown.key, unknown.detail.as_str()),
            ("err.import_unknown_column", "maas")
        );
        let dup = parse("employee_number,ad,given_name\n1,a,b\n").unwrap_err();
        assert_eq!(dup.key, "err.import_duplicate_column");
        let no_key = parse("given_name\nAli\n").unwrap_err();
        assert_eq!(no_key.key, "err.import_no_key_column");
        let ragged = parse("employee_number,given_name\n1,Ali\n2\n").unwrap_err();
        assert_eq!(
            (ragged.key, ragged.detail.as_str()),
            ("err.import_ragged_row", "3")
        );
        assert_eq!(
            parse("employee_number\n").unwrap_err().key,
            "err.import_empty"
        );
        assert_eq!(
            parse("a,\"b\n").unwrap_err().key,
            "err.import_unclosed_quote"
        );
    }

    // OS-12: bos satir ve ayrac yigini kayit kayit bellege alinmaz; sinirlar
    // dosyanin geri kalanini okumadan dosyayi reddeder.
    #[test]
    fn row_and_cell_limits_stop_parsing_early() {
        let full_row = vec!["x"; MAX_CELLS].join(",");
        let wide = format!("employee_number,given_name\n1,a\n{full_row},x\n");
        let err = parse(&wide).unwrap_err();
        assert_eq!(
            (err.key, err.detail.as_str()),
            ("err.import_ragged_row", "3")
        );
        assert_eq!(
            Records::new(&full_row, ',').next(),
            Some(Ok(vec!["x".to_string(); MAX_CELLS]))
        );

        let blanks = format!("employee_number,given_name\n{}1,a\n", "\n".repeat(1 << 20));
        assert_eq!(parse(&blanks).unwrap().rows.len(), 1);

        let mut many = String::from("employee_number,given_name\n");
        for i in 0..=MAX_ROWS {
            many.push_str(&format!("{i},a\n"));
        }
        many.push_str("\"acik tirnak");
        assert_eq!(parse(&many).unwrap_err().key, "err.import_too_many_rows");
    }

    #[test]
    fn normalizes_values() {
        assert_eq!(fold("  Çalışma_Tipi  "), "calisma tipi");
        assert_eq!(employment_type("Kadrolu"), "permanent");
        assert_eq!(employment_type("dış kaynak"), "outsourced");
        assert_eq!(employment_type("başka"), "başka");
        assert_eq!(iso_date("15.01.2026"), "2026-01-15");
        assert_eq!(iso_date("5/1/2026"), "2026-01-05");
        assert_eq!(iso_date("2026-01-15"), "2026-01-15");
        assert_eq!(role_names("VPN; Nöbet |"), vec!["VPN", "Nöbet"]);
        assert_eq!(
            Column::from_header("Yönetici Sicil No"),
            Some(Column::ManagerEmployeeNumber)
        );
        assert_eq!(
            Column::from_header("manager_employee_number"),
            Some(Column::ManagerEmployeeNumber)
        );
        assert_eq!(delimiter("a\tb\tc"), '\t');
    }

    async fn seed(pool: &PgPool) -> (i64, i64, i64) {
        let department: i64 = sqlx::query_scalar(
            "INSERT INTO departments (name, code, slug) VALUES ('Bilgi İşlem', 'BT', 'bt') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let primary: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, slug) VALUES ('primary', 'Sistem Uzmanı', 'sistem-uzmani') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let additional: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, slug) VALUES ('additional', 'VPN', 'vpn') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        (department, primary, additional)
    }

    // Degismez kural (identity::placeholder_refused): CSV yer tutucu rolu taniyamaz;
    // kapi kalkarsa gece dolumu yonetilen hesabin rolunu is acmadan degistirir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_placeholder_role_cannot_be_named_in_a_file() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        seed(&pool).await;
        let text = "employee_number,given_name,surname,department_code,primary_role,\
                    employment_type,start_date\n\
                    3000,Veli,Can,BT,Tanımsız,permanent,2026-01-01\n";
        let plan = plan(&pool, &TEST_KEYS, false, "", &table_of(text))
            .await
            .unwrap();
        assert!(!plan.valid(), "yer tutucu rol dosyada seçilemez");
        assert_eq!(plan.new, 0);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // Guvenlik denetimi OS-02: formun 403'u CSV ile atlanamaz (ADR-005)
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn an_operator_cannot_change_access_on_their_own_record_by_file() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let (department, primary, _) = seed(&pool).await;
        sqlx::query(
            "INSERT INTO identities (given_name, surname, employee_number, username, department_id, \
             primary_role_id, employment_type, start_date) \
             VALUES ('İK', 'Operatörü', 'E1', 'ik.operatoru', $1, $2, 'permanent', current_date)",
        )
        .bind(department)
        .bind(primary)
        .execute(&pool)
        .await
        .unwrap();
        let header = "employee_number,given_name,surname,department_code,primary_role,\
                      additional_roles,employment_type,start_date\n";
        let plan_as = |operator: &'static str, row: &'static str| {
            let pool = pool.clone();
            async move {
                let text = format!("{header}{row}\n");
                plan(&pool, &TEST_KEYS, false, operator, &table_of(&text))
                    .await
                    .unwrap()
            }
        };
        let own_role = "E1,İK,Operatörü,BT,Sistem Uzmanı,VPN,permanent,2026-01-01";
        for operator in ["IK.Operatoru", "ik.operatoru@corp.example"] {
            let refused = plan_as(operator, own_role).await;
            assert_eq!(refused.updated, 0, "{operator}");
            assert_eq!(refused.errors[0].key, "err.import_own_record", "{operator}");
        }
        let other = plan_as("baska.operator", own_role).await;
        assert_eq!((other.updated, other.errors.len()), (1, 0));
        let own_name = "E1,Yeni,Operatörü,BT,Sistem Uzmanı,,permanent,2026-01-01";
        let renamed = plan_as("ik.operatoru", own_name).await;
        assert_eq!(
            (renamed.updated, renamed.errors.len()),
            (1, 0),
            "ad serbest"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    const TEST_KEYS: Keys<'static> = Keys {
        aead: &[7; crate::crypto::KEY_LEN],
        blind_index: &[9; crate::crypto::KEY_LEN],
    };
    const TZ: &str = "Europe/Istanbul";

    /// Yeni + mevcut kimlik tek dosyada: eksik kolon dokunmaz, bos hucre temizler,
    /// ek roller yalnizca tarihsizleri yonetir, yonetici dosyadan baglanir, isler acilir.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn applies_new_and_updated_rows_in_one_transaction() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let (department, primary, additional) = seed(&pool).await;
        let dated: i64 = sqlx::query_scalar(
            "INSERT INTO roles (kind, name, slug) VALUES ('additional', 'Nöbet', 'nobet') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let existing: i64 = sqlx::query_scalar(
            "INSERT INTO identities (given_name, surname, employee_number, mobile_phone, department_id, \
             primary_role_id, employment_type, start_date) \
             VALUES ('Ali', 'Kaya', '2000', '+905551112233', $1, $2, 'permanent', current_date) RETURNING id",
        )
        .bind(department)
        .bind(primary)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) \
             VALUES ($1, $2, current_date + 30), ($1, $3, NULL)",
        )
        .bind(existing)
        .bind(dated)
        .bind(additional)
        .execute(&pool)
        .await
        .unwrap();

        // Ali: telefon temizlenir, ek roller bos (tarihli Nöbet kalir, tarihsiz VPN gider),
        // yoneticisi dosyada acilan Ayşe; Ayşe yeni, VPN ek rolu ile
        let text = "employee_number,given_name,surname,mobile_phone,department_code,primary_role,\
                    additional_roles,manager_employee_number,employment_type,start_date\n\
                    2000,Ali,Kaya,,BT,Sistem Uzmanı,,1000,permanent,2026-01-01\n\
                    1000,Ayşe,Yılmaz,+905321234567,bt,sistem uzmani,VPN,,Kadrolu,15.01.2026\n";
        let table = table_of(text);
        let plan = plan(&pool, &TEST_KEYS, false, "", &table).await.unwrap();
        assert!(plan.valid(), "{:?}", plan.errors);
        assert_eq!((plan.new, plan.updated, plan.unchanged), (1, 1, 0));
        assert_eq!(plan.cleared, 2, "telefon + ek roller");
        assert!(plan.duplicates.is_empty());
        let ali = plan
            .rows
            .iter()
            .find(|r| r.employee_number == "2000")
            .unwrap();
        assert_eq!(ali.action, Action::Update);
        assert!(ali.changes.contains(&"mobile_phone") && ali.changes.contains(&"additional_roles"));
        assert!(
            ali.changes.contains(&"manager_employee_number") && ali.changes.contains(&"start_date")
        );

        let applied = apply(&pool, &TEST_KEYS, TZ, &plan).await.unwrap();
        assert_eq!((applied.created.len(), applied.updated.len()), (1, 1));
        let ayse = applied.created[0];
        let row: (Option<String>, Option<i64>, String) = sqlx::query_as(
            "SELECT mobile_phone, manager_id, to_char(start_date, 'YYYY-MM-DD') FROM identities WHERE id = $1",
        )
        .bind(existing)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, (None, Some(ayse), "2026-01-01".to_string()));
        let ali_roles: Vec<i64> = sqlx::query_scalar(
            "SELECT role_id FROM identity_additional_roles WHERE identity_id = $1 ORDER BY role_id",
        )
        .bind(existing)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            ali_roles,
            vec![dated],
            "tarihli atama korunur, tarihsiz temizlenir"
        );
        let ayse_roles: Vec<i64> = sqlx::query_scalar(
            "SELECT role_id FROM identity_additional_roles WHERE identity_id = $1",
        )
        .bind(ayse)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(ayse_roles, vec![additional]);
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE priority = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 4, "iki kimlik × iki hedef, toplu oncelik");

        // Ikinci kez ayni dosya: hicbir sey degismez
        let again = super::plan(&pool, &TEST_KEYS, false, "", &table)
            .await
            .unwrap();
        assert_eq!((again.new, again.updated, again.unchanged), (0, 0, 2));

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// Kurallar: ipucu zorunlulugu (ADR-023), ayrilmisin bitisi (ADR-030), bilinmeyen
    /// departman/rol/yonetici, cift sicil no, kimlik no baska kimlikte, mukerrer uyarisi.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn rejects_rule_violations_and_warns_possible_duplicates() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let (department, primary, _) = seed(&pool).await;
        let departed: i64 = sqlx::query_scalar(
            "INSERT INTO identities (given_name, surname, employee_number, department_id, \
             primary_role_id, employment_type, start_date, end_at) \
             VALUES ('Eski', 'Personel', '3000', $1, $2, 'permanent', current_date - 100, now() - interval '10 days') \
             RETURNING id",
        )
        .bind(department)
        .bind(primary)
        .fetch_one(&pool)
        .await
        .unwrap();
        let nid = national_id::parse("TR", "10000000146").unwrap();
        national_id::store(&pool, &TEST_KEYS, departed, &nid)
            .await
            .unwrap();
        // kimlik numarasi olmayan ikinci kimlik: baskasinin numarasini alamaz
        sqlx::query(
            "INSERT INTO identities (given_name, surname, employee_number, department_id, \
             primary_role_id, employment_type, start_date) \
             VALUES ('Yok', 'Numara', '3500', $1, $2, 'permanent', current_date - 10)",
        )
        .bind(department)
        .bind(primary)
        .execute(&pool)
        .await
        .unwrap();

        let header = "employee_number,given_name,surname,department_code,primary_role,employment_type,start_date";
        let errors = |text: String| {
            let pool = pool.clone();
            async move {
                let table = table_of(&text);
                let plan = super::plan(&pool, &TEST_KEYS, false, "", &table)
                    .await
                    .unwrap();
                plan.errors
                    .into_iter()
                    .map(|e| (e.line, e.column, e.key))
                    .collect::<Vec<_>>()
            }
        };
        let bad = errors(format!(
            "{header},end_date,manager_employee_number,national_id\n\
             3000,Eski,Personel,BT,Sistem Uzmanı,permanent,2025-01-01,,,\n\
             4000,Yeni,Kişi,YOK,Sistem Uzmanı,permanent,2026-01-01,,,\n\
             4001,Yeni,Kişi,BT,Müdür,permanent,2026-01-01,,,\n\
             4002,Yeni,Kişi,BT,Sistem Uzmanı,permanent,2026-01-01,,9999,\n\
             4002,Yeni,Kişi,BT,Sistem Uzmanı,permanent,2026-01-01,,,\n\
             4003,Yeni,Kişi,BT,Sistem Uzmanı,stajyer,2026-01-01,,,\n\
             4004,Yeni,Kişi,BT,Sistem Uzmanı,permanent,2026-01-01,,,10000000146\n\
             3500,Yok,Numara,BT,Sistem Uzmanı,permanent,2026-01-01,,,10000000146\n"
        ))
        .await;
        // 8: kimlik no ayrilmis 3000'e ait → sicil no degisimi onerisi (ADR-055), ama bos
        // bitis ayrilmisin bitisini bosaltamaz (ADR-030); 9: 3500'un numarasi yok, 3000'inkini alamaz
        assert_eq!(
            bad,
            vec![
                (2, "end_date", "err.import_departed_end"),
                (3, "", "err.import_department_unknown"),
                (4, "", "err.import_role_unknown"),
                (5, "manager_employee_number", "err.import_manager_unknown"),
                (6, "employee_number", "err.import_duplicate_employee_number"),
                (7, "end_date", "err.end_required_non_permanent"),
                (8, "end_date", "err.import_departed_end"),
                (9, "national_id", "err.import_national_id_other"),
            ]
        );
        // sahiplenme acikken ipucusuz yeni satir reddedilir; ipuculu gecer
        let table = table_of(&format!(
            "{header},ad_account_hint\n5000,Ipucu,Yok,BT,Sistem Uzmanı,permanent,2026-01-01,\n\
             5001,Ipucu,Var,BT,Sistem Uzmanı,permanent,2026-01-01,ipucu.var\n"
        ));
        let plan = super::plan(&pool, &TEST_KEYS, true, "", &table)
            .await
            .unwrap();
        assert_eq!(
            plan.errors
                .iter()
                .map(|e| (e.line, e.key))
                .collect::<Vec<_>>(),
            vec![(2, "err.import_hint_required")]
        );
        // ayni ad-soyad, farkli sicil no: hata degil uyari (ADR-042 madde 2)
        let table = table_of(&format!(
            "{header}\n6000,Eski,Personel,BT,Sistem Uzmanı,permanent,2026-01-01\n"
        ));
        let plan = super::plan(&pool, &TEST_KEYS, false, "", &table)
            .await
            .unwrap();
        assert!(plan.valid());
        assert_eq!(plan.duplicates.len(), 1);
        assert_eq!(
            (
                plan.duplicates[0].existing_id,
                plan.duplicates[0].existing_number.as_str()
            ),
            (departed, "3000")
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[test]
    fn blocking_needs_both_confirmations() {
        let mut plan = Plan::default();
        assert_eq!(blocking_key(&plan, Confirmed::default()), None);
        plan.renumbers.push(Renumber {
            line: 2,
            person: "A B".into(),
            identity_id: 1,
            old_number: "1".into(),
            new_number: "2".into(),
        });
        let only_duplicates = Confirmed {
            duplicates: true,
            renumber: false,
        };
        assert_eq!(
            blocking_key(&plan, only_duplicates),
            Some("import.confirm_renumber_needed")
        );
        let both = Confirmed {
            duplicates: true,
            renumber: true,
        };
        assert_eq!(blocking_key(&plan, both), None);
        plan.errors.push(row_error(3, None, "err.x"));
        assert_eq!(blocking_key(&plan, both), Some("import.has_errors"));
    }

    /// ADR-055 madde 3: ayni kimlik no, farkli sicil no → hata degil oneri; uygulaninca
    /// mevcut kimligin sicil nosu degisir, yeni kimlik acilmaz; ayni kimlik no iki satirda hata.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn suggests_employee_number_change_for_a_known_national_id() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let (department, primary, _) = seed(&pool).await;
        let intern: i64 = sqlx::query_scalar(
            "INSERT INTO identities (given_name, surname, employee_number, department_id, \
             primary_role_id, employment_type, start_date, end_at) \
             VALUES ('Stajyer', 'Kadro', 'STJ-7', $1, $2, 'intern', current_date - 200, now() + interval '30 days') \
             RETURNING id",
        )
        .bind(department)
        .bind(primary)
        .fetch_one(&pool)
        .await
        .unwrap();
        let nid = national_id::parse("TR", "10000000146").unwrap();
        national_id::store(&pool, &TEST_KEYS, intern, &nid)
            .await
            .unwrap();

        // kadroya gecis: yeni sicil no, calisma tipi kadrolu, bitis bos (ADR-042 madde 3)
        let text = "employee_number,given_name,surname,national_id,department_code,primary_role,\
                    employment_type,start_date,end_date\n\
                    K-100,Stajyer,Kadro,10000000146,BT,Sistem Uzmanı,permanent,2026-01-01,\n";
        let table = table_of(text);
        let plan = super::plan(&pool, &TEST_KEYS, false, "", &table)
            .await
            .unwrap();
        assert!(plan.valid(), "{:?}", plan.errors);
        assert_eq!((plan.new, plan.updated), (0, 1), "yeni kimlik acilmaz");
        assert_eq!(plan.renumbers.len(), 1);
        let suggestion = &plan.renumbers[0];
        assert_eq!(
            (
                suggestion.identity_id,
                suggestion.old_number.as_str(),
                suggestion.new_number.as_str()
            ),
            (intern, "STJ-7", "K-100")
        );
        let row = &plan.rows[0];
        assert!(
            row.changes.contains(&"employee_number") && row.changes.contains(&"employment_type")
        );
        assert!(
            plan.duplicates.is_empty(),
            "ayni kisi, mukerrer uyarisi yok"
        );
        assert_eq!(
            blocking_key(&plan, Confirmed::default()),
            Some("import.confirm_renumber_needed")
        );

        apply(&pool, &TEST_KEYS, TZ, &plan).await.unwrap();
        let after: (Option<String>, String, Option<i64>) = sqlx::query_as(
            "SELECT employee_number, employment_type, EXTRACT(EPOCH FROM end_at)::bigint \
             FROM identities WHERE id = $1",
        )
        .bind(intern)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            after,
            (Some("K-100".to_string()), "permanent".to_string(), None)
        );
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM identities")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total, 1);

        // ayni kimlik no ile iki satir yine hata
        let twice = table_of(
            "employee_number,given_name,surname,national_id,department_code,primary_role,employment_type,start_date\n\
             K-100,Stajyer,Kadro,10000000146,BT,Sistem Uzmanı,permanent,2026-01-01\n\
             K-101,Baska,Biri,10000000146,BT,Sistem Uzmanı,permanent,2026-01-01\n",
        );
        let plan = super::plan(&pool, &TEST_KEYS, false, "", &twice)
            .await
            .unwrap();
        assert_eq!(
            plan.errors
                .iter()
                .map(|e| (e.line, e.key))
                .collect::<Vec<_>>(),
            vec![(3, "err.import_duplicate_national_id")]
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// HTTP: hr onizler ve uygular; esigi asan dosya sahnelenir, yukleyen onaylayamaz,
    /// baska bir Sistem yoneticisi onaylar ve satirlar yazilir; auditor 403.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn uploads_apply_or_stage_by_threshold_and_approval_needs_admin() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        seed(&pool).await;
        crate::test_support::set_threshold(&pool, 1).await;
        let state = crate::web::test_state(pool.clone(), "https://localhost");
        let app = crate::web::routes().with_state(state);
        let session = |subject: &'static str, authority: &'static str| {
            let pool = pool.clone();
            async move {
                let operator = crate::operator_session::Operator {
                    subject: subject.to_string(),
                    username: subject.to_string(),
                    email: String::new(),
                    authorities: vec![authority.to_string()],
                    auth_source: crate::operator_session::AuthSource::Oidc,
                    lang: crate::i18n::DEFAULT,
                };
                let token = crate::operator_session::create_session(&pool, &operator)
                    .await
                    .unwrap();
                format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME)
            }
        };
        let send = |method: &'static str, uri: String, body: String, cookie: String| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header(header::COOKIE, cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };
        let text = |r: axum::response::Response| async move {
            let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        };
        let encode = |csv: &str| {
            format!(
                "csv={}",
                csv.bytes().map(|b| format!("%{b:02X}")).collect::<String>()
            )
        };
        let one = "employee_number,given_name,surname,department_code,primary_role,employment_type,start_date\n\
                   1,Tek,Satır,BT,Sistem Uzmanı,permanent,2026-01-01\n";
        let two = "employee_number,given_name,surname,department_code,primary_role,employment_type,start_date\n\
                   2,İki,Satır,BT,Sistem Uzmanı,permanent,2026-01-01\n\
                   3,Üç,Satır,BT,Sistem Uzmanı,permanent,2026-01-01\n";

        let auditor = session("auditor-sub", "auditor").await;
        let r = send(
            "POST",
            "/imports/preview".into(),
            encode(one),
            auditor.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let hr = session("hr-sub", "hr").await;
        let page = text(send("GET", "/imports".into(), String::new(), hr.clone()).await).await;
        assert!(
            page.contains("employee_number") && page.contains("/imports/sample.csv"),
            "{page}"
        );
        let preview =
            text(send("POST", "/imports/preview".into(), encode(one), hr.clone()).await).await;
        assert!(preview.contains("Tek Satır"), "{preview}");
        // esik 1: tek kimlik hemen uygulanir
        // ADR-126 madde 1: basari yolu /imports'a yonlendirir, mesaj flash'tan bir kez
        let r = send("POST", "/imports".into(), encode(one), hr.clone()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers()[header::LOCATION], "/imports");
        let applied = text(send("GET", "/imports".into(), String::new(), hr.clone()).await).await;
        let message = op_lang().tn("import.applied", &["1", "0"]);
        assert!(applied.contains(&message), "{applied}");
        let again = text(send("GET", "/imports".into(), String::new(), hr.clone()).await).await;
        assert!(!again.contains(&message), "flash tek kullanımlık");
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM identities WHERE employee_number = '1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);

        // iki kimlik esigi asar: sahnelenir; hr yetkisizdir, baslatan `admin` kendi onaylar (ADR-132)
        let r = send("POST", "/imports".into(), encode(two), hr.clone()).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let location = r.headers()[header::LOCATION].to_str().unwrap().to_string();
        assert!(location.starts_with("/imports/"), "{location}");
        let batch_id: i64 = location.trim_start_matches("/imports/").parse().unwrap();
        let staged: i64 = sqlx::query_scalar("SELECT count(*) FROM import_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(staged, 1);
        let r = send(
            "POST",
            format!("{location}/approve"),
            String::new(),
            hr.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        // ADR-132: partiyi yukleyen ayni ozne `admin` yetkisiyle kendi onayini verir
        let same_admin = session("hr-sub", "admin").await;
        let page =
            text(send("GET", location.clone(), String::new(), same_admin.clone()).await).await;
        assert!(
            page.contains("/approve") && page.contains("Üç Satır"),
            "başlatan kendi partisini onaylayabilir: {page}"
        );
        let r = send(
            "POST",
            format!("{location}/approve"),
            String::new(),
            same_admin.clone(),
        )
        .await;
        assert_eq!(r.headers()[header::LOCATION], "/imports");
        let page =
            text(send("GET", "/imports".into(), String::new(), same_admin.clone()).await).await;
        assert!(
            page.contains(&op_lang().tn("import.applied", &["2", "0"])),
            "{page}"
        );
        let created: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM identities WHERE employee_number IN ('2', '3')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(created, 2);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM import_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "uygulanan parti silinir");
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM audit_log WHERE event_type LIKE 'import.%' ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            events,
            vec![
                "import.applied",
                "import.staged",
                "import.applied",
                "import.approved"
            ]
        );
        let r = send(
            "GET",
            format!("/imports/{batch_id}"),
            String::new(),
            same_admin,
        )
        .await;
        assert_eq!(
            r.status(),
            StatusCode::OK,
            "bulunamayan parti sayfaya bildirimle doner"
        );

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
