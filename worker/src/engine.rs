// --- START FEATURE: engine ---
// Bir isi calistirir (docs/02 motor): girdiyi yukle → olmasi gereken durumu
// hesapla (saf) → hedefi oku → farki islemlere cevirip tek yazma noktasindan
// uygula. AD: hesap acma (tek add, esleme ADR-012/082, gruplar), etkinlestirme/
// pasiflestirme (yalnizca durum gecisinde, ADR-032), uyelik farki ve OU tasima
// (ADR-050 sirasi, ADR-083), oznitelik farki, hesap baglantisi ve applied_state.
// Silme (saklama) ve parola sifirlama sonraki kutucuklarda.

use ldap3::Ldap;
use sqlx::PgPool;

use std::collections::HashMap;

use crate::ad;
use crate::ad_account::{self, AdWriter};
use crate::adoption;
use crate::counters;
use crate::desired_state::{
    desired_state, effective_manager, AccountPresence, Container, DesiredState, LifecycleState,
};
use crate::first_password;
use crate::mapping;
use crate::model::{self, JobInput, LinkRow};
use crate::queue::ClaimedJob;
use crate::username;
use crate::writes::{self, Applied, Mode, OperationClass, WriteFailure, WriteOp, WriteRequest};

#[derive(Clone)]
pub struct EngineEnv<'a> {
    pub time_zone: &'a str,
    pub mode: Mode,
    pub aead_key: &'a [u8; crate::crypto::KEY_LEN],
    pub ad_ca_file: Option<&'a str>,
    pub worker_id: &'a str,
    /// ADR-029: hassas kaynak eslemesi; kapaliyken satir mudahaledir
    pub sensitive_mapping_enabled: bool,
    /// ADR-019: ilk paroladan sonra pwdLastSet 0 (ilk giriste degistir)
    pub first_login_change_required: bool,
    /// ADR-018: sahiplenme; kapaliyken ipuculu kayit mudahaleye duser
    pub ownership_mode_enabled: bool,
    /// ADR-016/050: saatlik fren sayaclari ve acil kota
    pub limits: counters::Limits,
}

// ADR-052: hedefe ulasilamamasi deneme tuketmez; nesne duzeyi hata tuketir;
// insan karari gereken durum beklemeden mudahaleye duser (ADR-022).
#[derive(Debug)]
pub enum JobError {
    Unreachable(String),
    Failed(String),
    NeedsIntervention(String),
    /// ADR-050: sayac dolu; is hicbir islem uygulamadan pencerenin acilisini bekler
    Throttled(counters::Blocked),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Unreachable(reason) => write!(f, "hedefe ulaşılamıyor: {reason}"),
            JobError::Failed(reason) => write!(f, "{reason}"),
            JobError::NeedsIntervention(reason) => write!(f, "müdahale gerekiyor: {reason}"),
            JobError::Throttled(blocked) => write!(f, "{blocked}"),
        }
    }
}

impl From<writes::WriteError> for JobError {
    fn from(e: writes::WriteError) -> Self {
        match e {
            writes::WriteError::Unreachable(r) => JobError::Unreachable(r),
            writes::WriteError::Failed(r) => JobError::Failed(r),
        }
    }
}

impl From<WriteFailure> for JobError {
    fn from(e: WriteFailure) -> Self {
        match e {
            WriteFailure::LeaseLost => JobError::Failed(e.to_string()),
            WriteFailure::Unreachable(r) => JobError::Unreachable(r),
            WriteFailure::Failed(r) => JobError::Failed(r),
            WriteFailure::Db(e) => JobError::Failed(e.to_string()),
        }
    }
}

// Kuru calistirma sonucunun oneki; gozlem farki ayni metni kendi onekiyle sunar.
const DRY_RUN_PREFIX: &str = "kuru çalıştırma, uygulanacaktı: ";

pub fn state_name(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Pending => "pending",
        LifecycleState::Active => "active",
        LifecycleState::Suspended => "suspended",
        LifecycleState::Departed => "departed",
        LifecycleState::Deleted => "deleted",
    }
}

pub async fn run_job(
    pool: &PgPool,
    job: &ClaimedJob,
    env: &EngineEnv<'_>,
) -> Result<String, JobError> {
    let input = model::load(pool, job.identity_id, job.target_system_id, env.time_zone)
        .await
        .map_err(JobError::Failed)?;
    let desired = desired_state(
        &input.timeline,
        &input.model,
        input.link.as_ref(),
        &input.clock,
    );
    if input.target_kind != "ad" {
        return Ok(format!(
            "{} connector'ı yok, hedefe yazılmadı: {desired:?}",
            input.target_kind
        ));
    }
    // AD yapilandirilmamissa is deneme tuketmeden bekler (ADR-052 "erisilemiyor"
    // gibi): Yapilandirma sayfasindan ayar girilince kaldigi yerden surer.
    let cfg = ad::load_config(pool, env.aead_key, env.ad_ca_file)
        .await
        .map_err(JobError::Failed)?
        .ok_or_else(|| JobError::Unreachable("AD yapılandırılmamış".to_string()))?;
    // ADR-029: esleme satirlari her iste dogrulanir; ihlal mudahaledir
    let mappings = mapping::load_rows(pool, job.target_system_id)
        .await
        .map_err(JobError::Failed)?;
    mapping::validate(&mappings, "ad", env.sensitive_mapping_enabled)
        .map_err(JobError::NeedsIntervention)?;
    let mut ldap = ad::connect(&cfg).await?;
    let base_dn = ad::base_dn(&mut ldap).await?;
    let ctx = AdJob {
        pool,
        job,
        env,
        base_dn: &base_dn,
        input: &input,
        desired: &desired,
        mappings: &mappings,
    };
    let outcome = reconcile_ad(&ctx, &mut ldap).await;
    ldap.unbind().await.ok();
    outcome
}

#[derive(Clone, Copy)]
struct AdJob<'a> {
    pool: &'a PgPool,
    job: &'a ClaimedJob,
    env: &'a EngineEnv<'a>,
    base_dn: &'a str,
    input: &'a JobInput,
    desired: &'a DesiredState,
    mappings: &'a [mapping::MappingRow],
}

async fn reconcile_ad(c: &AdJob<'_>, ldap: &mut Ldap) -> Result<String, JobError> {
    // ADR-048: iptal once hedefte dogrulanir; sonuc baglantiya yazilir, uygulama sonraki iste
    if let (Some(link), true, Some(None)) = (
        &c.input.link_row,
        c.input.timeline.cancelled,
        c.input.link.as_ref().map(|l| l.verified_unused),
    ) {
        return verify_cancellation(c, ldap, link).await;
    }
    let link_row = resolve_link(c).await?;
    match (&link_row, c.desired.account) {
        // ADR-018: ipucu doluysa hesap acilmaz, sahiplenilir
        (None, AccountPresence::Present { enabled }) => {
            match c.input.person.existing_ad_account_hint.as_deref() {
                Some(hint) => adopt(c, ldap, hint).await,
                None => provision(c, ldap, enabled).await,
            }
        }
        (None, presence) => Ok(format!("hesap yok ve açılmayacak: {presence:?}")),
        // ADR-018: gozlem modunda motor farki hesaplar, hicbir sey uygulamaz
        (Some(link), _) if link.observed => observe(c, ldap, link).await,
        (Some(link), AccountPresence::Present { enabled }) => {
            let result = reconcile_existing(c, ldap, link, enabled).await?;
            // ADR-040: ayar yalnizca hesap yokken okunur; bagli hesap yonetilmeye devam
            // eder, mutabakat (3d) ayni bayragi "rol hesap ongormuyor" bulgusu yapar.
            Ok(if c.desired.provision_not_expected {
                format!("{result}; bilgi: rol hesap öngörmüyor, mevcut hesap yönetiliyor (ADR-040)")
            } else {
                result
            })
        }
        (Some(link), AccountPresence::Absent) => delete_account(c, ldap, link).await,
        (Some(_), AccountPresence::AwaitingDeletionApproval) => {
            Ok("silinmeyi bekliyor (ADR-024)".to_string())
        }
        // desired_state bagli hesapta NotProvisioned uretmez (ADR-040); savunma dali
        (Some(_), AccountPresence::NotProvisioned) => {
            Ok("'hesap açılsın = hayır' yalnızca hesap yokken okunur (ADR-040)".to_string())
        }
    }
}

// ADR-087: yonetime alma istendiyse gozlem bayragi bu isin basinda duser ve fark
// ayni iste uygulanir; kuru calistirmada istek bekler (ADR-054).
async fn resolve_link(c: &AdJob<'_>) -> Result<Option<LinkRow>, JobError> {
    let Some(link) = &c.input.link_row else {
        return Ok(None);
    };
    if !link.manage_requested || c.env.mode.dry_run {
        return Ok(Some(link.clone()));
    }
    let taken = adoption::take_over(c.pool, c.job.identity_id, c.job.target_system_id)
        .await
        .map_err(JobError::Failed)?;
    Ok(Some(LinkRow {
        observed: link.observed && !taken,
        manage_requested: !taken,
        ..link.clone()
    }))
}

// ADR-050: is sayaclara karsi butundur — hedefe ilk yazmadan once isin uretecegi
// butun sayac siniflari sinanir, biri doluysa hicbir islem uygulanmaz. Kuru
// calistirma ve gozlem modu hedefe yazmadigi icin sayaclara da girmez (ADR-054/087).
async fn gate(c: &AdJob<'_>, classes: &[OperationClass]) -> Result<(), JobError> {
    if c.env.mode.dry_run || classes.is_empty() {
        return Ok(());
    }
    let blocked = counters::blocked(
        c.pool,
        classes,
        c.job.identity_id,
        c.input.timeline.emergency_departure,
        &c.env.limits,
    )
    .await
    .map_err(JobError::Failed)?;
    match blocked {
        Some(blocked) => Err(JobError::Throttled(blocked)),
        None => Ok(()),
    }
}

async fn apply(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    op: WriteOp,
    class: OperationClass,
) -> Result<Applied, JobError> {
    let request = WriteRequest {
        op: &op,
        class,
        emergency: c.input.timeline.emergency_departure,
    };
    let mut writer = AdWriter { ldap };
    Ok(writes::apply(
        c.pool,
        c.job,
        c.env.worker_id,
        c.env.mode,
        &mut writer,
        &request,
    )
    .await?)
}

struct Names {
    username: String,
    email: Option<String>,
    upn: String,
}

// UPN soneki: tek degerli ayar, yoksa domain DNS adi (DC bilesenlerinden)
fn upn_suffix_for(c: &AdJob<'_>) -> String {
    c.desired.upn_suffix.clone().unwrap_or_else(|| {
        c.base_dn
            .split(',')
            .filter_map(|part| {
                part.trim()
                    .strip_prefix("DC=")
                    .or(part.trim().strip_prefix("dc="))
            })
            .collect::<Vec<_>>()
            .join(".")
    })
}

// docs/05: ad AD'ye yazilmadan once veritabanina kaydedilir; yeniden denemede
// yeniden uretilmez. Cakisma cozumu ADR-011/022 (username modulu).
async fn ensure_names(c: &AdJob<'_>, ldap: &mut Ldap) -> Result<Names, JobError> {
    let person = &c.input.person;
    if let (Some(username), Some(upn)) = (&person.username, &person.upn) {
        return Ok(Names {
            username: username.clone(),
            email: person.email.clone(),
            upn: upn.clone(),
        });
    }
    let candidate = candidate_for(person)?;
    let names = resolve_names(c, ldap, &candidate).await?;
    sqlx::query("UPDATE identities SET username = $2, email = $3, upn = $4 WHERE id = $1")
        .bind(c.job.identity_id)
        .bind(&names.username)
        .bind(&names.email)
        .bind(&names.upn)
        .execute(c.pool)
        .await
        .map_err(|e| JobError::Failed(format!("kullanıcı adı kaydedilemedi: {e}")))?;
    Ok(names)
}

// ADR-022: elle girilen ad varsa sablon calismaz; e-posta yerel kismi da odur.
fn candidate_for(person: &model::Person) -> Result<username::Candidate, JobError> {
    let override_conflicts = person.name_conflict_override;
    let requested = person
        .requested_username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(raw) = requested {
        let name = username::validate_manual(raw).map_err(JobError::NeedsIntervention)?;
        return Ok(username::Candidate {
            email_local: name.clone(),
            username: name,
            manual: true,
            override_conflicts,
        });
    }
    let templates = username::Templates::from_lookup(|n| std::env::var(n).ok());
    let input = username::NameInput {
        given_names: &person.given_name,
        surname: &person.surname,
        employee_number: person.employee_number.as_deref(),
    };
    Ok(username::Candidate {
        username: username::base_username(&templates, &input)
            .map_err(JobError::NeedsIntervention)?,
        email_local: username::base_email_local(&templates, &input)
            .map_err(JobError::NeedsIntervention)?,
        manual: false,
        override_conflicts,
    })
}

async fn resolve_names(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    candidate: &username::Candidate,
) -> Result<Names, JobError> {
    let upn_suffix = upn_suffix_for(c);
    let ctx = username::Context {
        base_dn: c.base_dn,
        upn_suffix: &upn_suffix,
        email_domain: c.desired.email_domain.as_deref(),
    };
    match username::resolve(c.pool, ldap, &ctx, candidate).await? {
        username::Resolution::Ok {
            username,
            email_local,
        } => Ok(Names {
            email: c
                .desired
                .email_domain
                .as_ref()
                .map(|d| format!("{email_local}@{d}")),
            upn: format!("{username}@{upn_suffix}"),
            username,
        }),
        username::Resolution::NeedsIntervention(reason) => Err(JobError::NeedsIntervention(reason)),
    }
}

async fn catalog_guid(pool: &PgPool, item_id: i64) -> Result<Option<String>, JobError> {
    sqlx::query_scalar(
        "SELECT external_id FROM catalog_items WHERE id = $1 AND missing_since IS NULL",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| JobError::Failed(format!("katalog öğesi okunamadı: {e}")))
}

async fn container_dn(c: &AdJob<'_>, ldap: &mut Ldap) -> Result<String, JobError> {
    let guid = match c.desired.container {
        Container::Item(id) => catalog_guid(c.pool, id).await?,
        Container::Passive => {
            return passive_ou().ok_or_else(|| {
                JobError::Failed("pasif OU tanımlı değil (AD_PASSIVE_OU)".to_string())
            })
        }
        Container::Unchanged | Container::Unspecified => None,
    };
    let guid = guid.ok_or_else(|| {
        JobError::Failed(
            "hesap için OU belirsiz: rol, departman ya da hedef varsayılanında OU yok".to_string(),
        )
    })?;
    ad_account::dn_by_guid(ldap, &guid)
        .await?
        .ok_or_else(|| JobError::Failed(format!("OU hedefte bulunamadı (GUID {guid})")))
}

// Eslenemeyen kimlik alanlari (ADR-034): acilista bir kez yazilir, sonra dokunulmaz.
fn fixed_attributes(names: &Names, cn: &str) -> Vec<(String, String)> {
    vec![
        ("cn".to_string(), cn.to_string()),
        ("sAMAccountName".to_string(), names.username.clone()),
        ("userPrincipalName".to_string(), names.upn.clone()),
    ]
}

// Esleme kaynaklari (ADR-012/082). Hassas kaynaklar yalnizca ayar acikken cozulur;
// `names` acilista DB'ye yeni yazilan adlardir (Person henuz eski kopya).
fn plain_sources(c: &AdJob<'_>, names: Option<&Names>) -> HashMap<&'static str, String> {
    let p = &c.input.person;
    let or_person = |fresh: Option<String>, stored: &Option<String>| {
        fresh.or_else(|| stored.clone()).unwrap_or_default()
    };
    HashMap::from([
        ("given_name", p.given_name.clone()),
        ("surname", p.surname.clone()),
        (
            "employee_number",
            p.employee_number.clone().unwrap_or_default(),
        ),
        (
            "email",
            or_person(names.and_then(|n| n.email.clone()), &p.email),
        ),
        (
            "username",
            or_person(names.map(|n| n.username.clone()), &p.username),
        ),
        ("upn", or_person(names.map(|n| n.upn.clone()), &p.upn)),
        ("department_name", p.department_name.clone()),
        ("root_department_name", p.root_department_name.clone()),
        ("title", c.desired.title.clone().unwrap_or_default()),
        ("employment_type", p.employment_type.clone()),
        ("start_date", p.start_date.clone()),
        ("end_date", p.end_date.clone().unwrap_or_default()),
    ])
}

async fn sources(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    names: Option<&Names>,
) -> Result<mapping::Sources, JobError> {
    let mut values = plain_sources(c, names);
    if c.env.sensitive_mapping_enabled {
        let p = &c.input.person;
        values.insert("mobile_phone", p.mobile_phone.clone().unwrap_or_default());
        let national_id = p
            .national_id_enc
            .as_deref()
            .and_then(|enc| mapping::decrypt_national_id(c.env.aead_key, enc))
            .unwrap_or_default();
        values.insert("national_id", national_id);
    }
    let needs_manager = c
        .mappings
        .iter()
        .any(|r| r.source_kind == "manager_account");
    let manager_dn = if needs_manager {
        manager_dn(c, ldap).await?
    } else {
        Some(None)
    };
    Ok(mapping::Sources { values, manager_dn })
}

// ADR-040/041: etkin yonetici yok → temizle (None); var ama bu hedefte yonetilen
// hesabi yok ya da bulunamiyor → belirsiz (Some(None)), dokunulmaz.
async fn manager_dn(c: &AdJob<'_>, ldap: &mut Ldap) -> Result<Option<Option<String>>, JobError> {
    let Some(manager) = effective_manager(c.input.manager.as_ref()) else {
        return Ok(None);
    };
    let guid: Option<String> = sqlx::query_scalar(
        "SELECT external_id FROM account_links WHERE identity_id = $1 AND target_system_id = $2 \
         AND mode = 'managed' AND deleted_by_us_at IS NULL",
    )
    .bind(manager)
    .bind(c.job.target_system_id)
    .fetch_optional(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("yönetici bağlantısı okunamadı: {e}")))?;
    match guid {
        None => Ok(Some(None)),
        Some(guid) => Ok(Some(ad_account::dn_by_guid(ldap, &guid).await?)),
    }
}

async fn provision(c: &AdJob<'_>, ldap: &mut Ldap, enabled: bool) -> Result<String, JobError> {
    let names = ensure_names(c, ldap).await?;
    let ou_dn = container_dn(c, ldap).await?;
    let person = &c.input.person;
    let mut cn = ad_account::cn_for(&person.given_name, &person.surname, None);
    if ad_account::cn_exists(ldap, &ou_dn, &cn).await? {
        cn = ad_account::cn_for(&person.given_name, &person.surname, Some(&names.username));
    }
    let dn = ad_account::account_dn(&cn, &ou_dn);
    let mut attributes = fixed_attributes(&names, &cn);
    let s = sources(c, ldap, Some(&names)).await?;
    attributes.extend(mapping::initial_attributes(c.mappings, &s));
    // Hesap acma ve ilk uyelikler "verme"dir (ADR-050); etkinlestirme oznitelik.
    let first_request = pending_first_password(c).await?;
    gate(
        c,
        &counters::needed_classes(true, false, first_request.is_some()),
    )
    .await?;
    let create = WriteOp::CreateAccount {
        dn: dn.clone(),
        attributes,
        password: ad_account::random_password(),
        account_expires: c.desired.account_expires,
    };
    if apply(c, ldap, create, OperationClass::Grant).await? == Applied::DryRun {
        return Ok(format!(
            "kuru çalıştırma, uygulanacaktı: hesap {dn}, {} grup, {}",
            c.desired.memberships.len(),
            if enabled { "etkin" } else { "pasif" }
        ));
    }
    let guid = ad_account::guid_by_dn(ldap, &dn).await?;
    let link = insert_link(c, &guid).await?;
    let added = add_memberships(c, ldap, &dn).await?;
    if enabled {
        let op = WriteOp::SetEnabled {
            dn: dn.clone(),
            enabled: true,
        };
        apply(c, ldap, op, OperationClass::Attribute).await?;
    }
    set_applied_state(c, state_name(c.desired.state)).await?;
    // ADR-056: "kaydet ve ilk parolayi ver" istegi ayni iste, hesap acilir acilmaz
    let first = issue_first_password(c, ldap, &dn, &link, first_request).await?;
    Ok(format!(
        "hesap açıldı: {dn} (objectGUID {guid}), {added} grup, {}{first}",
        if enabled { "etkin" } else { "pasif" }
    ))
}

// --- START FEATURE: adoption ---
// ADR-018/086: ipucundaki hesap kurallardan gecerse baglanti gozlem modunda yazilir;
// her ret mudahaledir, nedeni is satirinda.
async fn adopt(c: &AdJob<'_>, ldap: &mut Ldap, hint: &str) -> Result<String, JobError> {
    let intervene = |why: String| Err(JobError::NeedsIntervention(why));
    if !c.env.ownership_mode_enabled {
        return intervene("sahiplenme kapalı (OWNERSHIP_MODE_ENABLED=false): ipucuyu kaldırın ya da ayarı açıp tekrar deneyin (ADR-018)".to_string());
    }
    if c.env.mode.dry_run {
        return intervene("kuru çalıştırma açık: sahiplenme uygulanmadı, DRY_RUN kapanınca tekrar deneyin (ADR-054)".to_string());
    }
    let employee_attr = c
        .mappings
        .iter()
        .find(|m| m.source_kind == "employee_number")
        .map(|m| m.attribute.as_str());
    let Some(cand) = adoption::find_by_sam(ldap, c.base_dn, hint, employee_attr).await? else {
        return intervene(format!("ipucu bulunamadı: '{hint}' AD'de yok"));
    };
    if let Some(why) = adoption_violation(c, ldap, &cand).await? {
        return intervene(format!("sahiplenme reddedildi: {why} ({})", cand.dn));
    }
    let person = &c.input.person;
    let mismatch = !adoption::name_matches(
        cand.given_name.as_deref(),
        cand.surname.as_deref(),
        &person.given_name,
        &person.surname,
    );
    adoption::link_observed(
        c.pool,
        c.job.identity_id,
        c.job.target_system_id,
        &cand,
        mismatch,
    )
    .await
    .map_err(JobError::Failed)?;
    adopted_result(c, ldap, &cand, mismatch).await
}

// Fark ayni iste hesaplanir: zamanlayici gozlem baglantisina is acmaz, yoksa operator
// yonetime almaya karar verecegi farki hic gormezdi (ADR-018/087).
async fn adopted_result(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    cand: &adoption::Candidate,
    mismatch: bool,
) -> Result<String, JobError> {
    let warning = if mismatch {
        "; uyarı: hedefteki ad-soyad kimlikle uyuşmuyor (ADR-042)"
    } else {
        ""
    };
    // Numara uydurulmaz: E.164 olmayan deger kimlige yazilmaz, operator duzeltir (ADR-106).
    let phone_warning = if cand.phone_rejected() {
        "; uyarı: hedefteki telefon E.164 biçiminde değil, kimliğe yazılmadı (ADR-106)"
    } else {
        ""
    };
    let link = LinkRow {
        external_id: cand.guid.clone(),
        applied_state: None,
        password_reset_at_departure: false,
        first_password_pwd_last_set: None,
        observed: true,
        manage_requested: false,
    };
    let diff = observe(c, ldap, &link).await?;
    Ok(format!(
        "sahiplenildi (gözlem modu): {}{warning}{phone_warning}; {diff}",
        cand.dn
    ))
}

// ADR-018 kosullari 2–5 sirayla; ilk ihlalin nedeni doner.
async fn adoption_violation(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    cand: &adoption::Candidate,
) -> Result<Option<String>, JobError> {
    let scope = ad::parse_scope(|n| std::env::var(n).ok()).map_err(JobError::Failed)?;
    if !ad::under_any(&cand.dn, &scope.user_ous) {
        return Ok(Some("yönetilen kullanıcı OU'larının dışında".to_string()));
    }
    let linked = adoption::linked_identity(c.pool, c.job.target_system_id, &cand.guid)
        .await
        .map_err(JobError::Failed)?;
    if let Some(other) = linked {
        return Ok(Some(format!("hesap #{other} kimliğine bağlı")));
    }
    if cand.admin_count {
        return Ok(Some(
            "adminCount dolu; yapışkandır, kurum temizleyip yeniden dener (docs/05)".to_string(),
        ));
    }
    if let Some(group) = ad::privileged_group(ldap, &cand.member_of).await? {
        return Ok(Some(format!("yasaklı grup üyesi: {group}")));
    }
    let employee_ok = adoption::employee_number_matches(
        cand.employee_value.as_deref(),
        c.input.person.employee_number.as_deref(),
    );
    Ok((!employee_ok).then(|| "sicil no hedefteki değerle uyuşmuyor".to_string()))
}

// ADR-018: gozlem modunda motor "yonetime alinirsa ne degisir" farkini hesaplar ve
// is sonucuna yazar (kisi sayfasi gosterir); hedefe ve denetim kaydina dokunmaz —
// yazma noktasi kuru calistirma gibi gecer, sayaclar degismez (ADR-054 ile ayni yol).
async fn observe(c: &AdJob<'_>, ldap: &mut Ldap, link: &LinkRow) -> Result<String, JobError> {
    let pending = if link.manage_requested {
        " (yönetime alma istendi: kuru çalıştırma kapanınca uygulanır, ADR-054)"
    } else {
        ""
    };
    let AccountPresence::Present { enabled } = c.desired.account else {
        return Ok(format!(
            "gözlem modunda, uygulanmadı: hesabın kalmaması gereken durum (silme/saklama) \
             yönetime alınmadan uygulanmaz{pending}"
        ));
    };
    let env = EngineEnv {
        mode: Mode { dry_run: true },
        ..c.env.clone()
    };
    let observed = AdJob { env: &env, ..*c };
    let diff = reconcile_existing(&observed, ldap, link, enabled).await?;
    let diff = diff.strip_prefix(DRY_RUN_PREFIX).unwrap_or(&diff);
    Ok(format!(
        "gözlem modunda, yönetime alınırsa: {diff}{pending}"
    ))
}
// --- END FEATURE: adoption ---

async fn insert_link(c: &AdJob<'_>, guid: &str) -> Result<LinkRow, JobError> {
    sqlx::query(
        "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
         VALUES ($1, $2, $3, 'provisioned', 'managed')",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .bind(guid)
    .execute(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("hesap bağlantısı yazılamadı: {e}")))?;
    Ok(LinkRow {
        external_id: guid.to_string(),
        applied_state: None,
        password_reset_at_departure: false,
        first_password_pwd_last_set: None,
        observed: false,
        manage_requested: false,
    })
}

async fn add_memberships(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    member_dn: &str,
) -> Result<usize, JobError> {
    let mut added = 0;
    for item in &c.desired.memberships {
        let Some(guid) = catalog_guid(c.pool, *item).await? else {
            continue; // kayip katalog ogesi: islem uretilmez (docs/03)
        };
        let Some(group_dn) = ad_account::dn_by_guid(ldap, &guid).await? else {
            continue;
        };
        let op = WriteOp::AddMember {
            group_dn,
            member_dn: member_dn.to_string(),
        };
        apply(c, ldap, op, OperationClass::Grant).await?;
        added += 1;
    }
    Ok(added)
}

async fn set_applied_state(c: &AdJob<'_>, state: &str) -> Result<(), JobError> {
    sqlx::query(
        "UPDATE account_links SET applied_state = $3 WHERE identity_id = $1 AND target_system_id = $2",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .bind(state)
    .execute(c.pool)
    .await
    .map(|_| ())
    .map_err(|e| JobError::Failed(format!("applied_state yazılamadı: {e}")))
}

struct ExistingPlan {
    enabled: EnabledPlan,
    groups: GroupPlan,
    first_request: Option<i64>,
}

// ADR-050/091: once plan (yalnizca okuma), sonra fren. Buradan donuldugunde isin
// uretecegi butun sayac siniflari sinanmistir; sonrasi hedefe yazar.
async fn plan_existing(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    link: &LinkRow,
    account: &ad_account::DirectoryAccount,
    enabled: bool,
) -> Result<ExistingPlan, JobError> {
    let transition = link.applied_state.as_deref() != Some(state_name(c.desired.state));
    let enabled_plan = plan_enabled(link, account, enabled, transition);
    let groups = plan_groups(c, ldap, account).await?;
    let first_request = pending_first_password(c).await?;
    gate(
        c,
        &counters::needed_classes(
            !groups.to_add.is_empty(),
            !groups.to_remove.is_empty()
                || enabled_plan.class() == Some(OperationClass::Destructive),
            first_request.is_some(),
        ),
    )
    .await?;
    Ok(ExistingPlan {
        enabled: enabled_plan,
        groups,
        first_request,
    })
}

// ADR-032: etkinlestirme yalnizca durum gecisinde; elle pasiflestirilmis hesap
// korunur. Pasiflestirme her iste. ADR-030: ayrildidan donus yikici sayilir.
async fn reconcile_existing(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    link: &LinkRow,
    enabled: bool,
) -> Result<String, JobError> {
    let Some(account) = ad_account::find_by_guid(ldap, &link.external_id).await? else {
        return Ok(format!(
            "kayıp hesap (GUID {}): dokunulmadı (ADR-040)",
            link.external_id
        ));
    };
    let state = state_name(c.desired.state);
    let transition = link.applied_state.as_deref() != Some(state);
    let plan = plan_existing(c, ldap, link, &account, enabled).await?;
    let (applied, note) = apply_enabled(c, ldap, plan.enabled).await?;
    // ADR-050 sirasi: pasiflestirme → ekleme → OU tasima → cikarma → oznitelikler
    let (dn, groups) = apply_groups_and_ou(c, ldap, &account, &plan.groups).await?;
    let attrs = sync_attributes(c, ldap, &dn).await?;
    let expires = sync_account_expires(c, ldap, &dn).await?;
    let reset = reset_password_if_due(c, ldap, &dn, link).await?;
    let first = issue_first_password(c, ldap, &dn, link, plan.first_request).await?;
    let expires = format!("{expires}{reset}{first}");
    if applied == Applied::DryRun || c.env.mode.dry_run {
        return Ok(format!(
            "{DRY_RUN_PREFIX}{note}{groups}{attrs}{expires} ({dn})"
        ));
    }
    if transition {
        set_applied_state(c, state).await?;
        // ADR-041: ayrildiya giris/cikis astlarin etkin yoneticisini degistirir → astlara is
        let departed = "departed";
        if state == departed || link.applied_state.as_deref() == Some(departed) {
            enqueue_subordinates(c).await?;
        }
    }
    Ok(format!(
        "{note}{groups}{attrs}{expires}: {dn} → applied_state {state}"
    ))
}

// ADR-033: ayrilistan G gun sonra (acilde hemen) parola rastgelelestirilir, bir kez;
// yikici sayaca girmez (hesap zaten pasif). Isaret baglantida kalir (ADR-046 ilk parola).
async fn reset_password_if_due(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    dn: &str,
    link: &LinkRow,
) -> Result<String, JobError> {
    if !c.desired.password_reset_due || link.password_reset_at_departure {
        return Ok(String::new());
    }
    let op = WriteOp::ResetPassword { dn: dn.to_string() };
    if apply(c, ldap, op, OperationClass::Attribute).await? == Applied::DryRun {
        return Ok(", parola sıfırlanır".to_string());
    }
    sqlx::query(
        "UPDATE account_links SET password_reset_at_departure = TRUE \
         WHERE identity_id = $1 AND target_system_id = $2",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .execute(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("parola sıfırlama işareti yazılamadı: {e}")))?;
    Ok(", parola sıfırlandı".to_string())
}

// --- START FEATURE: first-password ---
// ADR-046/085: bekleyen istek varsa hesap "kullanilmamis" kuralindan gecer, okunabilir
// parola yazilir ve sifreli olarak istege birakilir; red nedeni istege yazilir.
async fn pending_first_password(c: &AdJob<'_>) -> Result<Option<i64>, JobError> {
    first_password::pending(c.pool, c.job.identity_id, c.job.target_system_id)
        .await
        .map_err(JobError::Failed)
}

// ADR-046: hesaba hic girilmemis mi; hedeften okunan iki damgaya bakilir.
async fn account_unused(ldap: &mut Ldap, dn: &str, link: &LinkRow) -> Result<bool, JobError> {
    let before =
        ad_account::read_attributes(ldap, dn, &["lastLogonTimestamp", "pwdLastSet"]).await?;
    Ok(first_password::account_unused(
        first_value(&before, "lastLogonTimestamp").as_deref(),
        first_value(&before, "pwdLastSet").as_deref(),
        link,
    ))
}

async fn issue_first_password(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    dn: &str,
    link: &LinkRow,
    request: Option<i64>,
) -> Result<String, JobError> {
    let Some(request) = request else {
        return Ok(String::new());
    };
    let reject = |reason: &'static str| async move {
        first_password::reject(c.pool, request, reason)
            .await
            .map_err(JobError::Failed)?;
        Ok(format!(", ilk parola reddedildi: {reason}"))
    };
    if !account_unused(ldap, dn, link).await? {
        return reject(first_password::REJECT_USED).await;
    }
    let password = ad_account::readable_password();
    let op = WriteOp::SetFirstPassword {
        dn: dn.to_string(),
        password: password.clone(),
        change_required: c.env.first_login_change_required,
    };
    if apply(c, ldap, op, OperationClass::FirstPassword).await? == Applied::DryRun {
        // Gozlem modu yazma noktasini kuru gecer; red nedeni operatorun gordugu sebeptir
        return reject(if link.observed {
            first_password::REJECT_OBSERVED
        } else {
            first_password::REJECT_DRY_RUN
        })
        .await;
    }
    let stamp = match c.env.first_login_change_required {
        true => None,
        false => first_value(
            &ad_account::read_attributes(ldap, dn, &["pwdLastSet"]).await?,
            "pwdLastSet",
        ),
    };
    first_password::issue(c.pool, c.env.aead_key, request, &password, stamp.as_deref())
        .await
        .map_err(JobError::Failed)?;
    Ok(", ilk parola verildi".to_string())
}

fn first_value(attrs: &HashMap<String, Vec<String>>, name: &str) -> Option<String> {
    attrs.get(name).and_then(|v| v.first()).cloned()
}
// --- END FEATURE: first-password ---

// Astlar icin ayni hedefe tek kimlik oncelikli is; acik is varsa yenisi acilmaz.
async fn enqueue_subordinates(c: &AdJob<'_>) -> Result<(), JobError> {
    sqlx::query(
        "INSERT INTO jobs (identity_id, target_system_id, priority) \
         SELECT id, $2, 1 FROM identities WHERE manager_id = $1 AND deleted_at IS NULL \
         ON CONFLICT (identity_id, target_system_id) WHERE status <> 'succeeded' DO NOTHING",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .execute(c.pool)
    .await
    .map(|_| ())
    .map_err(|e| JobError::Failed(format!("astların işi açılamadı: {e}")))
}

// ADR-046/048: hic kullanilmamis = lastLogonTimestamp bos ve pwdLastSet 0.
// Dogrulama sonucu baglantiya yazilir; red ayrilis olarak uygulanir (desired_state).
async fn verify_cancellation(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    link: &LinkRow,
) -> Result<String, JobError> {
    let unused = match ad_account::find_by_guid(ldap, &link.external_id).await? {
        Some(account) => {
            account.last_logon_timestamp.is_none()
                && account.pwd_last_set.as_deref().is_none_or(|v| v == "0")
        }
        // hedefte hesap yok: silinecek bir sey yok, iptal gecerli sayilir
        None => true,
    };
    sqlx::query(
        "UPDATE account_links SET verified_unused = $3 WHERE identity_id = $1 AND target_system_id = $2",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .bind(unused)
    .execute(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("iptal doğrulaması yazılamadı: {e}")))?;
    let (event, note) = if unused {
        (
            "ad.cancellation.verified",
            "iptal doğrulandı: hesap hiç kullanılmamış; silme sonraki işte",
        )
    } else {
        (
            "ad.cancellation.rejected",
            "iptal reddedildi: hesap kullanılmış ya da sahiplenilmiş; ayrılış olarak uygulanır (ADR-048)",
        )
    };
    sqlx::query(
        "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
         VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(event)
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .bind(format!("{{\"unused\":{unused}}}"))
    .execute(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("denetim satırı yazılamadı: {e}")))?;
    Ok(note.to_string())
}

// ADR-024/038/084: hesap silinir (yikici), baglanti isaretlenir; son hesapsa kimlik
// `silindi` olur, kisisel veri temizlenir, adlar yakilir (iptalde yakilmaz, ADR-035).
async fn delete_account(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    link: &LinkRow,
) -> Result<String, JobError> {
    let note = match ad_account::dn_by_guid(ldap, &link.external_id).await? {
        Some(dn) => {
            gate(c, &counters::needed_classes(false, true, false)).await?;
            let op = WriteOp::DeleteAccount { dn: dn.clone() };
            if apply(c, ldap, op, OperationClass::Destructive).await? == Applied::DryRun {
                return Ok(format!(
                    "kuru çalıştırma, uygulanacaktı: hesap silinir ({dn})"
                ));
            }
            format!("hesap silindi: {dn}")
        }
        None => "hesap hedefte zaten yok; bağlantı silindi işaretlendi".to_string(),
    };
    sqlx::query(
        "UPDATE account_links SET deleted_by_us_at = now(), applied_state = 'deleted' \
         WHERE identity_id = $1 AND target_system_id = $2",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .execute(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("bağlantı güncellenemedi: {e}")))?;
    let finalized = finalize_if_last_account(c).await?;
    Ok(if finalized {
        format!("{note}; son hesap: kimlik silindi, kişisel veri temizlendi")
    } else {
        note
    })
}

async fn finalize_if_last_account(c: &AdJob<'_>) -> Result<bool, JobError> {
    let db = |e: sqlx::Error| JobError::Failed(format!("kimlik kapanışı yazılamadı: {e}"));
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM account_links WHERE identity_id = $1 AND deleted_by_us_at IS NULL",
    )
    .bind(c.job.identity_id)
    .fetch_one(c.pool)
    .await
    .map_err(db)?;
    if remaining > 0 {
        return Ok(false);
    }
    if !c.input.timeline.cancelled {
        sqlx::query(
            "INSERT INTO used_names (name, kind, former_identity_id) \
             SELECT username, 'username', id FROM identities WHERE id = $1 AND username IS NOT NULL \
             UNION ALL SELECT email, 'email', id FROM identities WHERE id = $1 AND email IS NOT NULL \
             ON CONFLICT DO NOTHING",
        )
        .bind(c.job.identity_id)
        .execute(c.pool)
        .await
        .map_err(db)?;
    }
    sqlx::query(
        "UPDATE identities SET deleted_at = now(), given_name = '', surname = '', \
         employee_number = NULL, mobile_phone = NULL, national_id_enc = NULL, \
         national_id_bidx = NULL, national_id_country = NULL WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(c.job.identity_id)
    .execute(c.pool)
    .await
    .map_err(db)?;
    sqlx::query(
        "INSERT INTO audit_log (event_type, identity_id, target_system_id, detail) \
         VALUES ('identity.deleted', $1, $2, $3::jsonb)",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .bind(format!(
        "{{\"names_burned\":{}}}",
        !c.input.timeline.cancelled
    ))
    .execute(c.pool)
    .await
    .map_err(db)?;
    Ok(true)
}

// ADR-059 madde 4: accountExpires olmasi gereken durumun parcasi; bitis yoksa 0.
// AD "suresiz"i 0 ya da i64::MAX ile gosterir.
async fn sync_account_expires(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    dn: &str,
) -> Result<String, JobError> {
    const NEVER_MAX: &str = "9223372036854775807";
    let current = ad_account::read_attributes(ldap, dn, &["accountExpires"]).await?;
    let current = current
        .get("accountExpires")
        .and_then(|v| v.first())
        .map(|v| {
            if v == NEVER_MAX {
                "0".to_string()
            } else {
                v.clone()
            }
        })
        .unwrap_or_else(|| "0".to_string());
    let desired = c
        .desired
        .account_expires
        .map(ad_account::unix_to_filetime)
        .unwrap_or(0)
        .to_string();
    if current == desired {
        return Ok(String::new());
    }
    let op = WriteOp::SetAttributes {
        dn: dn.to_string(),
        changes: vec![("accountExpires".to_string(), Some(desired))],
    };
    apply(c, ldap, op, OperationClass::Attribute).await?;
    Ok(", accountExpires güncellendi".to_string())
}

// Uyelik farki yalnizca katalog gruplari uzerinden (docs/03 "Motor neye dokunur").
pub fn membership_diff(
    current: &[String],
    desired: &[String],
    catalog: &[String],
) -> (Vec<String>, Vec<String>) {
    let add = desired
        .iter()
        .filter(|g| !current.contains(g))
        .cloned()
        .collect();
    let remove = current
        .iter()
        .filter(|g| catalog.contains(g) && !desired.contains(g))
        .cloned()
        .collect();
    (add, remove)
}

async fn desired_group_guids(c: &AdJob<'_>) -> Result<Vec<String>, JobError> {
    let mut guids = Vec::new();
    for item in &c.desired.memberships {
        if let Some(guid) = catalog_guid(c.pool, *item).await? {
            guids.push(guid);
        }
    }
    Ok(guids)
}

async fn catalog_group_guids(c: &AdJob<'_>) -> Result<Vec<String>, JobError> {
    sqlx::query_scalar(
        "SELECT external_id FROM catalog_items WHERE target_system_id = $1 \
         AND kind = 'group' AND missing_since IS NULL",
    )
    .bind(c.job.target_system_id)
    .fetch_all(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("katalog grupları okunamadı: {e}")))
}

struct GroupPlan {
    to_add: Vec<String>,
    to_remove: Vec<String>,
}

// Yalnizca okuma: katalog ve hedef uyelikleri okunur, fark hesaplanir (ADR-050 freni
// bunun sonucuna bakar). OU tasimasi oznitelik sinifidir, sayaca girmez.
async fn plan_groups(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    account: &ad_account::DirectoryAccount,
) -> Result<GroupPlan, JobError> {
    let desired = desired_group_guids(c).await?;
    let catalog = catalog_group_guids(c).await?;
    let current = ad_account::member_group_guids(ldap, c.base_dn, &account.dn).await?;
    let (to_add, to_remove) = membership_diff(&current, &desired, &catalog);
    Ok(GroupPlan { to_add, to_remove })
}

// Doner: hesabin guncel DN'i (tasindiysa yenisi) ve ozet notu.
async fn apply_groups_and_ou(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    account: &ad_account::DirectoryAccount,
    plan: &GroupPlan,
) -> Result<(String, String), JobError> {
    let mut notes = Vec::new();
    let added = change_memberships(c, ldap, &account.dn, &plan.to_add, true).await?;
    if added > 0 {
        notes.push(format!("{added} grup eklendi"));
    }
    let (dn, moved) = move_if_needed(c, ldap, account).await?;
    if moved {
        notes.push("OU taşındı".to_string());
    }
    let removed = change_memberships(c, ldap, &dn, &plan.to_remove, false).await?;
    if removed > 0 {
        notes.push(format!("{removed} grup çıkarıldı"));
    }
    let note = notes.iter().map(|n| format!(", {n}")).collect::<String>();
    Ok((dn, note))
}

async fn change_memberships(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    member_dn: &str,
    guids: &[String],
    add: bool,
) -> Result<usize, JobError> {
    let mut changed = 0;
    for guid in guids {
        let Some(group_dn) = ad_account::dn_by_guid(ldap, guid).await? else {
            continue;
        };
        let member_dn = member_dn.to_string();
        let (op, class) = if add {
            (
                WriteOp::AddMember {
                    group_dn,
                    member_dn,
                },
                OperationClass::Grant,
            )
        } else {
            (
                WriteOp::RemoveMember {
                    group_dn,
                    member_dn,
                },
                OperationClass::Destructive,
            )
        };
        apply(c, ldap, op, class).await?;
        changed += 1;
    }
    Ok(changed)
}

// docs/05 OU tasima: ayni RDN ile modifyDN; hedefte CN cakisirsa CN kurali.
// Pasif OU tanimli degilse ayrilan tasinmaz (docs/04 "tanimliysa").
async fn move_if_needed(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    account: &ad_account::DirectoryAccount,
) -> Result<(String, bool), JobError> {
    let target_ou = match c.desired.container {
        Container::Unchanged | Container::Unspecified => return Ok((account.dn.clone(), false)),
        Container::Passive if passive_ou().is_none() => return Ok((account.dn.clone(), false)),
        _ => container_dn(c, ldap).await?,
    };
    let Some((rdn, parent)) = ad_account::split_dn(&account.dn) else {
        return Err(JobError::Failed(format!(
            "DN ayrıştırılamadı: {}",
            account.dn
        )));
    };
    if parent.eq_ignore_ascii_case(&target_ou) {
        return Ok((account.dn.clone(), false));
    }
    let cn = rdn.strip_prefix("CN=").unwrap_or(rdn);
    let new_rdn = if ad_account::cn_exists(ldap, &target_ou, cn).await? {
        let p = &c.input.person;
        let username = p.username.clone().unwrap_or_default();
        format!(
            "CN={}",
            ldap3::dn_escape(ad_account::cn_for(
                &p.given_name,
                &p.surname,
                Some(&username)
            ))
        )
    } else {
        rdn.to_string()
    };
    let op = WriteOp::MoveAccount {
        dn: account.dn.clone(),
        new_rdn: new_rdn.clone(),
        new_parent: target_ou.clone(),
    };
    // Kuru calistirma ve gozlem: hesap tasinmadi, sonraki okumalar eski DN'de olmali
    Ok(match apply(c, ldap, op, OperationClass::Attribute).await? {
        Applied::DryRun => (account.dn.clone(), true),
        Applied::Applied => (format!("{new_rdn},{target_ou}"), true),
    })
}

fn passive_ou() -> Option<String> {
    std::env::var("AD_PASSIVE_OU")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

struct EnabledPlan {
    op: Option<(WriteOp, OperationClass)>,
    note: &'static str,
}

impl EnabledPlan {
    fn class(&self) -> Option<OperationClass> {
        self.op.as_ref().map(|(_, class)| *class)
    }
}

fn plan_enabled(
    link: &LinkRow,
    account: &ad_account::DirectoryAccount,
    enabled: bool,
    transition: bool,
) -> EnabledPlan {
    let set = |enabled: bool| WriteOp::SetEnabled {
        dn: account.dn.clone(),
        enabled,
    };
    let plan = |op: WriteOp, class: OperationClass, note: &'static str| EnabledPlan {
        op: Some((op, class)),
        note,
    };
    match (enabled, account.enabled) {
        (true, false) if transition => {
            // ADR-030: `ayrildi`dan donus yikici sayilir
            let class = if link.applied_state.as_deref() == Some("departed") {
                OperationClass::Destructive
            } else {
                OperationClass::Attribute
            };
            plan(set(true), class, "etkinleştirildi")
        }
        (true, false) => EnabledPlan {
            op: None,
            note: "hedefte elle pasifleştirilmiş, korunuyor (ADR-032)",
        },
        (false, true) => plan(set(false), OperationClass::Destructive, "pasifleştirildi"),
        _ => EnabledPlan {
            op: None,
            note: "hesap durumu zaten uyumlu",
        },
    }
}

async fn apply_enabled(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    plan: EnabledPlan,
) -> Result<(Applied, &'static str), JobError> {
    let applied = match plan.op {
        Some((op, class)) => apply(c, ldap, op, class).await?,
        None => Applied::Applied,
    };
    Ok((applied, plan.note))
}

// docs/05 Oznitelik guncelleme: eslenen oznitelikler okunur, fark tek modify ile
// yazilir; "sadece bossa yaz" dolu degeri korur (ADR-034), belirsiz kaynak atlanir.
async fn sync_attributes(c: &AdJob<'_>, ldap: &mut Ldap, dn: &str) -> Result<String, JobError> {
    if c.mappings.is_empty() {
        return Ok(String::new());
    }
    let names: Vec<&str> = c.mappings.iter().map(|r| r.attribute.as_str()).collect();
    let current = ad_account::read_attributes(ldap, dn, &names).await?;
    let s = sources(c, ldap, None).await?;
    let changes = mapping::changes(c.mappings, &s, &current);
    if changes.is_empty() {
        return Ok(String::new());
    }
    let count = changes.len();
    let op = WriteOp::SetAttributes {
        dn: dn.to_string(),
        changes,
    };
    apply(c, ldap, op, OperationClass::Attribute).await?;
    Ok(format!(", {count} öznitelik güncellendi"))
}
// --- END FEATURE: engine ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    // Lab senaryolari freni sinamaz; sayaclar bol tutulur (fren testi counters.rs'te).
    const LAB_LIMITS: counters::Limits = counters::Limits {
        destructive: 1000,
        grant: 1000,
        first_password: 1000,
        emergency_quota: 1000,
    };

    // ADR-018/086: ipuclu kayit hesap acmaz; retler mudahale (kapali, kuru, yok, kapsam disi,
    // yasakli grup, sicil, baska kimlige bagli); kabulde gozlem baglantisi + ad uyarisi;
    // sonraki iste hicbir sey uygulanmaz.
    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn adopts_existing_lab_account_in_observed_mode_after_rule_checks() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let key = [7u8; crate::crypto::KEY_LEN];
        let cfg = test_support::configure_lab_ad(
            &pool,
            &key,
            &var("AD_LAB_URL"),
            &var("AD_LAB_BIND_DN"),
            &var("AD_LAB_PASSWORD"),
            &var("AD_CA_FILE"),
        )
        .await;
        test_support::point_model_at_real_catalog(&pool, &seed, &cfg).await;
        // SAFETY: tek is parcacikli test; ayni degiskenleri eszamanli degistiren baska test yok.
        unsafe {
            std::env::set_var("AD_MANAGED_USER_OUS", "OU=Personel,DC=opensicil,DC=lab");
            std::env::set_var("AD_MANAGED_GROUP_OUS", "OU=Gruplar,DC=opensicil,DC=lab");
        }
        let mut ldap = ad::connect(&cfg).await.unwrap();
        let base = ad::base_dn(&mut ldap).await.unwrap();
        let existing = adoption::find_by_sam(&mut ldap, &base, "mevcut.personel", None)
            .await
            .unwrap()
            .expect("seed.sh mevcut.personel hesabını açar");
        let domain_admins = format!("CN=Domain Admins,CN=Users,{base}");
        async fn membership(
            ldap: &mut Ldap,
            group: &str,
            dn: &str,
            add: bool,
        ) -> Result<(), ldap3::LdapError> {
            let values = std::collections::HashSet::from([dn]);
            let m = if add {
                ldap3::Mod::Add("member", values)
            } else {
                ldap3::Mod::Delete("member", values)
            };
            ldap.modify(group, vec![m])
                .await
                .and_then(|r| r.success().map(|_| ()))
        }
        async fn set_attr(
            ldap: &mut Ldap,
            dn: &str,
            attr: &str,
            value: Option<&str>,
        ) -> Result<(), ldap3::LdapError> {
            let m = match value {
                Some(v) => ldap3::Mod::Replace(attr, std::collections::HashSet::from([v])),
                None => ldap3::Mod::Delete(attr, std::collections::HashSet::new()),
            };
            ldap.modify(dn, vec![m])
                .await
                .and_then(|r| r.success().map(|_| ()))
        }
        // onceki yarim kalan calismanin izleri
        let _ = membership(&mut ldap, &domain_admins, &existing.dn, false).await;
        let _ = set_attr(&mut ldap, &existing.dn, "employeeID", None).await;
        let _ = set_attr(&mut ldap, &existing.dn, "telephoneNumber", None).await;
        let _ = set_attr(&mut ldap, &existing.dn, "adminCount", None).await;

        let set_hint = |hint: &'static str, identity: i64| {
            let pool = pool.clone();
            async move {
                // sicil no yalnizca ana kimlige: tekil sutun, ikinci kimlik carpismasin
                sqlx::query(
                    "UPDATE identities SET existing_ad_account_hint = $2, \
                     employee_number = CASE WHEN id = $3 THEN '123' ELSE employee_number END \
                     WHERE id = $1",
                )
                .bind(identity)
                .bind(hint)
                .bind(seed.identity)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        set_hint("mevcut.personel", seed.identity).await;
        let job = ClaimedJob {
            id: test_support::enqueue(&pool, seed.identity, seed.ad, 1).await,
            identity_id: seed.identity,
            target_system_id: seed.ad,
            priority: 1,
            attempts: 0,
        };
        let job = crate::queue::claim(&pool, "w1", &[])
            .await
            .unwrap()
            .unwrap_or(job);
        let ca = var("AD_CA_FILE");
        let env = |ownership: bool, dry_run: bool| EngineEnv {
            time_zone: "Europe/Istanbul",
            mode: Mode { dry_run },
            aead_key: &key,
            ad_ca_file: Some(&ca),
            worker_id: "w1",
            sensitive_mapping_enabled: false,
            first_login_change_required: true,
            ownership_mode_enabled: ownership,
            limits: LAB_LIMITS,
        };
        let intervention = |run: Result<String, JobError>| match run {
            Err(JobError::NeedsIntervention(reason)) => reason,
            other => panic!("müdahale beklenirdi: {other:?}"),
        };

        let r = intervention(run_job(&pool, &job, &env(false, false)).await);
        assert!(r.contains("sahiplenme kapalı"), "{r}");
        let r = intervention(run_job(&pool, &job, &env(true, true)).await);
        assert!(r.contains("kuru çalıştırma"), "{r}");
        set_hint("yok.boyle.biri", seed.identity).await;
        let r = intervention(run_job(&pool, &job, &env(true, false)).await);
        assert!(r.contains("ipucu bulunamadı"), "{r}");
        set_hint("Administrator", seed.identity).await;
        let r = intervention(run_job(&pool, &job, &env(true, false)).await);
        assert!(r.contains("OU'larının dışında"), "{r}");
        set_hint("mevcut.personel", seed.identity).await;
        membership(&mut ldap, &domain_admins, &existing.dn, true)
            .await
            .unwrap();
        let r = intervention(run_job(&pool, &job, &env(true, false)).await);
        membership(&mut ldap, &domain_admins, &existing.dn, false)
            .await
            .unwrap();
        assert!(r.contains("yasaklı grup üyesi: Domain Admins"), "{r}");
        // adminCount yapiskandir: gruptan cikmis olsa da reddedilir (docs/05)
        set_attr(&mut ldap, &existing.dn, "adminCount", Some("1"))
            .await
            .unwrap();
        let r = intervention(run_job(&pool, &job, &env(true, false)).await);
        set_attr(&mut ldap, &existing.dn, "adminCount", None)
            .await
            .unwrap();
        assert!(r.contains("adminCount dolu"), "{r}");
        set_attr(&mut ldap, &existing.dn, "employeeID", Some("124"))
            .await
            .unwrap();
        let r = intervention(run_job(&pool, &job, &env(true, false)).await);
        assert!(r.contains("sicil no"), "{r}");
        set_attr(&mut ldap, &existing.dn, "employeeID", Some("0123"))
            .await
            .unwrap();
        // sabit hat E.164 degil: kimlige yazilmaz, uyari is sonucuna girer (ADR-106)
        set_attr(
            &mut ldap,
            &existing.dn,
            "telephoneNumber",
            Some("01632 960001"),
        )
        .await
        .unwrap();

        let adopted = run_job(&pool, &job, &env(true, false)).await.unwrap();
        assert!(
            adopted.starts_with("sahiplenildi (gözlem modu)"),
            "{adopted}"
        );
        assert!(adopted.contains("ad-soyad kimlikle uyuşmuyor"), "{adopted}");
        // fark sahiplenme isinde hesaplanir: operator yonetime almadan once gorur
        assert!(adopted.contains("yönetime alınırsa"), "{adopted}");
        let (origin, mode, mismatch, external_id): (String, String, bool, String) = sqlx::query_as(
            "SELECT origin, mode, name_mismatch, external_id FROM account_links WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (
                origin.as_str(),
                mode.as_str(),
                mismatch,
                external_id.as_str()
            ),
            ("adopted", "observed", true, existing.guid.as_str())
        );
        let (username, upn): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT username, upn FROM identities WHERE id = $1")
                .bind(seed.identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(username.as_deref(), Some("mevcut.personel"));
        assert!(
            upn.is_some_and(|u| u.contains('@')),
            "UPN hedeften okunur (ADR-034)"
        );
        assert!(adopted.contains("E.164 biçiminde değil"), "{adopted}");
        let (sicil, cep): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT employee_number, mobile_phone FROM identities WHERE id = $1")
                .bind(seed.identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (sicil.as_deref(), cep),
            (Some("123"), None),
            "dolu sicil ezilmez, sabit hat cep alanına yazılmaz"
        );

        // gozlem: sonraki is hicbir sey yazmaz
        let observed = run_job(&pool, &job, &env(true, false)).await.unwrap();
        assert!(observed.starts_with("gözlem modunda"), "{observed}");
        let intents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE identity_id = $1 AND operation_class IS NOT NULL",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(intents, 0, "gözlemde hedefe yazma niyeti yok");

        // ayni hesap ikinci kimlige baglanamaz
        set_hint("mevcut.personel", seed.other_identity).await;
        let other = ClaimedJob {
            id: test_support::enqueue(&pool, seed.other_identity, seed.ad, 1).await,
            identity_id: seed.other_identity,
            target_system_id: seed.ad,
            priority: 1,
            attempts: 0,
        };
        let other = crate::queue::claim(&pool, "w1", &[])
            .await
            .unwrap()
            .unwrap_or(other);
        let r = intervention(run_job(&pool, &other, &env(true, false)).await);
        assert!(r.contains("kimliğine bağlı"), "{r}");

        // --- fark gorunumu ve yonetime alma (3e-2, ADR-018/087) ---
        // Kurumun kendi actigi, OpenSicil'in bilmedigi bir hesap sahiplenilir:
        // gozlem isi farki yazar ama hedefe dokunmaz; onay gelince ayni fark uygulanir.
        let own_dn = format!("CN=Devralinacak Hesap,OU=Personel,{base}");
        let _ = ldap.delete(&own_dn).await; // onceki yarim kalan kosu
        let attrs: Vec<(Vec<u8>, std::collections::HashSet<Vec<u8>>)> = vec![
            (
                b"objectClass".to_vec(),
                std::collections::HashSet::from([b"user".to_vec()]),
            ),
            (
                b"sAMAccountName".to_vec(),
                std::collections::HashSet::from([b"devralinacak".to_vec()]),
            ),
            (
                b"givenName".to_vec(),
                std::collections::HashSet::from(["Ali".as_bytes().to_vec()]),
            ),
            (
                b"sn".to_vec(),
                std::collections::HashSet::from(["Kaya".as_bytes().to_vec()]),
            ),
            (
                b"userAccountControl".to_vec(),
                std::collections::HashSet::from([b"512".to_vec()]),
            ),
            // Sicil `employeeNumber`da: `employeeID` bos kalinca oraya duser (ADR-106 madde 2)
            (
                b"employeeNumber".to_vec(),
                std::collections::HashSet::from([b"7788".to_vec()]),
            ),
            (
                b"mobile".to_vec(),
                std::collections::HashSet::from([b"+905321234567".to_vec()]),
            ),
            (
                b"unicodePwd".to_vec(),
                // kimsenin bilmesi gerekmeyen parola: sahiplenme bu hesapla bind etmez
                std::collections::HashSet::from([ad_account::unicode_pwd(
                    &ad_account::random_password(),
                )]),
            ),
        ];
        ldap.add(&own_dn, attrs)
            .await
            .unwrap()
            .success()
            .expect("lab hesabı açılamadı");
        set_hint("devralinacak", seed.other_identity).await;
        let adopted = run_job(&pool, &other, &env(true, false)).await.unwrap();
        assert!(
            adopted.starts_with("sahiplenildi (gözlem modu)"),
            "{adopted}"
        );
        // bos kimlik: sicil `employeeNumber`dan, cep `mobile`dan dolar (ADR-106)
        let (sicil, cep): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT employee_number, mobile_phone FROM identities WHERE id = $1")
                .bind(seed.other_identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (sicil.as_deref(), cep.as_deref()),
            (Some("7788"), Some("+905321234567"))
        );

        let diff = run_job(&pool, &other, &env(true, false)).await.unwrap();
        assert!(
            diff.starts_with("gözlem modunda, yönetime alınırsa:"),
            "{diff}"
        );
        assert!(diff.contains("1 grup eklendi"), "{diff}");
        assert!(diff.contains("OU taşındı"), "{diff}");
        let observed_intents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE identity_id = $1 AND operation_class IS NOT NULL",
        )
        .bind(seed.other_identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(observed_intents, 0, "gözlem farkı hedefe yazmaz");
        let still_there = ad_account::read_attributes(&mut ldap, &own_dn, &["memberOf"])
            .await
            .unwrap();
        assert!(
            !still_there.contains_key("memberOf"),
            "gözlemde gruba eklenmez"
        );

        // operatorun onayi (backend yalnizca bu kolonu yazar, ADR-087)
        sqlx::query("UPDATE account_links SET manage_requested_at = now() WHERE identity_id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        let applied = run_job(&pool, &other, &env(true, false)).await.unwrap();
        assert!(applied.contains("applied_state active"), "{applied}");
        let (mode, state): (String, Option<String>) =
            sqlx::query_as("SELECT mode, applied_state FROM account_links WHERE identity_id = $1")
                .bind(seed.other_identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (mode.as_str(), state.as_deref()),
            ("managed", Some("active"))
        );
        let managed_events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE event_type = $1")
                .bind(adoption::MANAGED_EVENT)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(managed_events, 1);
        let moved_dn =
            ad_account::dn_by_guid(&mut ldap, &external_id_of(&pool, seed.other_identity).await)
                .await
                .unwrap()
                .expect("hesap hedefte");
        assert!(
            moved_dn.contains("OU=SistemUzmanlari"),
            "OU taşınmış olmalı: {moved_dn}"
        );
        let after =
            ad_account::read_attributes(&mut ldap, &moved_dn, &["memberOf", "sAMAccountName"])
                .await
                .unwrap();
        assert!(
            after["memberOf"].iter().any(|g| g.contains("GG-VPN")),
            "{after:?}"
        );
        // ADR-034: yonetime alma mevcut hesabin kullanici adini degistirmez
        assert_eq!(after["sAMAccountName"], vec!["devralinacak".to_string()]);
        ldap.delete(&moved_dn).await.unwrap().success().unwrap();

        set_attr(&mut ldap, &existing.dn, "employeeID", None)
            .await
            .unwrap();
        set_attr(&mut ldap, &existing.dn, "telephoneNumber", None)
            .await
            .unwrap();
        ldap.unbind().await.unwrap();
        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    async fn external_id_of(pool: &sqlx::PgPool, identity: i64) -> String {
        sqlx::query_scalar("SELECT external_id FROM account_links WHERE identity_id = $1")
            .bind(identity)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // ADR-019/046/085: kuru modda red; acik modda pwdLastSet 0; kapali modda damga
    // baglantiya yazilir ve parola gercekten bind eder; kullanilmis hesap reddedilir.
    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn issues_first_password_in_lab_only_to_unused_account() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let key = [7u8; crate::crypto::KEY_LEN];
        let cfg = test_support::configure_lab_ad(
            &pool,
            &key,
            &var("AD_LAB_URL"),
            &var("AD_LAB_BIND_DN"),
            &var("AD_LAB_PASSWORD"),
            &var("AD_CA_FILE"),
        )
        .await;
        test_support::point_model_at_real_catalog(&pool, &seed, &cfg).await;
        test_support::delete_lab_accounts(&cfg, "parola.test*").await;
        sqlx::query("UPDATE identities SET given_name = 'Parola', surname = $2 WHERE id = $1")
            .bind(seed.identity)
            .bind(format!("Test{}", std::process::id() % 100_000))
            .execute(&pool)
            .await
            .unwrap();
        let job = ClaimedJob {
            id: test_support::enqueue(&pool, seed.identity, seed.ad, 1).await,
            identity_id: seed.identity,
            target_system_id: seed.ad,
            priority: 1,
            attempts: 0,
        };
        let job = crate::queue::claim(&pool, "w1", &[])
            .await
            .unwrap()
            .unwrap_or(job);
        let ca = var("AD_CA_FILE");
        let env = |dry_run: bool, change_required: bool| EngineEnv {
            time_zone: "Europe/Istanbul",
            mode: Mode { dry_run },
            aead_key: &key,
            ad_ca_file: Some(&ca),
            worker_id: "w1",
            sensitive_mapping_enabled: false,
            first_login_change_required: change_required,
            ownership_mode_enabled: false,
            limits: LAB_LIMITS,
        };
        // ADR-056: kayitla acilan istek hesap acilir acilmaz ayni iste karsilanir
        let at_provision: i64 = sqlx::query_scalar(
            "INSERT INTO first_passwords (identity_id, target_system_id, requested_by) \
             VALUES ($1, $2, 'ik') RETURNING id",
        )
        .bind(seed.identity)
        .bind(seed.ad)
        .fetch_one(&pool)
        .await
        .unwrap();
        let live = run_job(&pool, &job, &env(false, true)).await.unwrap();
        assert!(live.starts_with("hesap açıldı"), "{live}");
        assert!(live.contains("ilk parola verildi"), "{live}");
        let request = || {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO first_passwords (identity_id, target_system_id, requested_by) \
                     VALUES ($1, $2, 'ik') RETURNING id",
                )
                .bind(seed.identity)
                .bind(seed.ad)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let outcome = |id: i64| {
            let pool = pool.clone();
            async move {
                let (enc, error): (Option<Vec<u8>>, Option<String>) =
                    sqlx::query_as("SELECT password_enc, error FROM first_passwords WHERE id = $1")
                        .bind(id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                (
                    enc.map(|e| {
                        String::from_utf8(crate::crypto::decrypt_versioned(&key, &e).unwrap())
                            .unwrap()
                    }),
                    error,
                )
            }
        };

        // kuru calistirma: istek hemen reddedilir, parola yazilmaz
        let dry_request = request().await;
        let dry = run_job(&pool, &job, &env(true, true)).await.unwrap();
        assert!(dry.contains("ilk parola reddedildi: kuru"), "{dry}");
        assert_eq!(
            outcome(dry_request).await,
            (None, Some(first_password::REJECT_DRY_RUN.to_string()))
        );

        let (at_provision_password, _) = outcome(at_provision).await;
        assert!(
            at_provision_password.is_some(),
            "açılışta verilen parola yazılmalı"
        );

        // acik mod: parola yazilir, pwdLastSet 0
        let open_request = request().await;
        let issued = run_job(&pool, &job, &env(false, true)).await.unwrap();
        assert!(issued.contains("ilk parola verildi"), "{issued}");
        let (password, error) = outcome(open_request).await;
        let password = password.expect("şifreli parola yazılmalı");
        assert!(error.is_none());
        assert_eq!(password.len(), 19, "{password}");
        assert_eq!(password.split('-').count(), 4, "{password}");
        let guid: String =
            sqlx::query_scalar("SELECT external_id FROM account_links WHERE identity_id = $1")
                .bind(seed.identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        let mut ldap = ad::connect(&cfg).await.unwrap();
        let account = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(account.pwd_last_set.as_deref(), Some("0"));

        // kapali mod: damga baglantiya yazilir, parola gercekten bind eder
        let closed_request = request().await;
        let issued = run_job(&pool, &job, &env(false, false)).await.unwrap();
        assert!(issued.contains("ilk parola verildi"), "{issued}");
        let (password, _) = outcome(closed_request).await;
        let password = password.expect("şifreli parola yazılmalı");
        let stamp: Option<String> = sqlx::query_scalar(
            "SELECT first_password_pwd_last_set FROM account_links WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        let account = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        assert!(stamp.as_deref().is_some_and(|s| s != "0"), "{stamp:?}");
        assert_eq!(account.pwd_last_set, stamp);
        let bound = ldap
            .simple_bind(&account.dn, &password)
            .await
            .unwrap()
            .success();
        assert!(bound.is_ok(), "teslim edilen parola bind etmeli: {bound:?}");
        // ADR-046 lab sorusu: Samba simple bind'da lastLogonTimestamp yazar mi?
        let after_bind = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            after_bind.last_logon_timestamp.is_some(),
            "Samba simple bind lastLogonTimestamp yazmalı (docs/05 Samba tablosu)"
        );
        ldap.unbind().await.unwrap();

        // kullanilmis hesap: damga tutmuyor (kisi parolasini degistirmis) → red
        sqlx::query(
            "UPDATE account_links SET first_password_pwd_last_set = '1' WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .execute(&pool)
        .await
        .unwrap();
        let used_request = request().await;
        let rejected = run_job(&pool, &job, &env(false, false)).await.unwrap();
        assert!(
            rejected.contains("ilk parola reddedildi: hesap"),
            "{rejected}"
        );
        assert_eq!(
            outcome(used_request).await,
            (None, Some(first_password::REJECT_USED.to_string()))
        );
        let (intents, leaked): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*) FILTER (WHERE operation_class = 'first_password'), \
             COUNT(*) FILTER (WHERE detail::text LIKE $2) FROM audit_log \
             WHERE event_type = 'ad.account.first_password' AND identity_id = $1",
        )
        .bind(seed.identity)
        .bind(format!("%{password}%"))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (intents, leaked),
            (3, 0),
            "açılış + iki istek = üç niyet, parola denetime girmez"
        );

        // ADR-048/056: giris yapilmis hesabin kaydi iptal edilemez — iptal reddedilir,
        // hesap silinmez, ayrilis olarak uygulanir (yukarida gercek bind yapildi).
        sqlx::query(
            "UPDATE identities SET cancelled = TRUE, end_at = now() - interval '1 hour' \
             WHERE id = $1",
        )
        .bind(seed.identity)
        .execute(&pool)
        .await
        .unwrap();
        let cancel = run_job(&pool, &job, &env(false, false)).await.unwrap();
        assert!(cancel.contains("iptal reddedildi"), "{cancel}");
        let verified: Option<bool> =
            sqlx::query_scalar("SELECT verified_unused FROM account_links WHERE identity_id = $1")
                .bind(seed.identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(verified, Some(false), "giriş yapılmış: doğrulanmadı");
        let mut ldap = ad::connect(&cfg).await.unwrap();
        assert!(
            ad_account::find_by_guid(&mut ldap, &guid)
                .await
                .unwrap()
                .is_some(),
            "iptal reddedilince hesap silinmez"
        );
        ldap.unbind().await.ok();

        test_support::delete_lab_accounts(&cfg, "parola.test*").await;
        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // Testte hedefteki elle mudahaleyi taklit eder (ADR-032): UAC'yi dogrudan yazar.
    async fn set_uac(ldap: &mut Ldap, dn: &str, value: &str) {
        ldap.modify(
            dn,
            vec![ldap3::Mod::Replace(
                "userAccountControl",
                std::collections::HashSet::from([value]),
            )],
        )
        .await
        .unwrap()
        .success()
        .unwrap();
    }

    #[test]
    fn membership_diff_only_touches_catalog_groups() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let (add, remove) = membership_diff(
            &s(&["vpn", "elle-eklenen", "eski-rol"]),
            &s(&["vpn", "nobet"]),
            &s(&["vpn", "nobet", "eski-rol"]),
        );
        assert_eq!(add, s(&["nobet"]));
        assert_eq!(remove, s(&["eski-rol"]), "katalog dışı grup dokunulmaz");
    }

    #[test]
    fn state_names_match_account_links_check() {
        assert_eq!(state_name(LifecycleState::Pending), "pending");
        assert_eq!(state_name(LifecycleState::Deleted), "deleted");
    }

    // Postgres + lab Samba AD: kayit → AD'de hesap (etkin, gruplu) → baglanti →
    // ikinci calisma degisiklik uretmez; kuru mod hedefe dokunmaz.
    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn provisions_account_in_lab_then_is_idempotent() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let key = [7u8; crate::crypto::KEY_LEN];
        let cfg = test_support::configure_lab_ad(
            &pool,
            &key,
            &var("AD_LAB_URL"),
            &var("AD_LAB_BIND_DN"),
            &var("AD_LAB_PASSWORD"),
            &var("AD_CA_FILE"),
        )
        .await;
        test_support::point_model_at_real_catalog(&pool, &seed, &cfg).await;
        test_support::delete_lab_accounts(&cfg, "motor.test*").await;
        test_support::delete_lab_accounts(&cfg, "ozel.testx*").await;
        sqlx::query("UPDATE identities SET given_name = 'Motor', surname = $2 WHERE id = $1")
            .bind(seed.identity)
            .bind(format!("Test{}", std::process::id() % 100_000))
            .execute(&pool)
            .await
            .unwrap();
        let job = ClaimedJob {
            id: test_support::enqueue(&pool, seed.identity, seed.ad, 1).await,
            identity_id: seed.identity,
            target_system_id: seed.ad,
            priority: 1,
            attempts: 0,
        };
        let job = crate::queue::claim(&pool, "w1", &[])
            .await
            .unwrap()
            .unwrap_or(job);
        let ca = var("AD_CA_FILE");
        let env = |dry_run: bool| EngineEnv {
            time_zone: "Europe/Istanbul",
            mode: Mode { dry_run },
            aead_key: &key,
            ad_ca_file: Some(&ca),
            worker_id: "w1",
            sensitive_mapping_enabled: false,
            first_login_change_required: true,
            ownership_mode_enabled: false,
            limits: LAB_LIMITS,
        };

        let dry = run_job(&pool, &job, &env(true)).await.unwrap();
        assert!(dry.starts_with("kuru çalıştırma"), "{dry}");
        let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account_links")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(links, 0, "kuru modda bağlantı yazılmaz");

        let live = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(live.starts_with("hesap açıldı"), "{live}");
        let (guid, applied): (String, Option<String>) = sqlx::query_as(
            "SELECT external_id, applied_state FROM account_links WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(applied.as_deref(), Some("active"));
        let (username, upn): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT username, upn FROM identities WHERE id = $1")
                .bind(seed.identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            username.as_deref().unwrap_or("").starts_with("motor.test"),
            "{username:?}"
        );
        assert!(
            upn.as_deref().unwrap_or("").ends_with("@example.local"),
            "{upn:?}"
        );
        let mut ldap = ad::connect(&cfg).await.unwrap();
        let account = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .expect("hesap AD'de olmalı");
        assert!(account.enabled, "başlangıç geçmiş: etkin");
        assert!(
            account.member_of.iter().any(|g| g.contains("GG-VPN")),
            "{:?}",
            account.member_of
        );
        assert!(
            account
                .dn
                .to_ascii_lowercase()
                .contains("ou=sistemuzmanlari"),
            "{}",
            account.dn
        );

        let again = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(again.contains("zaten uyumlu"), "{again}");
        let intents: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE operation_class IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            intents, 3,
            "hesap + grup + etkinleştirme; ikinci çalışma yazmadı"
        );

        // ADR-032: hedefte elle pasiflestirilen hesap korunur — durum gecisi olmadigi
        // icin motor yeniden etkinlestirmez. Testin kalani etkin hesap bekler: elle geri acilir.
        set_uac(&mut ldap, &account.dn, "514").await;
        let kept = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(!kept.contains("etkinleştirildi"), "{kept}");
        assert!(
            !ad_account::find_by_guid(&mut ldap, &guid)
                .await
                .unwrap()
                .unwrap()
                .enabled,
            "ADR-032: elle pasifleştirilen hesap korunur"
        );
        set_uac(&mut ldap, &account.dn, "512").await;

        // ADR-012/034/082: esleme — replace, "sadece bossa yaz" korur, hassas kaynak kapisi
        ldap.modify(
            &account.dn,
            vec![ldap3::Mod::Replace(
                "physicalDeliveryOfficeName",
                std::collections::HashSet::from(["Elle"]),
            )],
        )
        .await
        .unwrap()
        .success()
        .unwrap();
        sqlx::query(
            "INSERT INTO attribute_mappings (target_system_id, target_attribute, source_kind, source_text, write_if_empty) \
             VALUES ($1, 'description', 'constant', 'Personel', FALSE), \
                    ($1, 'physicalDeliveryOfficeName', 'constant', 'Ankara', TRUE), \
                    ($1, 'company', 'constant', 'Kurum', TRUE)",
        )
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();
        let mapped = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(mapped.contains("2 öznitelik güncellendi"), "{mapped}");
        let attrs = ad_account::read_attributes(
            &mut ldap,
            &account.dn,
            &[
                "description",
                "physicalDeliveryOfficeName",
                "company",
                "displayName",
            ],
        )
        .await
        .unwrap();
        assert_eq!(attrs["description"], vec!["Personel"]);
        assert_eq!(
            attrs["physicalDeliveryOfficeName"],
            vec!["Elle"],
            "boşsa yaz: dolu değer korunur"
        );
        assert_eq!(
            attrs["company"],
            vec!["Kurum"],
            "ADR-034: boşsa yaz, boş özniteliği doldurur"
        );
        assert!(attrs["displayName"][0].starts_with("Motor Test"));
        sqlx::query(
            "INSERT INTO attribute_mappings (target_system_id, target_attribute, source_kind) \
             VALUES ($1, 'mobile', 'mobile_phone')",
        )
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();
        let gated = run_job(&pool, &job, &env(false)).await;
        assert!(
            matches!(&gated, Err(JobError::NeedsIntervention(r)) if r.contains("hassas")),
            "{gated:?}"
        );
        sqlx::query("DELETE FROM attribute_mappings WHERE target_attribute = 'mobile'")
            .execute(&pool)
            .await
            .unwrap();

        // Gorev degisikligi (ADR-050/083): rol grubu VPN → Nobet, OU SistemUzmanlari → Personel;
        // once ekleme, sonra tasima, sonra cikarma.
        let real_item = |name: &'static str| {
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM catalog_items WHERE display_name = $1 AND target_system_id = $2 \
                 AND external_id NOT LIKE 'GG-%' AND external_id NOT IN ('Personel', 'SistemUzmanlari')",
            )
            .bind(name)
            .bind(seed.ad)
            .fetch_one(&pool)
        };
        let nobet = real_item("GG-Nobet").await.unwrap();
        let personel_ou = real_item("Personel").await.unwrap();
        sqlx::query("UPDATE role_entitlements SET catalog_item_id = $1 WHERE role_id IN (SELECT id FROM roles WHERE kind = 'primary')")
            .bind(nobet)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE role_target_settings SET container_item_id = $1")
            .bind(personel_ou)
            .execute(&pool)
            .await
            .unwrap();

        // ADR-050 kapanis testi: verme sayaci doluyken gorev degisikligi isi eski
        // gruplari CIKARMIYOR — hicbir islem uygulamadan bekliyor; pencere acilinca
        // ekleme ve cikarmayi birlikte uyguluyor.
        let narrow = counters::Limits {
            grant: 1,
            ..LAB_LIMITS
        };
        let open_window = |pool: sqlx::PgPool| async move {
            sqlx::query("UPDATE audit_log SET occurred_at = now() - interval '2 hours'")
                .execute(&pool)
                .await
                .unwrap();
        };
        let fill_grant = |pool: sqlx::PgPool, identity: i64, target: i64| async move {
            sqlx::query(
                "INSERT INTO audit_log (event_type, identity_id, target_system_id, \
                 operation_class, detail) VALUES ('t', $1, $2, 'grant', '{}'::jsonb)",
            )
            .bind(identity)
            .bind(target)
            .execute(&pool)
            .await
            .unwrap();
        };
        open_window(pool.clone()).await;
        fill_grant(pool.clone(), seed.other_identity, seed.ad).await;
        let throttled = run_job(
            &pool,
            &job,
            &EngineEnv {
                limits: narrow,
                ..env(false)
            },
        )
        .await;
        assert!(
            matches!(&throttled, Err(JobError::Throttled(b)) if b.class == OperationClass::Grant),
            "{throttled:?}"
        );
        let untouched = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            untouched.member_of.iter().any(|g| g.contains("GG-VPN")),
            "fren: eski grup çıkarılmadı {:?}",
            untouched.member_of
        );
        assert!(
            untouched
                .dn
                .to_ascii_lowercase()
                .contains("ou=sistemuzmanlari"),
            "fren: OU taşınmadı {}",
            untouched.dn
        );
        open_window(pool.clone()).await;

        let moved = run_job(&pool, &job, &env(false)).await.unwrap();
        for expected in ["1 grup eklendi", "OU taşındı", "1 grup çıkarıldı"] {
            assert!(moved.contains(expected), "{moved}");
        }
        let account = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .expect("hesap taşındı ama duruyor");
        let dn = account.dn.to_ascii_lowercase();
        assert!(
            dn.ends_with("ou=personel,dc=opensicil,dc=lab") && !dn.contains("sistemuzmanlari"),
            "{}",
            account.dn
        );
        assert!(account.member_of.iter().any(|g| g.contains("GG-Nobet")));
        assert!(!account.member_of.iter().any(|g| g.contains("GG-VPN")));
        // SAFETY: tek is parcacikli test; ayni degiskeni eszamanli degistiren baska test yok.
        unsafe {
            std::env::set_var("AD_PASSIVE_OU", "OU=Pasif,OU=Personel,DC=opensicil,DC=lab");
        }

        // ADR-053/059: tarihli aski — hesap pasiflesir ama uyelik ve OU korunur (ayrilis degil);
        // aski kalkinca yeniden etkinlesir. Pasif OU tanimli, yine tasinmaz.
        sqlx::query(
            "UPDATE identities SET suspension_start = current_date - 1, \
             suspension_end = current_date + 7 WHERE id = $1",
        )
        .bind(seed.identity)
        .execute(&pool)
        .await
        .unwrap();
        let suspended = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(suspended.contains("pasifleştirildi"), "{suspended}");
        let on_leave = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        assert!(!on_leave.enabled, "askıda: hesap pasif");
        assert!(
            on_leave.member_of.iter().any(|g| g.contains("GG-Nobet")),
            "askıda üyelik korunur: {:?}",
            on_leave.member_of
        );
        assert!(
            !on_leave.dn.to_ascii_lowercase().contains("ou=pasif"),
            "askıda OU korunur: {}",
            on_leave.dn
        );
        sqlx::query(
            "UPDATE identities SET suspension_start = NULL, suspension_end = NULL WHERE id = $1",
        )
        .bind(seed.identity)
        .execute(&pool)
        .await
        .unwrap();
        let lifted = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(lifted.contains("etkinleştirildi"), "{lifted}");

        // ADR-040: rol artik "hesap acilsin = hayir" diyor; bagli hesap yine yonetilir.
        for sql in [
            "UPDATE role_target_settings SET provision_account = FALSE WHERE target_system_id = $1",
            "UPDATE department_target_settings SET provision_account = FALSE WHERE target_system_id = $1",
            "UPDATE target_systems SET provision_account_default = FALSE WHERE id = $1",
        ] {
            sqlx::query(sql).bind(seed.ad).execute(&pool).await.unwrap();
        }

        // ayrilis: pasiflestirme (yikici niyet), applied_state departed; Ali'nin yoneticisi
        // Ayse → ADR-041 asta is acilir
        sqlx::query("UPDATE identities SET manager_id = $1 WHERE id = $2")
            .bind(seed.identity)
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE identities SET end_at = now() - interval '1 hour' WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        // ADR-050: yalnizca yikici is verme sayacindan etkilenmez — ayrilis dolu
        // verme penceresinde de uygulanir.
        open_window(pool.clone()).await;
        fill_grant(pool.clone(), seed.other_identity, seed.ad).await;
        let departed = run_job(
            &pool,
            &job,
            &EngineEnv {
                limits: narrow,
                ..env(false)
            },
        )
        .await
        .unwrap();
        assert!(departed.contains("pasifleştirildi"), "{departed}");
        let subordinate_jobs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE identity_id = $1 AND status = 'queued' AND priority = 1",
        )
        .bind(seed.other_identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            subordinate_jobs, 1,
            "yönetici ayrıldı: asta iş açılır (ADR-041)"
        );
        sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE identity_id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        assert!(departed.contains("rol hesap öngörmüyor"), "{departed}");
        // Ayrilis: katalog gruplari kalkar, pasif OU'ya tasinir (docs/04 planli ayrilis)
        assert!(departed.contains("1 grup çıkarıldı"), "{departed}");
        assert!(departed.contains("OU taşındı"), "{departed}");
        let account = ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            account.dn.to_ascii_lowercase().contains("ou=pasif"),
            "{}",
            account.dn
        );
        assert!(account.member_of.is_empty(), "{:?}", account.member_of);
        assert!(!account.enabled);
        // ADR-033: G gun sonra parola rastgelelestirilir (bir kez); lab'da G = 0
        sqlx::query("UPDATE target_systems SET password_reset_delay_days = 0 WHERE id = $1")
            .bind(seed.ad)
            .execute(&pool)
            .await
            .unwrap();
        let reset = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(reset.contains("parola sıfırlandı"), "{reset}");
        let flagged: bool = sqlx::query_scalar(
            "SELECT password_reset_at_departure FROM account_links WHERE identity_id = $1",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(flagged);
        assert_eq!(
            ad_account::find_by_guid(&mut ldap, &guid)
                .await
                .unwrap()
                .unwrap()
                .pwd_last_set
                .as_deref(),
            Some("0")
        );
        let once = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(!once.contains("parola sıfırlandı"), "{once}");

        // ADR-059 madde 4: bitis kalkinca accountExpires 0 (suresiz) yazilir.
        let expires = ad_account::read_attributes(&mut ldap, &account.dn, &["accountExpires"])
            .await
            .unwrap();
        assert_ne!(
            expires["accountExpires"][0], "0",
            "ayrılan: bitiş FILETIME yazılı"
        );
        // bitis kaldirilinca (geri alma) accountExpires suresiz (0) yazilir; sonra yeniden ayrilis
        sqlx::query("UPDATE identities SET end_at = NULL WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        let reverted = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(
            reverted.contains("accountExpires güncellendi"),
            "{reverted}"
        );
        // geri alma hesabi rol OU'suna geri tasidi: DN GUID'den yeniden okunur
        let dn = ad_account::dn_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .unwrap();
        let expires = ad_account::read_attributes(&mut ldap, &dn, &["accountExpires"])
            .await
            .unwrap();
        assert_eq!(expires["accountExpires"][0], "0");
        sqlx::query("UPDATE identities SET end_at = now() - interval '1 hour' WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        run_job(&pool, &job, &env(false)).await.unwrap();

        // ADR-048/084: kayit iptali — once dogrulama (hic giris yok, pwdLastSet 0), sonra silme;
        // iptal adi yakmaz (ADR-035), son hesap kimligi `silindi` yapar (ADR-038).
        sqlx::query("UPDATE identities SET cancelled = TRUE WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        let verified = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(verified.contains("iptal doğrulandı"), "{verified}");
        let deleted = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(
            deleted.contains("hesap silindi") && deleted.contains("kimlik silindi"),
            "{deleted}"
        );
        assert!(ad_account::find_by_guid(&mut ldap, &guid)
            .await
            .unwrap()
            .is_none());
        let (deleted_at, given, burned): (Option<i64>, String, i64) = sqlx::query_as(
            "SELECT EXTRACT(EPOCH FROM deleted_at)::bigint, given_name, \
             (SELECT COUNT(*) FROM used_names) FROM identities WHERE id = $1",
        )
        .bind(seed.identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            deleted_at.is_some() && given.is_empty(),
            "kişisel veri temizlendi"
        );
        assert_eq!(burned, 0, "kayıt iptali ad yakmaz");

        // ADR-040: hedefte elle silinmis bagli hesap yeniden acilmaz, "kayip hesap"
        // (ikinci kimlikle: hesap ac, elle sil, is yeniden acmaz). Once "hesap acilsin"
        // yeniden acilir (yukarida kapatilmisti).
        for sql in [
            "UPDATE role_target_settings SET provision_account = NULL WHERE target_system_id = $1",
            "UPDATE department_target_settings SET provision_account = NULL WHERE target_system_id = $1",
            "UPDATE target_systems SET provision_account_default = TRUE WHERE id = $1",
        ] {
            sqlx::query(sql).bind(seed.ad).execute(&pool).await.unwrap();
        }
        // yonetici gecisleri asta is acmisti; kapat, sonra temiz bir is ac
        sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE identity_id = $1")
            .bind(seed.other_identity)
            .execute(&pool)
            .await
            .unwrap();
        // Ozel karakterli ad (virgul, tirnak, yildiz, parantez): CN kacislidir, DN ve arama
        // filtresi bozulmaz; kullanici adi normallestirmeden sonra ozel.testx olur (ADR-011).
        sqlx::query(
            "UPDATE identities SET given_name = 'Öz*el', surname = 'Te,st\"(x)' WHERE id = $1",
        )
        .bind(seed.other_identity)
        .execute(&pool)
        .await
        .unwrap();
        test_support::enqueue(&pool, seed.other_identity, seed.ad, 1).await;
        // ilk is hala kirali ("running"); claim siradaki acik isi, yani bunu verir
        let other_job = crate::queue::claim(&pool, "w1", &[])
            .await
            .unwrap()
            .expect("ikinci kimliğin işi kuyrukta olmalı");
        assert_eq!(other_job.identity_id, seed.other_identity);
        let opened = run_job(&pool, &other_job, &env(false)).await.unwrap();
        assert!(opened.starts_with("hesap açıldı"), "{opened}");
        let other_guid: String =
            sqlx::query_scalar("SELECT external_id FROM account_links WHERE identity_id = $1")
                .bind(seed.other_identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        let other_dn = ad_account::dn_by_guid(&mut ldap, &other_guid)
            .await
            .unwrap()
            .unwrap();
        assert!(
            other_dn.starts_with("CN=Öz*el Te\\,st\\\"(x)"),
            "özel karakterli CN kaçışlı yazılır: {other_dn}"
        );
        let other_username: Option<String> =
            sqlx::query_scalar("SELECT username FROM identities WHERE id = $1")
                .bind(seed.other_identity)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(other_username.as_deref(), Some("ozel.testx"));
        ldap.delete(&other_dn).await.unwrap().success().unwrap();
        let lost = run_job(&pool, &other_job, &env(false)).await.unwrap();
        assert!(lost.contains("kayıp hesap"), "{lost}");
        let links: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM account_links WHERE identity_id = $1 AND deleted_by_us_at IS NULL",
        )
        .bind(seed.other_identity)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(links, 1, "bağlantı korunur, yeni hesap açılmaz");
        ldap.unbind().await.ok();
        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
