// --- START FEATURE: engine ---
// Bir is icin olmasi gereken durum fonksiyonunun girdisini DB'den yukler:
// kimlik zaman cizgisi, departman zinciri (yakindan koke), temel/birincil/ek
// roller, hedef varsayilanlari, hesap baglantisi ve saat (Postgres'in saat
// dilimiyle). Kayip katalog ogesi (missing_since dolu) modele girmez: motor onun
// icin islem uretmez (docs/03 katalog).

use sqlx::PgPool;

use crate::desired_state::{
    lifecycle_state, AccountLink, AdditionalRole, Clock, Date, ManagerChain, Model, Origin,
    SingleValued, Source, TargetDefaults, Timeline,
};

pub struct JobInput {
    pub timeline: Timeline,
    pub model: Model,
    pub link: Option<AccountLink>,
    pub clock: Clock,
    pub person: Person,
    pub target_kind: String,
    /// Baglantinin motorun ihtiyac duydugu ham kismi (GUID, uygulanan durum)
    pub link_row: Option<LinkRow>,
    /// Yonetici zinciri (ADR-041); `manager` eslemesinin kaynagi
    pub manager: Option<ManagerChain>,
}

/// Kimlik alanlari: eslemenin kaynaklari (ADR-012) ve ad uretimi girdisi (ADR-011)
#[derive(Debug, Clone)]
pub struct Person {
    pub given_name: String,
    pub surname: String,
    pub employee_number: Option<String>,
    pub department_name: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub upn: Option<String>,
    /// Elle girilen ad (ADR-022); yalnizca username bosken okunur
    pub requested_username: Option<String>,
    /// "Farkli kisi, siradaki adi ver" (ADR-022/042)
    pub name_conflict_override: bool,
    // Esleme kaynaklari (ADR-012/082); hassas olanlar yalnizca ayar acikken cozulur
    pub mobile_phone: Option<String>,
    pub national_id_enc: Option<Vec<u8>>,
    pub start_date: String,
    /// Bitisin son calisma gunu (kurulum diliminde), ISO
    pub end_date: Option<String>,
    pub employment_type: String,
    pub root_department_name: String,
}

#[derive(Debug, Clone)]
pub struct LinkRow {
    pub external_id: String,
    pub applied_state: Option<String>,
    /// ADR-033: ayrilis parolasi bir kez rastgelelestirilir
    pub password_reset_at_departure: bool,
    /// ADR-019 isaret kapaliyken worker'in son yazdigi ilk parolanin pwdLastSet'i
    pub first_password_pwd_last_set: Option<String>,
}

const MAX_DEPARTMENT_DEPTH: i32 = 8;

type IdentityRow = (
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    bool,
    bool,
    Option<i64>,
    i64,
    String,
    i64,
    i64,
);

type SettingsRow = (Option<bool>, Option<i64>, Option<String>, Option<String>);
// (departman id, provision_account, container, email_domain, upn_suffix)
type ChainRow = (
    i64,
    Option<bool>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

pub async fn load(
    pool: &PgPool,
    identity_id: i64,
    target: i64,
    time_zone: &str,
) -> Result<JobInput, String> {
    let (timeline, clock, department_id, primary_role_id) =
        load_identity(pool, identity_id, time_zone).await?;
    let department_chain = load_department_chain(pool, department_id, target).await?;
    let primary_role = load_primary_role(pool, primary_role_id, target).await?;
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM roles WHERE id = $1")
        .bind(primary_role_id)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("birincil rol okunamadı: {e}"))?;
    let base_entitlements = base_entitlements(pool, target).await?;
    let additional_roles = load_additional_roles(pool, identity_id, target).await?;
    let target_defaults = load_target_defaults(pool, target).await?;
    let (link, link_row) = load_link(pool, identity_id, target).await?;
    let mut person = load_person(pool, identity_id, time_zone).await?;
    person.root_department_name = root_department_name(pool, department_id).await?;
    let manager = load_manager_chain(pool, identity_id, time_zone).await?;
    let target_kind: String = sqlx::query_scalar("SELECT kind FROM target_systems WHERE id = $1")
        .bind(target)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("hedef sistem türü okunamadı: {e}"))?;
    Ok(JobInput {
        timeline,
        model: Model {
            base_entitlements,
            department_chain,
            primary_role,
            title,
            additional_roles,
            target: target_defaults,
        },
        link,
        clock,
        person,
        target_kind,
        manager,
        link_row,
    })
}

type PersonRow = (
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    Option<Vec<u8>>,
    String,
    Option<String>,
    String,
);

async fn load_person(pool: &PgPool, identity_id: i64, time_zone: &str) -> Result<Person, String> {
    let row: PersonRow = sqlx::query_as(
        "SELECT i.given_name, i.surname, i.employee_number, d.name, i.username, i.email, i.upn, \
         i.requested_username, i.name_conflict_override, i.mobile_phone, i.national_id_enc, \
         to_char(i.start_date, 'YYYY-MM-DD'), \
         to_char((i.end_at AT TIME ZONE $2) - interval '1 day', 'YYYY-MM-DD'), i.employment_type \
         FROM identities i JOIN departments d ON d.id = i.department_id WHERE i.id = $1",
    )
    .bind(identity_id)
    .bind(time_zone)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("kimlik alanları okunamadı: {e}"))?;
    Ok(Person {
        given_name: row.0,
        surname: row.1,
        employee_number: row.2,
        department_name: row.3,
        username: row.4,
        email: row.5,
        upn: row.6,
        requested_username: row.7,
        name_conflict_override: row.8,
        mobile_phone: row.9,
        national_id_enc: row.10,
        start_date: row.11,
        end_date: row.12,
        employment_type: row.13,
        root_department_name: String::new(),
    })
}

// ADR-017: kok departman adi esleme kaynagi (`company` gibi).
async fn root_department_name(pool: &PgPool, department_id: i64) -> Result<String, String> {
    sqlx::query_scalar(
        "WITH RECURSIVE up AS ( \
           SELECT id, parent_id, name, 0 AS depth FROM departments WHERE id = $1 \
           UNION ALL SELECT d.id, d.parent_id, d.name, up.depth + 1 FROM departments d \
           JOIN up ON d.id = up.parent_id WHERE up.depth < $2) \
         SELECT name FROM up ORDER BY depth DESC LIMIT 1",
    )
    .bind(department_id)
    .bind(MAX_DEPARTMENT_DEPTH)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("kök departman okunamadı: {e}"))
}

// ADR-041: yonetici ve onun kaydindaki devir yoneticisi, turetilen durumlariyla.
async fn load_manager_chain(
    pool: &PgPool,
    identity_id: i64,
    time_zone: &str,
) -> Result<Option<ManagerChain>, String> {
    let row: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT i.manager_id, m.handover_manager_id FROM identities i \
         LEFT JOIN identities m ON m.id = i.manager_id WHERE i.id = $1",
    )
    .bind(identity_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("yönetici okunamadı: {e}"))?;
    let Some(manager) = row.0 else {
        return Ok(None);
    };
    let state_of = |id: i64| async move {
        let (timeline, clock, _, _) = load_identity(pool, id, time_zone).await?;
        Ok::<_, String>(lifecycle_state(&timeline, &clock))
    };
    let manager_state = state_of(manager).await?;
    let handover = match row.1 {
        Some(h) => Some((h, state_of(h).await?)),
        None => None,
    };
    Ok(Some(ManagerChain {
        manager,
        manager_state,
        handover,
    }))
}

fn date(text: &str) -> Result<Date, String> {
    Date::from_iso(text).ok_or_else(|| format!("tarih çözümlenemedi: {text}"))
}

async fn load_identity(
    pool: &PgPool,
    identity_id: i64,
    time_zone: &str,
) -> Result<(Timeline, Clock, i64, i64), String> {
    let row: IdentityRow = sqlx::query_as(
        "SELECT to_char(start_date, 'YYYY-MM-DD'), EXTRACT(EPOCH FROM end_at)::bigint, \
         to_char(suspension_start, 'YYYY-MM-DD'), to_char(suspension_end, 'YYYY-MM-DD'), \
         cancelled, emergency_departure, EXTRACT(EPOCH FROM deleted_at)::bigint, \
         EXTRACT(EPOCH FROM now())::bigint, to_char((now() AT TIME ZONE $2)::date, 'YYYY-MM-DD'), \
         department_id, primary_role_id FROM identities WHERE id = $1",
    )
    .bind(identity_id)
    .bind(time_zone)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("kimlik okunamadı: {e}"))?;
    let timeline = Timeline {
        start_date: date(&row.0)?,
        end_at: row.1,
        suspension_start: row.2.as_deref().map(date).transpose()?,
        suspension_end: row.3.as_deref().map(date).transpose()?,
        cancelled: row.4,
        emergency_departure: row.5,
        deleted_at: row.6,
    };
    let clock = Clock {
        now: row.7,
        today: date(&row.8)?,
    };
    Ok((timeline, clock, row.9, row.10))
}

fn single_valued(row: Option<SettingsRow>) -> SingleValued {
    match row {
        Some((provision_account, container, email_domain, upn_suffix)) => SingleValued {
            provision_account,
            container,
            email_domain,
            upn_suffix,
        },
        None => SingleValued::default(),
    }
}

// Kimligin departmani (depth 0) once, kok sonda; dongu/derinlik siniri 8 (ADR-017).
async fn load_department_chain(
    pool: &PgPool,
    department_id: i64,
    target: i64,
) -> Result<Vec<Source>, String> {
    let rows: Vec<ChainRow> = sqlx::query_as(
        "WITH RECURSIVE chain AS ( \
                SELECT id, parent_id, 0 AS depth FROM departments WHERE id = $1 \
                UNION ALL \
                SELECT d.id, d.parent_id, c.depth + 1 FROM departments d \
                JOIN chain c ON d.id = c.parent_id WHERE c.depth < $3) \
             SELECT c.id, s.provision_account, s.container_item_id, s.email_domain, s.upn_suffix \
             FROM chain c LEFT JOIN department_target_settings s \
               ON s.department_id = c.id AND s.target_system_id = $2 \
             ORDER BY c.depth",
    )
    .bind(department_id)
    .bind(target)
    .bind(MAX_DEPARTMENT_DEPTH)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("departman zinciri okunamadı: {e}"))?;
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let entitlements: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT e.department_id, e.catalog_item_id FROM department_entitlements e \
         JOIN catalog_items c ON c.id = e.catalog_item_id \
         WHERE e.department_id = ANY($1) AND c.target_system_id = $2 AND c.missing_since IS NULL",
    )
    .bind(&ids)
    .bind(target)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("departman yetkileri okunamadı: {e}"))?;
    Ok(rows
        .into_iter()
        .map(
            |(id, provision, container, email_domain, upn_suffix)| Source {
                entitlements: entitlements
                    .iter()
                    .filter(|(owner, _)| *owner == id)
                    .map(|(_, item)| *item)
                    .collect(),
                settings: single_valued(Some((provision, container, email_domain, upn_suffix))),
            },
        )
        .collect())
}

const ROLE_ENTITLEMENTS_SQL: &str = "SELECT e.role_id, e.catalog_item_id FROM role_entitlements e \
     JOIN catalog_items c ON c.id = e.catalog_item_id \
     WHERE e.role_id = ANY($1) AND c.target_system_id = $2 AND c.missing_since IS NULL";

async fn role_entitlements(
    pool: &PgPool,
    role_ids: &[i64],
    target: i64,
) -> Result<Vec<(i64, i64)>, String> {
    sqlx::query_as(ROLE_ENTITLEMENTS_SQL)
        .bind(role_ids)
        .bind(target)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("rol yetkileri okunamadı: {e}"))
}

async fn load_primary_role(pool: &PgPool, role_id: i64, target: i64) -> Result<Source, String> {
    let settings: Option<SettingsRow> = sqlx::query_as(
        "SELECT provision_account, container_item_id, email_domain, upn_suffix \
         FROM role_target_settings WHERE role_id = $1 AND target_system_id = $2",
    )
    .bind(role_id)
    .bind(target)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("birincil rol ayarı okunamadı: {e}"))?;
    let entitlements = role_entitlements(pool, &[role_id], target).await?;
    Ok(Source {
        entitlements: entitlements.into_iter().map(|(_, item)| item).collect(),
        settings: single_valued(settings),
    })
}

async fn base_entitlements(pool: &PgPool, target: i64) -> Result<Vec<i64>, String> {
    let base_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM roles WHERE kind = 'base'")
        .fetch_all(pool)
        .await
        .map_err(|e| format!("temel rol okunamadı: {e}"))?;
    Ok(role_entitlements(pool, &base_ids, target)
        .await?
        .into_iter()
        .map(|(_, item)| item)
        .collect())
}

async fn load_additional_roles(
    pool: &PgPool,
    identity_id: i64,
    target: i64,
) -> Result<Vec<AdditionalRole>, String> {
    let assignments: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT role_id, to_char(ends_on, 'YYYY-MM-DD') FROM identity_additional_roles \
         WHERE identity_id = $1",
    )
    .bind(identity_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("ek roller okunamadı: {e}"))?;
    let role_ids: Vec<i64> = assignments.iter().map(|(id, _)| *id).collect();
    let entitlements = role_entitlements(pool, &role_ids, target).await?;
    assignments
        .into_iter()
        .map(|(role_id, ends_on)| {
            Ok(AdditionalRole {
                entitlements: entitlements
                    .iter()
                    .filter(|(owner, _)| *owner == role_id)
                    .map(|(_, item)| *item)
                    .collect(),
                ends_on: ends_on.as_deref().map(date).transpose()?,
            })
        })
        .collect()
}

async fn load_target_defaults(pool: &PgPool, target: i64) -> Result<TargetDefaults, String> {
    let row: (bool, Option<i64>, i32, bool, i32) = sqlx::query_as(
        "SELECT provision_account_default, default_container_item_id, retention_days, \
         delete_requires_approval, password_reset_delay_days FROM target_systems WHERE id = $1",
    )
    .bind(target)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("hedef sistem okunamadı: {e}"))?;
    Ok(TargetDefaults {
        provision_account: row.0,
        container: row.1,
        retention_days: row.2.max(0) as u32,
        delete_requires_approval: row.3,
        password_reset_delay_days: row.4.max(0) as u32,
    })
}

type LinkQueryRow = (
    String,
    Option<bool>,
    bool,
    String,
    Option<String>,
    bool,
    Option<String>,
);

async fn load_link(
    pool: &PgPool,
    identity_id: i64,
    target: i64,
) -> Result<(Option<AccountLink>, Option<LinkRow>), String> {
    let row: Option<LinkQueryRow> = sqlx::query_as(
        "SELECT origin, verified_unused, deletion_approved, external_id, applied_state, \
         password_reset_at_departure, first_password_pwd_last_set \
         FROM account_links WHERE identity_id = $1 AND target_system_id = $2",
    )
    .bind(identity_id)
    .bind(target)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("hesap bağlantısı okunamadı: {e}"))?;
    let Some((origin, verified_unused, deletion_approved, external_id, applied_state, reset, pwd)) =
        row
    else {
        return Ok((None, None));
    };
    let link_row = LinkRow {
        external_id,
        applied_state,
        password_reset_at_departure: reset,
        first_password_pwd_last_set: pwd,
    };
    Ok((
        Some(AccountLink {
            origin: if origin == "adopted" {
                Origin::Adopted
            } else {
                Origin::Provisioned
            },
            verified_unused,
            deletion_approved,
        }),
        Some(link_row),
    ))
}
// --- END FEATURE: engine ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn loads_docs03_example_with_chain_roles_and_defaults() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;

        let input = load(&pool, seed.identity, seed.ad, "Europe/Istanbul")
            .await
            .unwrap();
        assert_eq!(input.model.base_entitlements, vec![seed.gg_internet]);
        let chain: Vec<Vec<i64>> = input
            .model
            .department_chain
            .iter()
            .map(|s| s.entitlements.clone())
            .collect();
        assert_eq!(
            chain,
            vec![vec![seed.gg_bt_paylasim], vec![seed.gg_ankara_yazici]],
            "yakından köke"
        );
        assert_eq!(
            input.model.primary_role.entitlements,
            vec![seed.gg_sistem_uzmanlari]
        );
        assert_eq!(
            input.model.primary_role.settings.container,
            Some(seed.sistem_uzmanlari_ou)
        );
        assert_eq!(
            input.model.primary_role.settings.upn_suffix.as_deref(),
            Some("example.local")
        );
        assert_eq!(input.model.title.as_deref(), Some("Sistem Uzmanı"));
        assert_eq!(input.model.additional_roles.len(), 1);
        assert_eq!(
            input.model.additional_roles[0].entitlements,
            vec![seed.gg_nobet]
        );
        assert!(input.model.additional_roles[0].ends_on.is_some());
        assert_eq!(input.model.target.retention_days, 90);
        assert!(input.link.is_none());
        assert!(input.clock.now > 0);

        // kayip oge modele girmez
        sqlx::query("UPDATE catalog_items SET missing_since = now() WHERE id = $1")
            .bind(seed.gg_nobet)
            .execute(&pool)
            .await
            .unwrap();
        let input = load(&pool, seed.identity, seed.ad, "Europe/Istanbul")
            .await
            .unwrap();
        assert!(input.model.additional_roles[0].entitlements.is_empty());

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
