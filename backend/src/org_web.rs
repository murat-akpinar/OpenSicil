// --- START FEATURE: role-department-screens ---
// Rol, departman ve hedef sistem ekranlari (ADR-080). Yazma role_admin/admin,
// okuma her operator. Kayit sonrasi etkilenen kimlikler icin toplu is acilir.

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;

use std::collections::HashMap;

use crate::change_set::{self, Draft, Impact, Pending, StagedDefinition};
use crate::desired_state as ds;
use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::operator_session::Operator;
use crate::org::{self, CatalogOptions, Definition, Owner, SaveError, TargetSetting};
use crate::shell::Shell;
use crate::web::{render, AppState};

const WRITE_AUTHORITIES: &[&str] = &["role_admin", "admin"];
// ADR-014/026/031: esigi asan degisiklik setini Sistem yoneticisi onaylar.
const APPROVE_AUTHORITIES: &[&str] = &["admin"];

// Ekranda iki satirdan biri dolu olur: hata ya da kayit sonrasi etki ozeti.
#[derive(Default)]
pub struct Notice {
    pub error: String,
    pub info: String,
}

/// Oturum satirinda bekleyen bildirim: bilgi, hata (ikisi de bos olabilir)
type FlashRow = (Option<String>, Option<String>);

impl Notice {
    pub fn err(text: String) -> Notice {
        Notice {
            error: text,
            ..Notice::default()
        }
    }

    pub fn info(text: String) -> Notice {
        Notice {
            info: text,
            ..Notice::default()
        }
    }

    /// ADR-126: POST cevabinda sayfa basmak F5'te "yeniden gonder" uyarisi
    /// cikariyordu. Cevap 303 olur, mesaj operatorun oturum satirinda bir
    /// sonraki GET'e kadar bekler. Yazilamazsa eylem yine de olmustur: hata
    /// log'a duser, sayfa mesajsiz acilir.
    pub async fn redirect(self, pool: &sqlx::PgPool, op: &Operator, to: &str) -> Response {
        if let Err(e) = self.save(pool, &op.username).await {
            log_error!("web: bildirim oturuma yazılamadı ({}): {e}", op.username);
        }
        Redirect::to(to).into_response()
    }

    async fn save(self, pool: &sqlx::PgPool, username: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE operator_sessions SET flash_info = $2, flash_error = $3 WHERE username = $1",
        )
        .bind(username)
        .bind(Some(self.info).filter(|t| !t.is_empty()))
        .bind(Some(self.error).filter(|t| !t.is_empty()))
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Bekleyen mesaji okur ve ayni ifadede siler — mesaj bir kez gorunur,
    /// kendini tazeleyen sayfada (ADR-126 madde 4) tekrar cikmaz. Eski deger
    /// `RETURNING` ile dondurulur: `UPDATE … RETURNING flash_info` yeni degeri
    /// (NULL) verirdi, bu yuzden satir once CTE'de okunur.
    pub async fn take(pool: &sqlx::PgPool, username: &str) -> Notice {
        let taken: Result<Option<FlashRow>, sqlx::Error> = sqlx::query_as(
            "WITH waiting AS ( \
                 SELECT token_hash, flash_info, flash_error FROM operator_sessions \
                 WHERE username = $1 AND (flash_info IS NOT NULL OR flash_error IS NOT NULL) \
             ) \
             UPDATE operator_sessions s SET flash_info = NULL, flash_error = NULL \
             FROM waiting w WHERE s.token_hash = w.token_hash \
             RETURNING w.flash_info, w.flash_error",
        )
        .bind(username)
        .fetch_optional(pool)
        .await;
        match taken {
            Ok(Some((info, error))) => Notice {
                info: info.unwrap_or_default(),
                error: error.unwrap_or_default(),
            },
            Ok(None) => Notice::default(),
            Err(e) => {
                log_error!("web: bekleyen bildirim okunamadı ({username}): {e}");
                Notice::default()
            }
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/roles", get(roles_page).post(create_role))
        .route("/roles/{id}", get(role_page).post(save_role))
        .route("/roles/{id}/approve", post(approve_role))
        .route("/roles/{id}/reject", post(reject_role))
        .route(
            "/departments",
            get(departments_page).post(create_department),
        )
        .route(
            "/departments/{id}",
            get(department_page).post(save_department),
        )
        .route("/departments/{id}/approve", post(approve_department))
        .route("/departments/{id}/reject", post(reject_department))
        .route("/targets", get(targets_page))
        .route("/targets/{id}", post(save_target))
        .route("/targets/{id}/catalog-refresh", post(refresh_catalog))
        .route("/catalog/{id}/hand-over", post(hand_over_missing))
        .route("/catalog/{id}/remove", post(remove_missing))
}

// Tekrar eden alanlar (entitlement) ve hedef basina ayar alanlari (pa.<hedef> ...).
pub(crate) struct Fields(pub Vec<(String, String)>);

impl Fields {
    pub(crate) fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    pub(crate) fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub(crate) fn all_i64(&self, key: &str) -> Vec<i64> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .filter_map(|(_, v)| v.parse().ok())
            .collect()
    }

    pub(crate) fn opt_i64(&self, key: &str) -> Option<i64> {
        self.get(key).trim().parse().ok()
    }

    fn settings(&self, targets: &[TargetSetting]) -> Vec<TargetSetting> {
        targets
            .iter()
            .map(|t| TargetSetting {
                target_id: t.target_id,
                target_name: t.target_name.clone(),
                provision_account: match self.get(&format!("pa.{}", t.target_id)) {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
                container_item_id: self.opt_i64(&format!("ct.{}", t.target_id)),
                email_domain: self.get(&format!("ed.{}", t.target_id)).to_string(),
                upn_suffix: self.get(&format!("us.{}", t.target_id)).to_string(),
            })
            .collect()
    }
}

#[derive(Clone)]
struct ItemView {
    id: i64,
    label: String,
    selected: bool,
}

/// Onay kutulari onek basina katlanir gruplara ayrilir: 25 kutuyu tek sutunda
/// dizmek "nereye ekleyecegim" sorusunu cevapsiz birakiyordu.
struct MembershipGroup {
    /// Katalog adinin son parcasindan onceki kismi (`GG-Course-Charms` → `GG-Course`)
    title: String,
    items: Vec<ItemView>,
    selected: usize,
    /// Secili oge tasiyan grup acik gelir
    open: bool,
}

struct TargetView {
    target_id: i64,
    target_name: String,
    groups: Vec<MembershipGroup>,
    /// Katalogda hic uyelik ogesi yok mu
    empty: bool,
    containers: Vec<ItemView>,
    provision: &'static str,
    email_domain: String,
    upn_suffix: String,
    /// "Su an gecerli" satirlari: dort tek degerli ayarin cozulmus degeri ve
    /// hangi kaynaktan geldigi (ADR-017 oncelik sirasi)
    effective: Vec<Effective>,
}

/// Bir tek degerli ayarin cozulmus hali; `desired_state::resolve_single_valued`
/// ne dondurduyse o — ekran ikinci bir oncelik algoritmasi calistirmaz.
struct Effective {
    /// i18n anahtari: `def.provision` / `def.container` / …
    label: &'static str,
    value: String,
    /// Degerin nereden geldigini soyleyen hazir cumle ("Teachers tanimindan",
    /// "bu tanimdan", "hedef sistem varsayilani")
    from: String,
}

/// En az bu kadar ogesi olan onek kendi grubunu acar; altinda kalanlar
/// "diger" grubunda toplanir — tek elemanli on bir baslik liste olmaz.
const MIN_GROUP: usize = 2;

/// Uyelik etiketinde yalnizca ad: grup adlari tekil, DN'i yanina yazmak 25
/// satirlik bir duvar uretiyordu. Konteynerde DN sart — ayni dizinde on bir
/// farkli `OU=Users` olabilir, yalnizca ad hangisi oldugunu soylemez.
fn item_label(c: &org::CatalogChoice, lang: Lang) -> String {
    let mut label = c.display_name.clone();
    let container = c.kind != "group" && c.kind != "list";
    if container && !c.location.is_empty() && c.location != c.display_name {
        label = c.location.clone();
    }
    if c.missing {
        label.push_str(lang.t("catalog.missing"));
    }
    label
}

/// `GG-Course-Charms` → `GG-Course`; ayirici yoksa bos (gruplanmaz).
fn name_prefix(name: &str) -> &str {
    match name.rfind('-') {
        Some(i) if i > 0 => &name[..i],
        _ => "",
    }
}

fn group_memberships(items: Vec<(String, ItemView)>, other: &'static str) -> Vec<MembershipGroup> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (name, _) in &items {
        *counts.entry(name_prefix(name)).or_default() += 1;
    }
    let mut groups: Vec<MembershipGroup> = Vec::new();
    for (name, item) in &items {
        let prefix = name_prefix(name);
        let title = match counts.get(prefix) {
            Some(n) if *n >= MIN_GROUP && !prefix.is_empty() => prefix,
            _ => other,
        };
        match groups.iter_mut().find(|g| g.title == title) {
            Some(g) => g.items.push(ItemView { ..item.clone() }),
            None => groups.push(MembershipGroup {
                title: title.to_string(),
                items: vec![item.clone()],
                selected: 0,
                open: false,
            }),
        }
    }
    for g in &mut groups {
        g.selected = g.items.iter().filter(|i| i.selected).count();
        g.open = g.selected > 0;
    }
    // "Diger" en sona: adlandirilmis gruplar once okunsun.
    groups.sort_by_key(|g| (g.title == other, g.title.clone()));
    groups
}

/// Tek degerli ayarlarin kaynaklari: tanimin kendisi + ust departmanlar +
/// hedef varsayilani. Bos gelirse ekran yalnizca "su an gecerli" satirini
/// atlar, form calismaya devam eder.
struct Chain {
    owner: Owner,
    sources: Vec<org::SettingSource>,
    targets: Vec<org::TargetRow>,
}

async fn load_chain(state: &AppState, owner: Owner, id: i64) -> Result<Chain, sqlx::Error> {
    Ok(Chain {
        owner,
        sources: org::setting_sources(&state.pool, owner, id).await?,
        targets: org::list_targets(&state.pool).await?,
    })
}

impl Chain {
    fn rows(&self, target: i64, options: &CatalogOptions, lang: Lang) -> Vec<Effective> {
        let Some(defaults) = self.targets.iter().find(|t| t.id == target) else {
            return Vec::new();
        };
        let chain: Vec<&org::SettingSource> = self
            .sources
            .iter()
            .filter(|s| s.target_id == target)
            .collect();
        effective_rows(self.owner, &chain, defaults, options, lang)
    }
}

/// Hedefin katalog ogeleri ekran satiri olarak; `pick` secili olani soyler.
fn item_views(
    list: &[org::CatalogChoice],
    target: i64,
    lang: Lang,
    pick: impl Fn(i64) -> bool,
) -> Vec<ItemView> {
    list.iter()
        .filter(|c| c.target_id == target && offered(c, &pick))
        .map(|c| ItemView {
            id: c.id,
            label: item_label(c, lang),
            selected: pick(c.id),
        })
        .collect()
}

/// ADR-127 madde 6: kayip oge yeni secenek olarak sunulmaz; tanimda zaten
/// isaretliyse "(kayıp)" etiketiyle kalir (operator kaldirabilsin diye).
fn offered(c: &org::CatalogChoice, pick: impl Fn(i64) -> bool) -> bool {
    !c.missing || pick(c.id)
}

fn target_views(
    def: &Definition,
    options: &CatalogOptions,
    chain: &Chain,
    lang: Lang,
) -> Vec<TargetView> {
    let named = |target: i64| {
        let selected = |id| def.entitlement_ids.contains(&id);
        let names = options
            .memberships
            .iter()
            .filter(|c| c.target_id == target && offered(c, selected))
            .map(|c| c.display_name.clone());
        names
            .zip(item_views(&options.memberships, target, lang, selected))
            .collect::<Vec<_>>()
    };
    def.settings
        .iter()
        .map(|s| {
            let named = named(s.target_id);
            TargetView {
                target_id: s.target_id,
                target_name: s.target_name.clone(),
                empty: named.is_empty(),
                groups: group_memberships(named, lang.t("def.other_group")),
                containers: item_views(&options.containers, s.target_id, lang, |id| {
                    s.container_item_id == Some(id)
                }),
                provision: match s.provision_account {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "",
                },
                email_domain: s.email_domain.clone(),
                upn_suffix: s.upn_suffix.clone(),
                effective: chain.rows(s.target_id, options, lang),
            }
        })
        .collect()
}

/// Kaynak satirlarini `desired_state`'in bekledigi modele cevirir. Departman
/// sayfasinda zincir departmanlardan, rol sayfasinda tek satir birincil
/// rolden gelir; sira **burada** kurulur, oncelik `resolve_single_valued`'da.
fn model_of(owner: Owner, chain: &[&org::SettingSource], target: &org::TargetRow) -> ds::Model {
    let source = |s: &org::SettingSource| ds::Source {
        entitlements: Vec::new(),
        settings: ds::SingleValued {
            provision_account: s.provision,
            container: s.container_item_id,
            email_domain: s.email_domain.clone(),
            upn_suffix: s.upn_suffix.clone(),
        },
    };
    let (primary, departments) = match owner {
        Owner::Role => (chain.first().map(|s| source(s)).unwrap_or_default(), vec![]),
        Owner::Department => (
            ds::Source::default(),
            chain.iter().map(|s| source(s)).collect(),
        ),
    };
    ds::Model {
        base_entitlements: Vec::new(),
        department_chain: departments,
        primary_role: primary,
        title: None,
        additional_roles: Vec::new(),
        target: ds::TargetDefaults {
            provision_account: target.provision_account_default,
            container: target.default_container_item_id,
            retention_days: 0,
            delete_requires_approval: false,
            password_reset_delay_days: 0,
        },
    }
}

/// Zincirdeki ilk dolu satir — `resolve_single_valued` ile **ayni sirayi**
/// okur (`model_of` kurdu); test ikisinin ayni degeri verdigini kilitler.
fn winner<'a, T>(
    chain: &[&'a org::SettingSource],
    pick: impl Fn(&org::SettingSource) -> Option<T>,
) -> Option<&'a org::SettingSource> {
    chain.iter().copied().find(|s| pick(s).is_some())
}

/// "Su an gecerli" satiri. Kaynak cumlesi burada kuruluyor: sablon yalnizca
/// basiyor, "hedef sistem varsayilani tanimindan" gibi bir birlesim cikmasin.
fn effective(
    lang: Lang,
    label: &'static str,
    value: Option<String>,
    from: Option<&org::SettingSource>,
) -> Effective {
    Effective {
        label,
        value: value.unwrap_or_else(|| lang.t("def.nobody_says").to_string()),
        from: match from {
            Some(s) if s.is_self => lang.t("def.from_self").to_string(),
            Some(s) => lang.t1("def.from", &s.label),
            None => lang.t("def.from_target_default").to_string(),
        },
    }
}

fn yes_no(lang: Lang, v: bool) -> String {
    lang.t(match v {
        true => "common.yes",
        false => "common.no",
    })
    .to_string()
}

fn effective_rows(
    owner: Owner,
    chain: &[&org::SettingSource],
    target: &org::TargetRow,
    options: &CatalogOptions,
    lang: Lang,
) -> Vec<Effective> {
    let resolved = ds::resolve_single_valued(&model_of(owner, chain, target));
    let container_name = |id: Option<i64>| {
        let id = id?;
        options
            .containers
            .iter()
            .find(|c| c.id == id)
            .map(|c| item_label(c, lang))
    };
    // `desired_state::provision_expected` ile ayni dususu yapar: kimse
    // soylemezse hedefin varsayilani gecerli olur.
    let provision = resolved
        .provision_account
        .unwrap_or(target.provision_account_default);
    vec![
        effective(
            lang,
            "def.provision",
            Some(yes_no(lang, provision)),
            winner(chain, |s| s.provision),
        ),
        effective(
            lang,
            "def.container",
            container_name(resolved.container),
            winner(chain, |s| s.container_item_id),
        ),
        effective(
            lang,
            "def.email_domain",
            resolved.email_domain.clone(),
            winner(chain, |s| s.email_domain.clone()),
        ),
        effective(
            lang,
            "def.upn_suffix",
            resolved.upn_suffix.clone(),
            winner(chain, |s| s.upn_suffix.clone()),
        ),
    ]
}

fn definition_edit(f: &Fields, def: &Definition) -> org::DefinitionEdit {
    org::DefinitionEdit {
        name: f.get("name").to_string(),
        entitlement_ids: f.all_i64("entitlement"),
        settings: f.settings(&def.settings),
    }
}

fn settings_json(settings: &[TargetSetting]) -> serde_json::Value {
    settings
        .iter()
        .map(|s| {
            serde_json::json!({
                "target_id": s.target_id,
                "provision_account": s.provision_account,
                "container_item_id": s.container_item_id,
                "email_domain": s.email_domain,
                "upn_suffix": s.upn_suffix,
            })
        })
        .collect()
}

// Kutu: clippy result_large_err (Response buyuk); hata yolu sicak degil.
fn save_error(e: SaveError, what: &str) -> Result<&'static str, Box<Response>> {
    match e {
        SaveError::Invalid(key) => Ok(key),
        SaveError::Db(e) => Err(Box::new(internal(what, e))),
    }
}

#[derive(Template)]
#[template(path = "roles.html")]
struct RolesTemplate {
    shell: Shell,
    lang: Lang,
    sections: Vec<org::RoleSection>,
    /// Hic rol yoksa ekran bolum tablolari yerine bos durumu basar
    any: bool,
    kinds: &'static [&'static str],
    /// ADR-119 A.6: `?new=<tur>` ile gelindiyse form acik ve bu tur secili
    /// basilir. Izinli listeden gecer (`ROLE_KINDS`), bos = form kapali.
    new_kind: &'static str,
    error: String,
    can_edit: bool,
}

impl RolesTemplate {
    /// Secili tur karsilastirmasi Rust tarafinda durur: askama'nin ifade
    /// ayristiricisi `*k` yazamiyor, `k.to_string()` ise clippy'nin
    /// `cmp_owned`una takiliyor.
    fn kind_selected(&self, kind: &str) -> bool {
        self.new_kind == kind
    }
}

#[derive(Template)]
#[template(path = "role.html")]
struct RoleTemplate {
    shell: Shell,
    lang: Lang,
    role: org::RoleDetail,
    targets: Vec<TargetView>,
    show_settings: bool,
    error: String,
    info: String,
    pending: PendingView,
    can_edit: bool,
}

#[derive(Template)]
#[template(path = "departments.html")]
struct DepartmentsTemplate {
    shell: Shell,
    lang: Lang,
    departments: Vec<org::DepartmentRow>,
    /// Ozet kutulari (ADR-119 B.7): toplam departman, agactaki kisi, bos departman
    total: i64,
    people: i64,
    empty: i64,
    error: String,
    can_edit: bool,
}

#[derive(Template)]
#[template(path = "department.html")]
struct DepartmentTemplate {
    shell: Shell,
    lang: Lang,
    dept: org::DepartmentDetail,
    parents: Vec<ItemView>,
    targets: Vec<TargetView>,
    error: String,
    info: String,
    pending: PendingView,
    can_edit: bool,
}

// Bekleyen taslak paneli (ADR-031/026). Bos `summary` = taslak yok.
#[derive(Default)]
struct PendingView {
    summary: String,
    note: String,
    approvable: bool,
}

async fn pending_view(state: &AppState, owner: Owner, id: i64, op: &Operator) -> PendingView {
    let lang = op.lang;
    let pending = match change_set::pending(&state.pool, owner, id).await {
        Ok(Some(p)) => p,
        Ok(None) => return PendingView::default(),
        Err(e) => {
            log_error!("web: bekleyen taslak okunamadı: {e}");
            return PendingView::default();
        }
    };
    let approvable = allowed(op, APPROVE_AUTHORITIES);
    let now = recompute(state, owner, id, &pending.definition).await;
    PendingView {
        summary: summary_text(lang, &pending, &now),
        note: pending_note(lang, &pending, approvable),
        approvable,
    }
}

// ADR-055 madde 1: fark ONAY ANINDA yeniden hesaplanir; taslak beklerken role
// atanan kimlikler sayiya girer. Yayimlanan, onay anindaki farktir.
async fn recompute(
    state: &AppState,
    owner: Owner,
    id: i64,
    staged: &StagedDefinition,
) -> Option<Impact> {
    let time_zone = match state.common().await {
        Ok(common) => common.time_zone,
        Err(e) => {
            log_error!("web: onay anında ortak ayarlar okunamadı: {e}");
            return None;
        }
    };
    match change_set::preview(&state.pool, &time_zone, &staged.as_draft(owner, id)).await {
        Ok(impact) => Some(impact),
        Err(e) => {
            log_error!("web: onay anında etki yeniden hesaplanamadı: {e}");
            None
        }
    }
}

// Sayi taslak kaydedildigindekinden farkliysa ekran bunu soyler (ADR-055).
fn summary_text(lang: Lang, pending: &Pending, now: &Option<Impact>) -> String {
    let staged = &pending.definition;
    let (applies, observed) = match now {
        Some(impact) => (impact.applies, impact.observed),
        None => (staged.applies, staged.observed),
    };
    let text = lang.tn(
        "changeset.pending_summary",
        &[&applies.to_string(), &observed.to_string()],
    );
    if applies == staged.applies {
        return text;
    }
    let changed = lang.tn(
        "changeset.changed_since",
        &[&staged.applies.to_string(), &applies.to_string()],
    );
    format!("{text}; {changed}")
}

// Kim baslatti, ne zaman; yetkisi olmayan operatore onayi kimin verdigi (F-12).
fn pending_note(lang: Lang, pending: &Pending, approvable: bool) -> String {
    let who = lang.tn(
        "changeset.pending_by",
        &[
            &pending.by_username,
            &(pending.age_seconds / 3600).to_string(),
        ],
    );
    if approvable {
        return who;
    }
    format!(
        "{who}; {}",
        lang.t1(
            "changeset.approver_group",
            crate::oidc::group_for(APPROVE_AUTHORITIES[0])
        )
    )
}

// Ust departman secenekleri: kendisi haric (dongu dogrulamasi yine de sunucuda).
fn parent_options(dept: &org::DepartmentDetail, all: &[org::DepartmentRow]) -> Vec<ItemView> {
    all.iter()
        .filter(|d| d.id != dept.def.id)
        .map(|d| ItemView {
            id: d.id,
            label: format!("{}{}", d.indent, d.name),
            selected: dept.parent_id == Some(d.id),
        })
        .collect()
}

struct TargetFormView {
    target: org::TargetRow,
    containers: Vec<ItemView>,
    /// Son katalog yenileme (okuma şeridi, ADR-051): durum anahtarı + zaman + sonuç
    refresh_status: String,
    refresh_at: String,
    refresh_result: String,
}

impl TargetFormView {
    /// ADR-126: yenileme kuyrukta ya da çalışıyor — rozet döner, sayfa kendini
    /// tazeler, iş bitince sonuç metni kendiliğinden görünür
    fn refreshing(&self) -> bool {
        org::read_job_open(&self.refresh_status)
    }
}

#[derive(Template)]
#[template(path = "targets.html")]
struct TargetsTemplate {
    shell: Shell,
    lang: Lang,
    targets: Vec<TargetFormView>,
    error: String,
    info: String,
    can_edit: bool,
    /// ADR-127: kayip katalog ogeleri ve onlara bakan tanimlar
    missing: Vec<crate::catalog_exit::Missing>,
}

impl TargetsTemplate {
    fn missing_of(&self, target: &i64) -> Vec<&crate::catalog_exit::Missing> {
        self.missing
            .iter()
            .filter(|m| m.target_id == *target)
            .collect()
    }
}

/// `?new=` degeri izinli listeden gecer: ekrana yalnizca `ROLE_KINDS`'teki bir
/// tur girer, kullanici metni formun `selected`ina hic ulasmaz.
fn role_kind_of(requested: Option<&str>) -> &'static str {
    let Some(requested) = requested else {
        return "";
    };
    org::ROLE_KINDS
        .iter()
        .copied()
        .find(|kind| *kind == requested)
        .unwrap_or("")
}

async fn render_roles(
    state: &AppState,
    op: &Operator,
    error: String,
    new_kind: &'static str,
) -> Response {
    match org::list_roles(&state.pool).await {
        Ok(roles) => render(&RolesTemplate {
            lang: op.lang,
            shell: Shell::of(op),
            any: !roles.is_empty(),
            sections: org::role_sections(roles),
            kinds: &org::ROLE_KINDS,
            new_kind,
            error,
            can_edit: allowed(op, WRITE_AUTHORITIES),
        }),
        Err(e) => internal("roller okunamadı", e),
    }
}

#[derive(serde::Deserialize)]
struct RolesQuery {
    new: Option<String>,
}

async fn roles_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Query(q): Query<RolesQuery>,
) -> Response {
    render_roles(&state, &op, String::new(), role_kind_of(q.new.as_deref())).await
}

async fn create_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let f = Fields(form);
    match org::create_role(&state.pool, f.get("kind"), f.get("name"), f.get("title")).await {
        Ok(id) => {
            let detail = serde_json::json!({ "action": "created", "role_id": id, "kind": f.get("kind"), "name": f.get("name") });
            audit_operator(&state, &op, crate::audit::ROLE_CHANGED, None, detail).await;
            Redirect::to(&address(&state, Owner::Role, id).await).into_response()
        }
        Err(e) => match save_error(e, "rol oluşturulamadı") {
            // Hata halinde form acik kalir: operator yazdigi turu yeniden secmesin
            Ok(key) => {
                let kind = role_kind_of(Some(f.get("kind")));
                render_roles(&state, &op, op.lang.t(key).to_string(), kind).await
            }
            Err(response) => *response,
        },
    }
}

async fn render_role(state: &AppState, op: &Operator, id: i64, notice: Notice) -> Response {
    let (role, options) = match (
        org::load_role(&state.pool, id).await,
        org::catalog_options(&state.pool).await,
    ) {
        (Ok(Some(role)), Ok(options)) => (role, options),
        (Ok(None), _) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.role_not_found")).into_response()
        }
        (Err(e), _) | (_, Err(e)) => return internal("rol okunamadı", e),
    };
    let chain = match load_chain(state, Owner::Role, id).await {
        Ok(chain) => chain,
        Err(e) => return internal("ayar zinciri okunamadı", e),
    };
    render(&RoleTemplate {
        lang: op.lang,
        shell: Shell::of(op),
        targets: target_views(&role.def, &options, &chain, op.lang),
        show_settings: role.kind == "primary",
        role,
        error: notice.error,
        info: notice.info,
        pending: pending_view(state, Owner::Role, id, op).await,
        can_edit: allowed(op, WRITE_AUTHORITIES),
    })
}

/// Tanimin adresi: slug varsa `/roles/sistem-uzmani`, yoksa id (ADR-107).
async fn address(state: &AppState, owner: Owner, id: i64) -> String {
    let base = match owner {
        Owner::Role => "/roles",
        Owner::Department => "/departments",
    };
    let key = match org::resolve(&state.pool, owner, &id.to_string()).await {
        Ok(Some(found)) => org::address_key(&found.slug, found.id),
        _ => id.to_string(),
    };
    format!("{base}/{key}")
}

/// Adres anahtarini (sayisal id ya da slug) tanima cevirir; yoksa 404. `GET`
/// sayisal adresi slug'a kalici yonlendirir (ADR-107 madde 5), `POST` yonlendirmez.
async fn resolve_key(
    state: &AppState,
    op: &Operator,
    owner: Owner,
    key: &str,
    redirect_numeric: bool,
) -> Result<i64, Box<Response>> {
    let (not_found, base) = match owner {
        Owner::Role => ("err.role_not_found", "/roles"),
        Owner::Department => ("err.department_not_found", "/departments"),
    };
    match org::resolve(&state.pool, owner, key).await {
        // 301: axum'un `Redirect::permanent`i 308 verir; eski bağlantılar GET'tir ve
        // yer imi/tarayıcı için 301 yerleşik beklenti (ADR-107 madde 5)
        Ok(Some(found)) if redirect_numeric && found.by_id && !found.slug.is_empty() => {
            let location = format!("{base}/{}", found.slug);
            Err(Box::new(
                (
                    StatusCode::MOVED_PERMANENTLY,
                    [(axum::http::header::LOCATION, location)],
                )
                    .into_response(),
            ))
        }
        Ok(Some(found)) => Ok(found.id),
        Ok(None) => Err(Box::new(
            (StatusCode::NOT_FOUND, op.lang.t(not_found)).into_response(),
        )),
        Err(e) => Err(Box::new(internal("tanım çözümlenemedi", e))),
    }
}

async fn role_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    match resolve_key(&state, &op, Owner::Role, &key, true).await {
        Ok(id) => {
            let notice = Notice::take(&state.pool, &op.username).await;
            render_role(&state, &op, id, notice).await
        }
        Err(response) => *response,
    }
}

async fn save_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let id = match resolve_key(&state, &op, Owner::Role, &key, false).await {
        Ok(id) => id,
        Err(response) => return *response,
    };
    let current = match org::load_role(&state.pool, id).await {
        Ok(Some(role)) => role,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.role_not_found")).into_response()
        }
        Err(e) => return internal("rol okunamadı", e),
    };
    let f = Fields(form);
    stage_or_publish(
        &state,
        Submission {
            owner: Owner::Role,
            id,
            edit: definition_edit(&f, &current.def),
            title: f.get("title").to_string(),
            code: String::new(),
            parent_id: None,
            with_settings: current.kind == "primary",
            op: &op,
        },
    )
    .await
}

// Bir kaydin tanimi: tur ozel alanlar + yetki ogeleri; sahneleme ve yayimlama
// yolu ikisi icin de aynidir.
struct Submission<'a> {
    owner: Owner,
    id: i64,
    edit: org::DefinitionEdit,
    title: String,
    code: String,
    parent_id: Option<i64>,
    /// ADR-007: tek degerli ayar yalnizca birincil rolde ve departmanda yazilir.
    with_settings: bool,
    op: &'a Operator,
}

// ADR-031: esigi asan duzenleme MODELE YAZILMAZ, taslak olarak bekler; altindaysa
// kaydedilirken yayimlanir ve isler acilir. Taslak gecerli olmali (onay onu uygular).
async fn stage_or_publish(state: &AppState, sub: Submission<'_>) -> Response {
    let (op, owner, id) = (sub.op, sub.owner, sub.id);
    let draft = Draft {
        owner,
        id,
        edit: &sub.edit,
        with_settings: sub.with_settings,
    };
    let threshold = match change_set::threshold(&state.pool).await {
        Ok(t) => t,
        Err(e) => return internal("değişiklik seti eşiği okunamadı", e),
    };
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let (impact, info) = impact_notice(state, op.lang, &draft, (threshold, &time_zone)).await;
    if let Err(e) = org::validate_definition(&state.pool, owner, id, &sub.edit, sub.parent_id).await
    {
        return match save_error(e, "tanım doğrulanamadı") {
            Ok(key) => {
                render_definition(state, op, owner, id, Notice::err(op.lang.t(key).into())).await
            }
            Err(response) => *response,
        };
    }
    if impact.exceeds(threshold) {
        return stage(state, sub, impact, info).await;
    }
    publish(state, sub, impact, Notice::info(info)).await
}

async fn stage(state: &AppState, sub: Submission<'_>, impact: Impact, info: String) -> Response {
    let (op, owner, id) = (sub.op, sub.owner, sub.id);
    let staged = StagedDefinition {
        edit: sub.edit,
        title: sub.title,
        code: sub.code,
        parent_id: sub.parent_id,
        with_settings: sub.with_settings,
        applies: impact.applies,
        observed: impact.observed,
    };
    let by = (op.subject.as_str(), op.username.as_str());
    if let Err(e) = change_set::stage(&state.pool, owner, id, &staged, by).await {
        return internal("taslak kaydedilemedi", e);
    }
    let detail = serde_json::json!({
        "action": "staged", "id": id, "name": staged.edit.name,
        "impact": impact.applies, "observed": impact.observed,
    });
    audit_operator(state, op, event_of(owner), None, detail).await;
    let notice = Notice::info(format!("{}; {info}", op.lang.t("changeset.staged")));
    redirect_definition(state, op, owner, id, notice).await
}

// Taslak yayimlanir: model yazilir, bekleyen taslak silinir, isler acilir.
async fn publish(
    state: &AppState,
    sub: Submission<'_>,
    impact: Impact,
    notice: Notice,
) -> Response {
    let (op, owner, id) = (sub.op, sub.owner, sub.id);
    let saved = match owner {
        Owner::Role => org::save_role(&state.pool, id, &sub.title, &sub.edit).await,
        Owner::Department => {
            org::save_department(&state.pool, id, &sub.code, sub.parent_id, &sub.edit).await
        }
    };
    if let Err(e) = saved {
        return match save_error(e, "tanım kaydedilemedi") {
            Ok(key) => {
                render_definition(state, op, owner, id, Notice::err(op.lang.t(key).into())).await
            }
            Err(response) => *response,
        };
    }
    if let Err(e) = change_set::clear(&state.pool, owner, id).await {
        log_error!("web: bekleyen taslak temizlenemedi: {e}");
    }
    let detail = serde_json::json!({
        "action": "saved", "id": id, "name": sub.edit.name, "code": sub.code,
        "parent_id": sub.parent_id, "entitlement_ids": sub.edit.entitlement_ids,
        "settings": settings_json(&sub.edit.settings),
        "impact": impact.applies, "observed": impact.observed,
    });
    audit_operator(state, op, event_of(owner), None, detail).await;
    enqueue_affected(state, owner, id).await;
    redirect_definition(state, op, owner, id, notice).await
}

/// ADR-126 madde 1: basari yolu sayfa basmaz, tanimin GET adresine yonlendirir;
/// mesaj flash'tan bir kez basilir. Dogrulama hatasi formu yerinde gosterir.
async fn redirect_definition(
    state: &AppState,
    op: &Operator,
    owner: Owner,
    id: i64,
    notice: Notice,
) -> Response {
    let to = address(state, owner, id).await;
    notice.redirect(&state.pool, op, &to).await
}

fn event_of(owner: Owner) -> &'static str {
    match owner {
        Owner::Role => crate::audit::ROLE_CHANGED,
        Owner::Department => crate::audit::DEPARTMENT_CHANGED,
    }
}

async fn render_definition(
    state: &AppState,
    op: &Operator,
    owner: Owner,
    id: i64,
    notice: Notice,
) -> Response {
    match owner {
        Owner::Role => render_role(state, op, id, notice).await,
        Owner::Department => render_department(state, op, id, notice).await,
    }
}

// ADR-031/037/043: etki onizlemesi kayitla birlikte hesaplanir (taslak = gonderilen
// form, yayimlanmis = veritabanindaki tanim) ve ekranda ozetlenir. Esigi asan setin
// onaya dusmesi sonraki kutucuktur; burada sayi raporlanir.
async fn impact_notice(
    state: &AppState,
    lang: Lang,
    draft: &Draft<'_>,
    (threshold, time_zone): (usize, &str),
) -> (Impact, String) {
    match change_set::preview(&state.pool, time_zone, draft).await {
        Ok(impact) => {
            let text = impact_text(lang, &impact, threshold);
            (impact, text)
        }
        Err(e) => {
            log_error!("web: etki önizlemesi hesaplanamadı: {e}");
            (Impact::default(), String::new())
        }
    }
}

fn impact_text(lang: Lang, impact: &Impact, threshold: usize) -> String {
    if impact.applies == 0 && impact.observed == 0 {
        return lang.t("changeset.none").to_string();
    }
    let mut parts = vec![lang.t1("changeset.applies", impact.applies)];
    for item in &impact.items {
        if item.added > 0 {
            parts.push(lang.tn("changeset.added", &[&item.name, &item.added.to_string()]));
        }
        if item.removed > 0 {
            parts.push(lang.tn(
                "changeset.removed",
                &[&item.name, &item.removed.to_string()],
            ));
        }
    }
    if impact.account_changes > 0 {
        parts.push(lang.t1("changeset.accounts", impact.account_changes));
    }
    if impact.observed > 0 {
        parts.push(lang.t1("changeset.observed", impact.observed));
    }
    if impact.exceeds(threshold) {
        parts.push(lang.tn(
            "changeset.over_threshold",
            &[&impact.applies.to_string(), &threshold.to_string()],
        ));
    }
    parts.join("; ")
}

// Is acilamazsa model yine kaydedilmistir; log'a duser, zamanlayici farki yakalar.
async fn enqueue_affected(state: &AppState, owner: Owner, id: i64) {
    match org::enqueue_affected(&state.pool, owner, id).await {
        Ok(n) if n > 0 => log_info!("web: tanım değişti, {n} kimlik için iş açıldı"),
        Ok(_) => {}
        Err(e) => log_error!("web: etkilenen kimlikler için iş açılamadı: {e}"),
    }
}

async fn render_departments(state: &AppState, op: &Operator, error: String) -> Response {
    match org::list_departments(&state.pool).await {
        Ok(departments) => {
            let (total, people, empty) = org::department_summary(&departments);
            render(&DepartmentsTemplate {
                lang: op.lang,
                shell: Shell::of(op),
                departments,
                total,
                people,
                empty,
                error,
                can_edit: allowed(op, WRITE_AUTHORITIES),
            })
        }
        Err(e) => internal("departmanlar okunamadı", e),
    }
}

async fn departments_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
) -> Response {
    render_departments(&state, &op, String::new()).await
}

async fn create_department(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let f = Fields(form);
    let parent = f.opt_i64("parent_id");
    match org::create_department(&state.pool, f.get("name"), f.get("code"), parent).await {
        Ok(id) => {
            let detail = serde_json::json!({ "action": "created", "department_id": id, "name": f.get("name"), "parent_id": parent });
            audit_operator(&state, &op, crate::audit::DEPARTMENT_CHANGED, None, detail).await;
            Redirect::to(&address(&state, Owner::Department, id).await).into_response()
        }
        Err(e) => match save_error(e, "departman oluşturulamadı") {
            Ok(key) => render_departments(&state, &op, op.lang.t(key).to_string()).await,
            Err(response) => *response,
        },
    }
}

async fn render_department(state: &AppState, op: &Operator, id: i64, notice: Notice) -> Response {
    let dept = match org::load_department(&state.pool, id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.department_not_found")).into_response()
        }
        Err(e) => return internal("departman okunamadı", e),
    };
    let (departments, options) = match (
        org::list_departments(&state.pool).await,
        org::catalog_options(&state.pool).await,
    ) {
        (Ok(d), Ok(o)) => (d, o),
        (Err(e), _) | (_, Err(e)) => return internal("departman seçenekleri okunamadı", e),
    };
    let chain = match load_chain(state, Owner::Department, id).await {
        Ok(chain) => chain,
        Err(e) => return internal("ayar zinciri okunamadı", e),
    };
    render(&DepartmentTemplate {
        lang: op.lang,
        shell: Shell::of(op),
        targets: target_views(&dept.def, &options, &chain, op.lang),
        parents: parent_options(&dept, &departments),
        dept,
        error: notice.error,
        info: notice.info,
        pending: pending_view(state, Owner::Department, id, op).await,
        can_edit: allowed(op, WRITE_AUTHORITIES),
    })
}

async fn department_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    match resolve_key(&state, &op, Owner::Department, &key, true).await {
        Ok(id) => {
            let notice = Notice::take(&state.pool, &op.username).await;
            render_department(&state, &op, id, notice).await
        }
        Err(response) => *response,
    }
}

async fn save_department(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let id = match resolve_key(&state, &op, Owner::Department, &key, false).await {
        Ok(id) => id,
        Err(response) => return *response,
    };
    let current = match org::load_department(&state.pool, id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, op.lang.t("err.department_not_found")).into_response()
        }
        Err(e) => return internal("departman okunamadı", e),
    };
    let f = Fields(form);
    stage_or_publish(
        &state,
        Submission {
            owner: Owner::Department,
            id,
            edit: definition_edit(&f, &current.def),
            title: String::new(),
            code: f.get("code").to_string(),
            parent_id: f.opt_i64("parent_id"),
            with_settings: true,
            op: &op,
        },
    )
    .await
}

// ADR-026/031: onay taslagi yayimlar, red atar; model onaya kadar degismemistir.
// Onaylayan baslatandan farkli bir Sistem yoneticisidir, ya da zaman kilidi aciksa
// N saat sonra baslatanin kendisi de olabilir.
async fn decide(state: &AppState, op: &Operator, owner: Owner, id: i64, approve: bool) -> Response {
    if !allowed(op, APPROVE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let pending = match change_set::pending(&state.pool, owner, id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            let notice = Notice::err(op.lang.t("err.no_pending_change_set").to_string());
            return render_definition(state, op, owner, id, notice).await;
        }
        Err(e) => return internal("bekleyen taslak okunamadı", e),
    };
    if !approve {
        return reject(state, op, owner, id).await;
    }
    // ADR-055: yayimlanan, onay anindaki farktir; denetim satirina da o girer.
    let d = pending.definition;
    let impact = recompute(state, owner, id, &d).await.unwrap_or(Impact {
        applies: d.applies,
        observed: d.observed,
        ..Impact::default()
    });
    let sub = Submission {
        owner,
        id,
        edit: d.edit,
        title: d.title,
        code: d.code,
        parent_id: d.parent_id,
        with_settings: d.with_settings,
        op,
    };
    publish(
        state,
        sub,
        impact,
        Notice::info(op.lang.t("changeset.approved").into()),
    )
    .await
}

async fn reject(state: &AppState, op: &Operator, owner: Owner, id: i64) -> Response {
    if let Err(e) = change_set::clear(&state.pool, owner, id).await {
        return internal("taslak atılamadı", e);
    }
    let detail = serde_json::json!({ "action": "rejected", "id": id });
    audit_operator(state, op, event_of(owner), None, detail).await;
    let notice = Notice::info(op.lang.t("changeset.rejected").to_string());
    redirect_definition(state, op, owner, id, notice).await
}

/// Onay/red: anahtar sayisal ya da slug; POST adresi slug'a cevirmez (ADR-107
/// madde 5), sonucu tanimin GET adresine yonlendirir (ADR-126 madde 1).
async fn decide_key(
    state: &AppState,
    op: &Operator,
    owner: Owner,
    key: &str,
    approve: bool,
) -> Response {
    match resolve_key(state, op, owner, key, false).await {
        Ok(id) => decide(state, op, owner, id, approve).await,
        Err(response) => *response,
    }
}

async fn approve_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    decide_key(&state, &op, Owner::Role, &key, true).await
}

async fn reject_role(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    decide_key(&state, &op, Owner::Role, &key, false).await
}

async fn approve_department(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    decide_key(&state, &op, Owner::Department, &key, true).await
}

async fn reject_department(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Response {
    decide_key(&state, &op, Owner::Department, &key, false).await
}

async fn render_targets(state: &AppState, op: &Operator, notice: Notice) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let loaded = tokio::try_join!(
        org::list_targets(&state.pool),
        org::catalog_options(&state.pool),
        org::last_catalog_refresh(&state.pool, &time_zone),
        crate::catalog_exit::missing(&state.pool),
    );
    let (targets, options, refreshes, missing) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => return internal("hedef sistemler okunamadı", e),
    };
    let views = targets
        .into_iter()
        .map(|target| target_form_view(target, &options, &refreshes, op.lang))
        .collect();
    render(&TargetsTemplate {
        lang: op.lang,
        shell: Shell::of(op),
        targets: views,
        error: notice.error,
        info: notice.info,
        can_edit: allowed(op, WRITE_AUTHORITIES),
        missing,
    })
}

// --- START FEATURE: missing-catalog-exit ---
/// ADR-127: kayip satirin iki cikisi; yetki tanim duzenlemeyle ayni, sonuc
/// hedefler sayfasina 303 + flash, denetimde hangi oge nereye/hangi tanimlardan.
async fn hand_over_missing(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    exit_missing(&state, &op, id, true).await
}

async fn remove_missing(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    exit_missing(&state, &op, id, false).await
}

async fn exit_missing(state: &AppState, op: &Operator, id: i64, hand_over: bool) -> Response {
    use crate::catalog_exit::{self, Outcome};
    if !allowed(op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let outcome = match hand_over {
        true => catalog_exit::hand_over(&state.pool, id).await,
        false => catalog_exit::remove(&state.pool, id).await,
    };
    let (detail, key) = match outcome {
        Ok(Outcome::NotMissing) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Outcome::NoTwin) => {
            return Notice::err(op.lang.t("catalog.no_twin").to_string())
                .redirect(&state.pool, op, "/targets")
                .await
        }
        Ok(Outcome::HandedOver { twin }) => (
            serde_json::json!({ "action": "catalog_handed_over", "item": id, "to": twin }),
            "catalog.handed_over",
        ),
        Ok(Outcome::Removed { roles, departments }) => (
            serde_json::json!({ "action": "catalog_removed", "item": id,
                "roles": roles, "departments": departments }),
            "catalog.removed",
        ),
        Err(e) => return internal("kayıp katalog öğesi işlenemedi", e),
    };
    audit_operator(state, op, crate::audit::TARGET_CHANGED, None, detail).await;
    Notice::info(op.lang.t(key).to_string())
        .redirect(&state.pool, op, "/targets")
        .await
}
// --- END FEATURE: missing-catalog-exit ---

type RefreshRow = (i64, String, String, String);

fn target_form_view(
    target: org::TargetRow,
    options: &CatalogOptions,
    refreshes: &[RefreshRow],
    lang: Lang,
) -> TargetFormView {
    let last = refreshes.iter().find(|(id, ..)| *id == target.id);
    TargetFormView {
        containers: options
            .containers
            .iter()
            .filter(|c| c.target_id == target.id)
            .map(|c| ItemView {
                id: c.id,
                label: item_label(c, lang),
                selected: target.default_container_item_id == Some(c.id),
            })
            .collect(),
        refresh_status: last.map(|(_, s, ..)| s.clone()).unwrap_or_default(),
        refresh_at: last.map(|(_, _, at, _)| at.clone()).unwrap_or_default(),
        refresh_result: last.map(|(.., r)| r.clone()).unwrap_or_default(),
        target,
    }
}

// ADR-051: ekrandan "yenile" okuma seridine istek yazar; tarama orada calisir,
// yazma seridi ve fren sayaclari etkilenmez.
async fn refresh_catalog(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let opened = match org::request_catalog_refresh(&state.pool, id, &op.username).await {
        Ok(opened) => opened,
        Err(e) => return internal("katalog yenileme isteği yazılamadı", e),
    };
    let key = match opened {
        true => "catalog.refresh_requested",
        false => "catalog.refresh_already_open",
    };
    if opened {
        let detail = serde_json::json!({ "action": "catalog_refresh", "target_id": id });
        audit_operator(&state, &op, crate::audit::TARGET_CHANGED, None, detail).await;
    }
    Notice::info(op.lang.t(key).into())
        .redirect(&state.pool, &op, "/targets")
        .await
}

async fn targets_page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
) -> Response {
    let notice = Notice::take(&state.pool, &op.username).await;
    render_targets(&state, &op, notice).await
}

async fn save_target(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    if !allowed(&op, WRITE_AUTHORITIES) {
        return forbidden(op.lang);
    }
    let Some(mut target) = org::list_targets(&state.pool)
        .await
        .map(|t| t.into_iter().find(|t| t.id == id))
        .unwrap_or_default()
    else {
        return (StatusCode::NOT_FOUND, op.lang.t("err.target_not_found")).into_response();
    };
    let f = Fields(form);
    target.provision_account_default = f.get("provision_account_default") == "1";
    target.default_container_item_id = f.opt_i64("default_container_item_id");
    target.delete_requires_approval = f.get("delete_requires_approval") == "1";
    let (Some(retention), Some(delay)) = (
        f.get("retention_days").trim().parse().ok(),
        f.get("password_reset_delay_days").trim().parse().ok(),
    ) else {
        return render_targets(
            &state,
            &op,
            Notice::err(op.lang.t("err.day_numbers").into()),
        )
        .await;
    };
    target.retention_days = retention;
    target.password_reset_delay_days = delay;
    if let Err(e) = org::save_target(&state.pool, &target).await {
        return match save_error(e, "hedef sistem kaydedilemedi") {
            Ok(key) => render_targets(&state, &op, Notice::err(op.lang.t(key).into())).await,
            Err(response) => *response,
        };
    }
    let detail = serde_json::json!({
        "target_id": id, "provision_account_default": target.provision_account_default,
        "default_container_item_id": target.default_container_item_id,
        "retention_days": retention, "delete_requires_approval": target.delete_requires_approval,
        "password_reset_delay_days": delay,
    });
    audit_operator(&state, &op, crate::audit::TARGET_CHANGED, None, detail).await;
    Redirect::to("/targets").into_response()
}
// --- END FEATURE: role-department-screens ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    async fn cookie(pool: &sqlx::PgPool, authorities: &[&str]) -> String {
        cookie_as(pool, "rol.yoneticisi", authorities).await
    }

    fn source(label: &str, is_self: bool, container: Option<i64>) -> org::SettingSource {
        org::SettingSource {
            target_id: 1,
            label: label.to_string(),
            is_self,
            provision: None,
            container_item_id: container,
            email_domain: None,
            upn_suffix: None,
        }
    }

    fn ad_target() -> org::TargetRow {
        org::TargetRow {
            id: 1,
            kind: "ad".into(),
            name: "Active Directory".into(),
            provision_account_default: true,
            default_container_item_id: Some(99),
            retention_days: 0,
            delete_requires_approval: false,
            password_reset_delay_days: 0,
        }
    }

    fn catalog(ids: &[(i64, &str)]) -> CatalogOptions {
        CatalogOptions {
            memberships: Vec::new(),
            containers: ids
                .iter()
                .map(|(id, name)| org::CatalogChoice {
                    id: *id,
                    target_id: 1,
                    kind: "ou".into(),
                    display_name: (*name).to_string(),
                    location: String::new(),
                    missing: false,
                })
                .collect(),
        }
    }

    // ADR-038: ekran ikinci bir oncelik algoritmasi calistirmaz. "Kazandi" diye
    // isaretlenen kaynagin degeri `resolve_single_valued`'in dondurdugu deger
    // olmali; bu test siranin iki yerde ayrisabilmesini engelliyor.
    #[test]
    fn the_row_marked_as_the_winner_carries_what_resolve_single_valued_returns() {
        // Yalnizca ust departmanlar soyluyor: yakin olan kazanir.
        let chain = [
            source("Charms", true, None),
            source("Teachers", false, Some(7)),
            source("Hogwarts", false, Some(8)),
        ];
        let refs: Vec<&org::SettingSource> = chain.iter().collect();
        let target = ad_target();
        let options = catalog(&[(7, "OU=Teachers"), (8, "OU=Hogwarts"), (99, "OU=Default")]);
        let rows = effective_rows(Owner::Department, &refs, &target, &options, Lang::Tr);
        let container = rows.iter().find(|r| r.label == "def.container").unwrap();
        assert_eq!(container.value, "OU=Teachers");
        assert_eq!(container.from, Lang::Tr.t1("def.from", "Teachers"));
        assert_ne!(container.from, Lang::Tr.t("def.from_self"));
        let resolved = ds::resolve_single_valued(&model_of(Owner::Department, &refs, &target));
        assert_eq!(resolved.container, Some(7), "iki yol ayni kaynagi secmeli");

        // Tanimin kendisi soyleyince o kazanir ve satir "bu tanimdan" der.
        let own = [
            source("Charms", true, Some(5)),
            source("Teachers", false, Some(7)),
        ];
        let refs: Vec<&org::SettingSource> = own.iter().collect();
        let options = catalog(&[(5, "OU=Charms"), (7, "OU=Teachers"), (99, "OU=Default")]);
        let rows = effective_rows(Owner::Department, &refs, &target, &options, Lang::Tr);
        let container = rows.iter().find(|r| r.label == "def.container").unwrap();
        assert_eq!(container.value, "OU=Charms");
        assert_eq!(container.from, Lang::Tr.t("def.from_self"));

        // Hic kaynak soylemezse hedef varsayilani gecerli olur (ADR-017 son halka).
        let none = [source("Charms", true, None)];
        let refs: Vec<&org::SettingSource> = none.iter().collect();
        let rows = effective_rows(Owner::Department, &refs, &target, &options, Lang::Tr);
        let container = rows.iter().find(|r| r.label == "def.container").unwrap();
        assert_eq!(container.value, "OU=Default");
        assert_eq!(container.from, Lang::Tr.t("def.from_target_default"));
        // provision'i da kimse soylemiyor: `provision_expected` gibi hedefe duser.
        let provision = rows.iter().find(|r| r.label == "def.provision").unwrap();
        assert_eq!(provision.value, Lang::Tr.t("common.yes"));
    }

    #[test]
    fn memberships_fold_into_prefix_groups_and_the_selected_one_opens() {
        let item = |id, selected| ItemView {
            id,
            label: String::new(),
            selected,
        };
        let items = vec![
            ("GG-Course-Charms".to_string(), item(1, false)),
            ("GG-Course-Potions".to_string(), item(2, true)),
            ("GG-House-Gryffindor".to_string(), item(3, false)),
            ("GG-House-Slytherin".to_string(), item(4, false)),
            ("GG-Teachers".to_string(), item(5, false)),
            ("Tekil".to_string(), item(6, false)),
        ];
        let groups = group_memberships(items, "Diğer");
        let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["GG-Course", "GG-House", "Diğer"],
            "diğer sonda"
        );
        let course = &groups[0];
        assert_eq!(course.items.len(), 2);
        assert_eq!(course.selected, 1);
        assert!(course.open, "seçili öge taşıyan grup açık gelir");
        assert!(!groups[1].open);
        // Tek elemanli onek kendi basligini acmaz: `GG-Teachers` ve `Tekil` birlikte.
        assert_eq!(groups[2].items.len(), 2);
        // Her oge tam olarak bir kere gecer: form mukerrer deger gondermesin.
        let total: usize = groups.iter().map(|g| g.items.len()).sum();
        assert_eq!(total, 6);
    }

    async fn cookie_as(pool: &sqlx::PgPool, username: &str, authorities: &[&str]) -> String {
        let operator = Operator {
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: format!("{username}@example.org"),
            authorities: authorities.iter().map(|a| a.to_string()).collect(),
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(pool, &operator)
            .await
            .unwrap();
        format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME)
    }

    fn request(method: &str, uri: &str, body: &str, cookie: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .header(header::COOKIE, cookie)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// ADR-127: kayip satirin eylemleri yetkide; canli satira 404; rol formu
    /// isaretli olmayan kayip ogeyi secenek olarak sunmaz.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn missing_catalog_items_have_an_exit_only_for_editors() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        crate::test_support::seed_two_identities(&pool).await;
        let cat = crate::test_support::seed_example_catalog(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let admin = cookie_as(&pool, "ayse.yonetici", &["admin"]).await;
        let auditor = cookie_as(&pool, "veli.denetci", &["auditor"]).await;
        let send = |method: &'static str, uri: String, c: String| {
            let app = app.clone();
            async move { app.oneshot(request(method, &uri, "", &c)).await.unwrap() }
        };
        sqlx::query("UPDATE catalog_items SET missing_since = now() WHERE id = $1")
            .bind(cat.gg_nobet)
            .execute(&pool)
            .await
            .unwrap();
        let role: i64 = sqlx::query_scalar("SELECT primary_role_id FROM identities LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        let page = body_string(send("GET", "/targets".into(), admin.clone()).await).await;
        assert!(
            page.contains(&format!("/catalog/{}/remove", cat.gg_nobet)),
            "{page}"
        );
        assert!(
            !page.contains(&format!("/catalog/{}/hand-over", cat.gg_nobet)),
            "ikizi yok"
        );
        let page = body_string(send("GET", "/targets".into(), auditor.clone()).await).await;
        assert!(!page.contains("/catalog/"), "auditor eylem görmez");
        let role_form =
            body_string(send("GET", format!("/roles/{role}"), admin.clone()).await).await;
        assert!(
            !role_form.contains(&format!("value=\"{}\"", cat.gg_nobet)),
            "işaretli olmayan kayıp öğe seçenek değil"
        );

        let remove = format!("/catalog/{}/remove", cat.gg_nobet);
        assert_eq!(
            send("POST", remove.clone(), auditor).await.status(),
            StatusCode::FORBIDDEN
        );
        let live = format!("/catalog/{}/remove", cat.gg_vpn);
        assert_eq!(
            send("POST", live, admin.clone()).await.status(),
            StatusCode::NOT_FOUND
        );
        let r = send("POST", remove, admin.clone()).await;
        let page = body_string(follow(&app, r, &admin).await).await;
        assert!(page.contains("katalogdan kaldırıldı"), "{page}");
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE detail->>'action' = 'catalog_removed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 1);

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// ADR-126 madde 1: basari POST'u 303 doner; mesaj yonlendirilen GET'te bir kez basilir.
    async fn follow(app: &axum::Router, r: Response, cookie: &str) -> Response {
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let to = r.headers()[header::LOCATION].to_str().unwrap().to_string();
        app.clone()
            .oneshot(request("GET", &to, "", cookie))
            .await
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn location(response: &Response) -> String {
        response.headers()["location"].to_str().unwrap().to_string()
    }

    /// ADR-119 A.6: `?new=` yalnizca izinli listeden bir tur gecirir. Keyfi
    /// metin formun `selected`ina ulasirsa ekranda var olmayan bir tur secili
    /// gorunurdu; bilinmeyen deger formu hic acmaz.
    #[test]
    fn only_a_known_role_kind_opens_the_new_role_form() {
        for kind in org::ROLE_KINDS {
            assert_eq!(role_kind_of(Some(kind)), kind, "{kind}");
        }
        for junk in ["", "yok", "primary'; DROP", "BASE"] {
            assert_eq!(role_kind_of(Some(junk)), "", "{junk}");
        }
        assert_eq!(role_kind_of(None), "");
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn role_admin_edits_roles_departments_and_targets_auditor_only_reads() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let admin = cookie(&pool, &["role_admin"]).await;
        let auditor = cookie(&pool, &["auditor"]).await;
        let send = |method: &'static str, uri: String, body: String, c: String| {
            let app = app.clone();
            async move { app.oneshot(request(method, &uri, &body, &c)).await.unwrap() }
        };

        // Auditor okur, yazamaz.
        let r = send("GET", "/roles".into(), String::new(), auditor.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(!body_string(r).await.contains("Kaydet"));
        let r = send(
            "POST",
            "/roles".into(),
            "kind=primary&name=X".into(),
            auditor.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        // Rol olustur → duzenle: uyelik + ayar; etkilenen kimlikler icin toplu is.
        let r = send(
            "POST",
            "/roles".into(),
            "kind=primary&name=Uzman&title=Sistem+Uzman%C4%B1".into(),
            admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        // ADR-107: yonlendirme okunur adrese, sayisal adres kalici yonlendirmeyle
        let role_url = location(&r);
        assert_eq!(role_url, "/roles/uzman");
        let role_id: i64 = sqlx::query_scalar("SELECT id FROM roles WHERE slug = 'uzman'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let r = send(
            "GET",
            format!("/roles/{role_id}"),
            String::new(),
            admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(location(&r), "/roles/uzman");
        let r = send(
            "GET",
            "/roles/yok-boyle-rol".into(),
            String::new(),
            admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        sqlx::query("UPDATE identities SET primary_role_id = $1 WHERE id = $2")
            .bind(role_id)
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        let body = format!(
            "name=Uzman&title=Uzman&entitlement={}&entitlement={}&pa.{}=true&ct.{}={}&ed.{}=example.com&us.{}=",
            catalog.gg_vpn, catalog.gg_nobet, catalog.ad, catalog.ad, catalog.sistem_uzmanlari_ou, catalog.ad, catalog.ad
        );
        // Kayit sonrasi sayfa etki ozetiyle doner (ADR-031/037): iki kimlik, iki ekleme.
        let r = send("POST", role_url.clone(), body, admin.clone()).await;
        let page = body_string(follow(&app, r, &admin).await).await;
        assert!(page.contains("1 kimlik etkilendi"), "{page}");
        assert!(page.contains("GG-VPN eklendi (1 kimlik)"), "{page}");
        let page =
            body_string(send("GET", role_url.clone(), String::new(), admin.clone()).await).await;
        assert!(
            page.contains("GG-VPN") && page.contains("checked"),
            "{page}"
        );
        assert!(page.contains("example.com"));
        let jobs: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND priority = 2")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(jobs, 2);

        // Bos ad: operator dilinde hata, sayfa yeniden.
        let r = send("POST", role_url.clone(), "name=".into(), admin.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_string(r).await.contains("boş olamaz"));

        // Departman: olustur, alt departman, dongu reddi.
        let r = send(
            "POST",
            "/departments".into(),
            "name=Ankara&code=ANK".into(),
            admin.clone(),
        )
        .await;
        let root_url = location(&r);
        assert_eq!(root_url, "/departments/ankara", "ADR-107 okunur adres");
        let by_slug = |slug: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT id FROM departments WHERE slug = $1")
                    .bind(slug)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let root_id = by_slug("ankara").await;
        let r = send(
            "POST",
            "/departments".into(),
            format!("name=BT&parent_id={root_id}"),
            admin.clone(),
        )
        .await;
        assert_eq!(location(&r), "/departments/bt");
        let child_id = by_slug("bt").await;
        let r = send(
            "POST",
            root_url.clone(),
            format!("name=Ankara&code=ANK&parent_id={child_id}"),
            admin.clone(),
        )
        .await;
        assert!(body_string(r).await.contains("alt departmanı olamaz"));
        let page =
            body_string(send("GET", "/departments".into(), String::new(), auditor.clone()).await)
                .await;
        // Girinti artik `— ` on eki degil derinlik sinifi (CSP: satir ici stil yok).
        // Sinif ile baglanti arasinda ac/kapa okunun yuvasi ve seviye noktasi
        // duruyor (ADR-114 C, ADR-119 B.3); "son kardes" isareti sinifa eklenir.
        assert!(page.contains("<span class=\"tree-d2"), "{page}");
        assert!(
            page.contains(
                "<span class=\"tree-spacer\" data-tree-slot></span>\n                <span class=\"dept-dot\" aria-hidden=\"true\"></span>\n                <a class=\"dept-name\" href=\"/departments/bt\">BT"
            ),
            "{page}"
        );

        // ADR-051: "yenile" okuma seridine istek yazar, tarama orada calisir.
        let r = send(
            "POST",
            format!("/targets/{}/catalog-refresh", catalog.ad),
            String::new(),
            admin.clone(),
        )
        .await;
        // ADR-126: cevap sayfa degil 303; mesaj bir sonraki GET'te basilir
        assert_eq!(location(&r), "/targets");
        let page =
            body_string(send("GET", "/targets".into(), String::new(), admin.clone()).await).await;
        assert!(page.contains("okuma şeridine yazıldı"), "{page}");
        let (kind, requested): (String, Option<String>) =
            sqlx::query_as("SELECT kind, requested_by FROM read_jobs WHERE target_system_id = $1")
                .bind(catalog.ad)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (kind.as_str(), requested.as_deref()),
            ("catalog_refresh", Some("rol.yoneticisi"))
        );
        let r = send(
            "POST",
            format!("/targets/{}/catalog-refresh", catalog.ad),
            String::new(),
            admin.clone(),
        )
        .await;
        assert_eq!(location(&r), "/targets");
        let page =
            body_string(send("GET", "/targets".into(), String::new(), admin.clone()).await).await;
        assert!(page.contains("zaten açık"), "{page}");
        let r = send(
            "POST",
            format!("/targets/{}/catalog-refresh", catalog.ad),
            String::new(),
            auditor.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "auditor yenileyemez");

        // Hedef sistem varsayilanlari.
        let r = send(
            "POST",
            format!("/targets/{}", catalog.ad),
            format!("provision_account_default=1&default_container_item_id={}&retention_days=45&password_reset_delay_days=7", catalog.personel_ou),
            admin.clone(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let page =
            body_string(send("GET", "/targets".into(), String::new(), admin.clone()).await).await;
        assert!(page.contains("value=\"45\""), "{page}");
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE event_type IN ('role.changed', 'department.changed', 'target.changed')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 6, "katalog yenileme isteği de denetlenir");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-031: esigi asan duzenleme MODELE YAZILMAZ, taslak bekler; ADR-026: baslatan
    // onaylayamaz, kilit kapaliyken baska Sistem yoneticisi onaylar; red modeli degistirmez.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn over_threshold_edit_waits_as_draft_until_an_admin_approves() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;
        // Esik 0: fark ureten her duzenleme onaya duser.
        crate::test_support::set_threshold(&pool, 0).await;
        let state = crate::web::test_state(pool.clone(), "https://localhost");
        let app = crate::web::routes().with_state(state);
        let author = cookie_as(&pool, "ayse.yonetici", &["admin"]).await;
        let second = cookie_as(&pool, "ali.yonetici", &["admin"]).await;
        // Duzenleyebilen ama onaylayamayan operator (WRITE_AUTHORITIES'te, APPROVE'da degil)
        let role_admin = cookie_as(&pool, "veli.rol", &["role_admin"]).await;
        let send = |uri: String, body: String, c: String| {
            let app = app.clone();
            async move { app.oneshot(request("POST", &uri, &body, &c)).await.unwrap() }
        };
        let role: i64 = sqlx::query_scalar("SELECT primary_role_id FROM identities WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        // ADR-107: sayfa okunur adresten okunur; POST'lar sayisal anahtari da kabul eder
        let slug: String = sqlx::query_scalar("SELECT slug FROM roles WHERE id = $1")
            .bind(role)
            .fetch_one(&pool)
            .await
            .unwrap();
        let url = format!("/roles/{slug}");
        // Ikinci kimlik baska bir role alinir: taslak kaydedilince 1 kimlik etkilenir.
        let other = org::create_role(&pool, "primary", "Diğer", "")
            .await
            .unwrap();
        sqlx::query("UPDATE identities SET primary_role_id = $1 WHERE id = $2")
            .bind(other)
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        let body = format!(
            "name=Test+Rol%C3%BC&title=Uzman&entitlement={}&pa.{}=true",
            catalog.gg_vpn, catalog.ad
        );

        let r = send(url.clone(), body, author.clone()).await;
        let page = body_string(follow(&app, r, &author).await).await;
        assert!(page.contains("taslak onay bekliyor"), "{page}");
        let published: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM role_entitlements WHERE role_id = $1")
                .bind(role)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(published, 0, "ADR-031: taslak modele yazılmaz");
        assert!(page.contains("Onay bekleyen taslak"), "{page}");

        // F-12: onay yetkisi olmayan operatore panel kimin onaylayacagini soyler
        // (grup adi, ADR-005); ADR-132 sonrasi not yalnizca ona basilir.
        let page = body_string(
            app.clone()
                .oneshot(request("GET", &url, "", &role_admin))
                .await
                .unwrap(),
        )
        .await;
        assert!(page.contains("OpenSicil-Admins"), "{page}");

        // ADR-132: onay kapisi yalnizca yetki sorar; `role_admin` reddedilir
        let r = send(format!("{url}/approve"), String::new(), role_admin).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let published: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM role_entitlements WHERE role_id = $1")
                .bind(role)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(published, 0, "yetkisiz onay modele yazmaz");

        // ADR-055 madde 1: taslak beklerken role atanan kimlik onay anindaki sayiya girer.
        sqlx::query("UPDATE identities SET primary_role_id = $1 WHERE id = $2")
            .bind(role)
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        let page = body_string(
            app.clone()
                .oneshot(request("GET", &url, "", &author))
                .await
                .unwrap(),
        )
        .await;
        assert!(page.contains("2 kimliği etkileyecek"), "{page}");
        assert!(page.contains("1 kimlikti, onay anında 2"), "{page}");

        // ADR-132: taslagi baslatan Sistem yoneticisi kendi onayini verir; model
        // yazilir, taslak duser, isler acilir.
        let page = body_string(
            follow(
                &app,
                send(format!("{url}/approve"), String::new(), author.clone()).await,
                &author,
            )
            .await,
        )
        .await;
        assert!(page.contains("onaylandı ve yayımlandı"), "{page}");
        let (items, draft): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM role_entitlements WHERE role_id = $1), \
             (SELECT COUNT(*) FROM roles WHERE id = $1 AND pending_definition IS NOT NULL)",
        )
        .bind(role)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((items, draft), (1, 0));
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE priority = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(jobs > 0, "onay işleri açar");

        // Red: taslak atilir, model degismez.
        let body = format!("name=Test+Rol%C3%BC&title=Uzman&pa.{}=true", catalog.ad);
        body_string(send(url.clone(), body, second.clone()).await).await;
        let r = send(format!("{url}/reject"), String::new(), second.clone()).await;
        let page = body_string(follow(&app, r, &second).await).await;
        assert!(page.contains("reddedildi"), "{page}");
        let (items, draft): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM role_entitlements WHERE role_id = $1), \
             (SELECT COUNT(*) FROM roles WHERE id = $1 AND pending_definition IS NOT NULL)",
        )
        .bind(role)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((items, draft), (1, 0), "red modeli değiştirmez");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
