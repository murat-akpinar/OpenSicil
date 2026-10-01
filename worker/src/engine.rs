// --- START FEATURE: engine ---
// Bir isi calistirir (docs/02 motor): girdiyi yukle → olmasi gereken durumu
// hesapla (saf) → hedefi oku → farki islemlere cevirip tek yazma noktasindan
// uygula. 3a dilimi: AD'de hesap acma (tek add, varsayilan esleme ADR-012,
// gruplar), etkinlestirme/pasiflestirme (yalnizca durum gecisinde, ADR-032),
// hesap baglantisi ve applied_state. Uyelik farki, OU tasima, oznitelik
// guncelleme ve silme sonraki kutucuklarda.

use ldap3::Ldap;
use sqlx::PgPool;

use std::collections::HashMap;

use crate::ad;
use crate::ad_account::{self, AdWriter};
use crate::desired_state::{
    desired_state, effective_manager, AccountPresence, Container, DesiredState, LifecycleState,
};
use crate::mapping;
use crate::model::{self, JobInput, LinkRow};
use crate::queue::ClaimedJob;
use crate::username;
use crate::writes::{self, Applied, Mode, OperationClass, WriteFailure, WriteOp, WriteRequest};

pub struct EngineEnv<'a> {
    pub time_zone: &'a str,
    pub mode: Mode,
    pub aead_key: &'a [u8; crate::crypto::KEY_LEN],
    pub ad_ca_file: Option<&'a str>,
    pub worker_id: &'a str,
    /// ADR-029: hassas kaynak eslemesi; kapaliyken satir mudahaledir
    pub sensitive_mapping_enabled: bool,
}

// ADR-052: hedefe ulasilamamasi deneme tuketmez; nesne duzeyi hata tuketir;
// insan karari gereken durum beklemeden mudahaleye duser (ADR-022).
#[derive(Debug)]
pub enum JobError {
    Unreachable(String),
    Failed(String),
    NeedsIntervention(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Unreachable(reason) => write!(f, "hedefe ulaşılamıyor: {reason}"),
            JobError::Failed(reason) => write!(f, "{reason}"),
            JobError::NeedsIntervention(reason) => write!(f, "müdahale gerekiyor: {reason}"),
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
    match (&c.input.link_row, c.desired.account) {
        (None, AccountPresence::Present { enabled }) => provision(c, ldap, enabled).await,
        (None, presence) => Ok(format!("hesap yok ve açılmayacak: {presence:?}")),
        (Some(link), AccountPresence::Present { enabled }) => {
            reconcile_existing(c, ldap, link, enabled).await
        }
        (Some(_), AccountPresence::Absent) => {
            Ok("hesap silinmeli; silme saklama kutucuğuyla (3c) gelir".to_string())
        }
        (Some(_), AccountPresence::AwaitingDeletionApproval) => {
            Ok("silinmeyi bekliyor (ADR-024)".to_string())
        }
        (Some(_), AccountPresence::NotProvisioned) => {
            Ok("bağlı hesap var; 'hesap açılsın = hayır' onu silmez (ADR-040)".to_string())
        }
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
            return std::env::var("AD_PASSIVE_OU")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| {
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
async fn sources(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    names: Option<&Names>,
) -> Result<mapping::Sources, JobError> {
    let p = &c.input.person;
    let or_person = |fresh: Option<String>, stored: &Option<String>| {
        fresh.or_else(|| stored.clone()).unwrap_or_default()
    };
    let mut values: HashMap<&'static str, String> = HashMap::from([
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
    ]);
    if c.env.sensitive_mapping_enabled {
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
    sqlx::query(
        "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
         VALUES ($1, $2, $3, 'provisioned', 'managed')",
    )
    .bind(c.job.identity_id)
    .bind(c.job.target_system_id)
    .bind(&guid)
    .execute(c.pool)
    .await
    .map_err(|e| JobError::Failed(format!("hesap bağlantısı yazılamadı: {e}")))?;
    let added = add_memberships(c, ldap, &dn).await?;
    if enabled {
        let op = WriteOp::SetEnabled {
            dn: dn.clone(),
            enabled: true,
        };
        apply(c, ldap, op, OperationClass::Attribute).await?;
    }
    set_applied_state(c, state_name(c.desired.state)).await?;
    Ok(format!(
        "hesap açıldı: {dn} (objectGUID {guid}), {added} grup, {}",
        if enabled { "etkin" } else { "pasif" }
    ))
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
    let (applied, note) = reconcile_enabled(c, ldap, link, &account, enabled, transition).await?;
    let attrs = sync_attributes(c, ldap, &account.dn).await?;
    if applied == Applied::DryRun || c.env.mode.dry_run {
        return Ok(format!(
            "kuru çalıştırma, uygulanacaktı: {note}{attrs} ({})",
            account.dn
        ));
    }
    if transition {
        set_applied_state(c, state).await?;
    }
    Ok(format!(
        "{note}{attrs}: {} → applied_state {state}",
        account.dn
    ))
}

async fn reconcile_enabled(
    c: &AdJob<'_>,
    ldap: &mut Ldap,
    link: &LinkRow,
    account: &ad_account::DirectoryAccount,
    enabled: bool,
    transition: bool,
) -> Result<(Applied, &'static str), JobError> {
    let set = |enabled: bool| WriteOp::SetEnabled {
        dn: account.dn.clone(),
        enabled,
    };
    Ok(match (enabled, account.enabled) {
        (true, false) if transition => {
            let class = if link.applied_state.as_deref() == Some("departed") {
                OperationClass::Destructive
            } else {
                OperationClass::Attribute
            };
            (apply(c, ldap, set(true), class).await?, "etkinleştirildi")
        }
        (true, false) => (
            Applied::Applied,
            "hedefte elle pasifleştirilmiş, korunuyor (ADR-032)",
        ),
        (false, true) => (
            apply(c, ldap, set(false), OperationClass::Destructive).await?,
            "pasifleştirildi",
        ),
        _ => (Applied::Applied, "hesap durumu zaten uyumlu"),
    })
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
                    ($1, 'physicalDeliveryOfficeName', 'constant', 'Ankara', TRUE)",
        )
        .bind(seed.ad)
        .execute(&pool)
        .await
        .unwrap();
        let mapped = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(mapped.contains("1 öznitelik güncellendi"), "{mapped}");
        let attrs = ad_account::read_attributes(
            &mut ldap,
            &account.dn,
            &["description", "physicalDeliveryOfficeName", "displayName"],
        )
        .await
        .unwrap();
        assert_eq!(attrs["description"], vec!["Personel"]);
        assert_eq!(
            attrs["physicalDeliveryOfficeName"],
            vec!["Elle"],
            "boşsa yaz: korunur"
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

        // ayrilis: pasiflestirme (yikici niyet), applied_state departed
        sqlx::query("UPDATE identities SET end_at = now() - interval '1 hour' WHERE id = $1")
            .bind(seed.identity)
            .execute(&pool)
            .await
            .unwrap();
        let departed = run_job(&pool, &job, &env(false)).await.unwrap();
        assert!(departed.contains("pasifleştirildi"), "{departed}");
        assert!(
            !ad_account::find_by_guid(&mut ldap, &guid)
                .await
                .unwrap()
                .unwrap()
                .enabled
        );

        ldap.delete(&account.dn).await.unwrap().success().unwrap();
        ldap.unbind().await.ok();
        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
