// --- START FEATURE: role-department-screens ---
// Rol ve departman tanimlari, katalogdan secim, hedef sistem varsayilanlari
// (docs/03 rol modeli; ADR-007/017/020/080). Sahneleme (ADR-031) 3f'te: kayit
// dogrudan modele yazilir, etkilenen kimlikler icin toplu oncelikli is acilir.

use sqlx::{PgPool, Postgres, Transaction};

use crate::jobs::Priority;

// ADR-017: agac derinligi en fazla 8. err.depth_exceeded metni bu sayiyi icerir.
pub const MAX_DEPTH: i64 = 8;
// Ekran karsiligi i18n'de: rolekind.<anahtar> (ADR-089)
pub const ROLE_KINDS: [&str; 3] = ["base", "primary", "additional"];

// Invalid: hata metni degil i18n anahtari (ADR-089); ceviri web katmaninda.
#[derive(Debug)]
pub enum SaveError {
    Invalid(&'static str),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for SaveError {
    fn from(e: sqlx::Error) -> Self {
        match e.as_database_error() {
            Some(db) if db.is_unique_violation() => SaveError::Invalid("err.name_or_code_taken"),
            _ => SaveError::Db(e),
        }
    }
}

pub struct RoleRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub title: String,
    pub entitlements: i64,
}

pub async fn list_roles(pool: &PgPool) -> Result<Vec<RoleRow>, sqlx::Error> {
    let rows: Vec<(i64, String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT r.id, r.kind, r.name, r.title, \
         (SELECT COUNT(*) FROM role_entitlements e WHERE e.role_id = r.id) \
         FROM roles r ORDER BY r.kind, r.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind, name, title, entitlements)| RoleRow {
            id,
            kind,
            name,
            title: title.unwrap_or_default(),
            entitlements,
        })
        .collect())
}

// ADR-007: temel rol tek (kismi tekil indeks), unvan yalnizca birincilde (CHECK).
pub async fn create_role(
    pool: &PgPool,
    kind: &str,
    name: &str,
    title: &str,
) -> Result<i64, SaveError> {
    if !ROLE_KINDS.contains(&kind) {
        return Err(SaveError::Invalid("err.role_kind_required"));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(SaveError::Invalid("err.role_name_blank"));
    }
    if kind == "base" && base_role_exists(pool).await? {
        return Err(SaveError::Invalid("err.base_role_exists"));
    }
    let title = (kind == "primary" && !title.trim().is_empty()).then(|| title.trim().to_string());
    Ok(
        sqlx::query_scalar(
            "INSERT INTO roles (kind, name, title) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(kind)
        .bind(name)
        .bind(title)
        .fetch_one(pool)
        .await?,
    )
}

async fn base_role_exists(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM roles WHERE kind = 'base')")
        .fetch_one(pool)
        .await
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TargetSetting {
    pub target_id: i64,
    pub target_name: String,
    pub provision_account: Option<bool>,
    pub container_item_id: Option<i64>,
    pub email_domain: String,
    pub upn_suffix: String,
}

pub struct Definition {
    pub id: i64,
    pub name: String,
    pub entitlement_ids: Vec<i64>,
    pub settings: Vec<TargetSetting>,
}

pub struct RoleDetail {
    pub def: Definition,
    pub kind: String,
    pub title: String,
}

pub struct DepartmentDetail {
    pub def: Definition,
    pub code: String,
    pub parent_id: Option<i64>,
}

// Hangi tablonun sahibi: rol ya da departman; SQL metinleri sabit (parametreli).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Role,
    Department,
}

impl Owner {
    fn select_entitlements(self) -> &'static str {
        match self {
            Owner::Role => "SELECT catalog_item_id FROM role_entitlements WHERE role_id = $1 ORDER BY 1",
            Owner::Department => {
                "SELECT catalog_item_id FROM department_entitlements WHERE department_id = $1 ORDER BY 1"
            }
        }
    }
    fn delete_entitlements(self) -> &'static str {
        match self {
            Owner::Role => "DELETE FROM role_entitlements WHERE role_id = $1",
            Owner::Department => "DELETE FROM department_entitlements WHERE department_id = $1",
        }
    }
    fn insert_entitlement(self) -> &'static str {
        match self {
            Owner::Role => "INSERT INTO role_entitlements (role_id, catalog_item_id) VALUES ($1, $2)",
            Owner::Department => {
                "INSERT INTO department_entitlements (department_id, catalog_item_id) VALUES ($1, $2)"
            }
        }
    }
    fn select_settings(self) -> &'static str {
        match self {
            Owner::Role => {
                "SELECT t.id, t.name, s.provision_account, s.container_item_id, s.email_domain, s.upn_suffix \
                 FROM target_systems t LEFT JOIN role_target_settings s \
                 ON s.target_system_id = t.id AND s.role_id = $1 ORDER BY t.id"
            }
            Owner::Department => {
                "SELECT t.id, t.name, s.provision_account, s.container_item_id, s.email_domain, s.upn_suffix \
                 FROM target_systems t LEFT JOIN department_target_settings s \
                 ON s.target_system_id = t.id AND s.department_id = $1 ORDER BY t.id"
            }
        }
    }
    fn upsert_setting(self) -> &'static str {
        match self {
            Owner::Role => {
                "INSERT INTO role_target_settings \
                 (role_id, target_system_id, provision_account, container_item_id, email_domain, upn_suffix) \
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (role_id, target_system_id) DO UPDATE SET \
                 provision_account = EXCLUDED.provision_account, container_item_id = EXCLUDED.container_item_id, \
                 email_domain = EXCLUDED.email_domain, upn_suffix = EXCLUDED.upn_suffix"
            }
            Owner::Department => {
                "INSERT INTO department_target_settings \
                 (department_id, target_system_id, provision_account, container_item_id, email_domain, upn_suffix) \
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (department_id, target_system_id) DO UPDATE SET \
                 provision_account = EXCLUDED.provision_account, container_item_id = EXCLUDED.container_item_id, \
                 email_domain = EXCLUDED.email_domain, upn_suffix = EXCLUDED.upn_suffix"
            }
        }
    }
}

type SettingRow = (
    i64,
    String,
    Option<bool>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

async fn load_definition(
    pool: &PgPool,
    owner: Owner,
    id: i64,
    name: String,
) -> Result<Definition, sqlx::Error> {
    let entitlement_ids: Vec<i64> = sqlx::query_scalar(owner.select_entitlements())
        .bind(id)
        .fetch_all(pool)
        .await?;
    let rows: Vec<SettingRow> = sqlx::query_as(owner.select_settings())
        .bind(id)
        .fetch_all(pool)
        .await?;
    let settings = rows
        .into_iter()
        .map(|(target_id, target_name, pa, ct, ed, us)| TargetSetting {
            target_id,
            target_name,
            provision_account: pa,
            container_item_id: ct,
            email_domain: ed.unwrap_or_default(),
            upn_suffix: us.unwrap_or_default(),
        })
        .collect();
    Ok(Definition {
        id,
        name,
        entitlement_ids,
        settings,
    })
}

pub async fn load_role(pool: &PgPool, id: i64) -> Result<Option<RoleDetail>, sqlx::Error> {
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT kind, name, title FROM roles WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((kind, name, title)) = row else {
        return Ok(None);
    };
    Ok(Some(RoleDetail {
        def: load_definition(pool, Owner::Role, id, name).await?,
        kind,
        title: title.unwrap_or_default(),
    }))
}

pub async fn load_department(
    pool: &PgPool,
    id: i64,
) -> Result<Option<DepartmentDetail>, sqlx::Error> {
    let row: Option<(String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT name, code, parent_id FROM departments WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((name, code, parent_id)) = row else {
        return Ok(None);
    };
    Ok(Some(DepartmentDetail {
        def: load_definition(pool, Owner::Department, id, name).await?,
        code: code.unwrap_or_default(),
        parent_id,
    }))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DefinitionEdit {
    pub name: String,
    pub entitlement_ids: Vec<i64>,
    pub settings: Vec<TargetSetting>,
}

// Yetki ogeleri kume olarak degistirilir, ayarlar hedef basina upsert; tek transaction.
async fn save_definition(
    tx: &mut Transaction<'_, Postgres>,
    owner: Owner,
    id: i64,
    edit: &DefinitionEdit,
    with_settings: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(owner.delete_entitlements())
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for item in &edit.entitlement_ids {
        sqlx::query(owner.insert_entitlement())
            .bind(id)
            .bind(item)
            .execute(&mut **tx)
            .await?;
    }
    if !with_settings {
        return Ok(());
    }
    for s in &edit.settings {
        sqlx::query(owner.upsert_setting())
            .bind(id)
            .bind(s.target_id)
            .bind(s.provision_account)
            .bind(s.container_item_id)
            .bind(blank_to_null(&s.email_domain))
            .bind(blank_to_null(&s.upn_suffix))
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

fn blank_to_null(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn required_name(name: &str, key: &'static str) -> Result<String, SaveError> {
    blank_to_null(name).ok_or(SaveError::Invalid(key))
}

/// Kaydetmeden once de, taslaga almadan once de ayni dogrulamalar (ADR-031):
/// onaylanacak taslak gecerli olmali. Doner: kirpilmis ad.
pub async fn validate_definition(
    pool: &PgPool,
    owner: Owner,
    id: i64,
    edit: &DefinitionEdit,
    parent_id: Option<i64>,
) -> Result<String, SaveError> {
    let name = match owner {
        Owner::Role => required_name(&edit.name, "err.role_name_blank")?,
        Owner::Department => required_name(&edit.name, "err.department_name_blank")?,
    };
    if owner == Owner::Department {
        validate_parent(pool, id, parent_id).await?;
    }
    Ok(name)
}

// ADR-007: tek degerli ayar yalnizca birincil rolde; digerlerinde gelen ayar yok sayilir.
pub async fn save_role(
    pool: &PgPool,
    id: i64,
    title: &str,
    edit: &DefinitionEdit,
) -> Result<(), SaveError> {
    let name = validate_definition(pool, Owner::Role, id, edit, None).await?;
    let kind: String = sqlx::query_scalar("SELECT kind FROM roles WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    let primary = kind == "primary";
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE roles SET name = $2, title = $3 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(primary.then(|| blank_to_null(title)).flatten())
        .execute(&mut *tx)
        .await?;
    save_definition(&mut tx, Owner::Role, id, edit, primary).await?;
    tx.commit().await?;
    Ok(())
}

pub struct DepartmentRow {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub indent: String,
    pub entitlements: i64,
}

// Agac, kokten yapraga; ayni seviyede ada gore (recursive CTE, ADR-017).
pub async fn list_departments(pool: &PgPool) -> Result<Vec<DepartmentRow>, sqlx::Error> {
    type Row = (i64, String, Option<String>, i64, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "WITH RECURSIVE tree AS ( \
           SELECT id, name, code, 1::bigint AS depth, ARRAY[lower(name)] AS path FROM departments \
           WHERE parent_id IS NULL \
           UNION ALL \
           SELECT d.id, d.name, d.code, t.depth + 1, t.path || lower(d.name) \
           FROM departments d JOIN tree t ON d.parent_id = t.id WHERE t.depth < $1) \
         SELECT id, name, code, depth, \
           (SELECT COUNT(*) FROM department_entitlements e WHERE e.department_id = tree.id) \
         FROM tree ORDER BY path",
    )
    .bind(MAX_DEPTH)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, code, depth, entitlements)| DepartmentRow {
            id,
            name,
            code: code.unwrap_or_default(),
            indent: "— ".repeat((depth - 1) as usize),
            entitlements,
        })
        .collect())
}

pub async fn create_department(
    pool: &PgPool,
    name: &str,
    code: &str,
    parent_id: Option<i64>,
) -> Result<i64, SaveError> {
    let name = required_name(name, "err.department_name_blank")?;
    if let Some(parent) = parent_id {
        if ancestor_depth(pool, parent).await? >= MAX_DEPTH {
            return Err(SaveError::Invalid("err.depth_exceeded"));
        }
    }
    Ok(sqlx::query_scalar(
        "INSERT INTO departments (name, code, parent_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(blank_to_null(code))
    .bind(parent_id)
    .fetch_one(pool)
    .await?)
}

pub async fn save_department(
    pool: &PgPool,
    id: i64,
    code: &str,
    parent_id: Option<i64>,
    edit: &DefinitionEdit,
) -> Result<(), SaveError> {
    let name = validate_definition(pool, Owner::Department, id, edit, parent_id).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE departments SET name = $2, code = $3, parent_id = $4 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(blank_to_null(code))
        .bind(parent_id)
        .execute(&mut *tx)
        .await?;
    save_definition(&mut tx, Owner::Department, id, edit, true).await?;
    tx.commit().await?;
    Ok(())
}

// Ust zincirin uzunlugu (kok dahil); 0 = yok.
async fn ancestor_depth(pool: &PgPool, id: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "WITH RECURSIVE up AS ( \
           SELECT id, parent_id, 1::bigint AS depth FROM departments WHERE id = $1 \
           UNION ALL SELECT d.id, d.parent_id, up.depth + 1 FROM departments d \
           JOIN up ON d.id = up.parent_id WHERE up.depth < $2) \
         SELECT COALESCE(MAX(depth), 0) FROM up",
    )
    .bind(id)
    .bind(MAX_DEPTH + 1)
    .fetch_one(pool)
    .await
}

// Kendi alt agacinin yuksekligi (kendisi dahil) ve alt agacta verilen id var mi.
async fn subtree(pool: &PgPool, id: i64, probe: Option<i64>) -> Result<(i64, bool), sqlx::Error> {
    sqlx::query_as(
        "WITH RECURSIVE down AS ( \
           SELECT id, 1::bigint AS depth FROM departments WHERE id = $1 \
           UNION ALL SELECT d.id, down.depth + 1 FROM departments d \
           JOIN down ON d.parent_id = down.id WHERE down.depth < $3) \
         SELECT COALESCE(MAX(depth), 0), bool_or(id = $2) FROM down",
    )
    .bind(id)
    .bind(probe)
    .bind(MAX_DEPTH + 1)
    .fetch_one(pool)
    .await
    .map(|(height, found): (i64, Option<bool>)| (height, found.unwrap_or(false)))
}

// ADR-017: dongu reddedilir, yeni konumda derinlik <= 8.
async fn validate_parent(pool: &PgPool, id: i64, parent_id: Option<i64>) -> Result<(), SaveError> {
    let Some(parent) = parent_id else {
        return Ok(());
    };
    let (height, cycle) = subtree(pool, id, Some(parent)).await?;
    if cycle {
        return Err(SaveError::Invalid("err.parent_cycle"));
    }
    if ancestor_depth(pool, parent).await? + height > MAX_DEPTH {
        return Err(SaveError::Invalid("err.depth_exceeded"));
    }
    Ok(())
}

// Tanim degisince etkilenen kimlikler: rol → birincil/ek rolu o olanlar, temel rol →
// herkes; departman → alt agac. Etki onizlemesi (change_set) ayni kumeyi okur.
pub async fn affected_identities(
    pool: &PgPool,
    owner: Owner,
    id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let sql = match owner {
        Owner::Role => {
            "SELECT DISTINCT i.id FROM identities i \
             LEFT JOIN identity_additional_roles a ON a.identity_id = i.id \
             WHERE i.deleted_at IS NULL AND (i.primary_role_id = $1 OR a.role_id = $1 \
               OR EXISTS (SELECT 1 FROM roles r WHERE r.id = $1 AND r.kind = 'base'))"
        }
        Owner::Department => {
            "WITH RECURSIVE down AS ( \
               SELECT id FROM departments WHERE id = $1 \
               UNION ALL SELECT d.id FROM departments d JOIN down ON d.parent_id = down.id) \
             SELECT i.id FROM identities i WHERE i.deleted_at IS NULL \
               AND i.department_id IN (SELECT id FROM down)"
        }
    };
    sqlx::query_scalar(sql).bind(id).fetch_all(pool).await
}

// ponytail: kimlik basina hedef sayisi kadar INSERT; taslak/onay yolu sonraki kutucukta.
pub async fn enqueue_affected(pool: &PgPool, owner: Owner, id: i64) -> Result<usize, sqlx::Error> {
    let ids = affected_identities(pool, owner, id).await?;
    for identity in &ids {
        crate::identity::enqueue_all_targets(pool, *identity, Priority::Bulk).await?;
    }
    Ok(ids.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogChoice {
    pub id: i64,
    pub target_id: i64,
    pub kind: String,
    pub display_name: String,
    pub location: String,
    pub missing: bool,
}

pub struct CatalogOptions {
    pub memberships: Vec<CatalogChoice>,
    pub containers: Vec<CatalogChoice>,
}

// Kayip oge listede kalir ve isaretlenir (docs/03 katalog: rolde elle degistirilir).
pub async fn catalog_options(pool: &PgPool) -> Result<CatalogOptions, sqlx::Error> {
    type Row = (i64, i64, String, String, Option<String>, bool, bool);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, target_system_id, kind, display_name, location, missing_since IS NOT NULL, \
         is_membership FROM catalog_items ORDER BY target_system_id, kind, display_name",
    )
    .fetch_all(pool)
    .await?;
    let mut options = CatalogOptions {
        memberships: Vec::new(),
        containers: Vec::new(),
    };
    for (id, target_id, kind, display_name, location, missing, is_membership) in rows {
        let choice = CatalogChoice {
            id,
            target_id,
            kind,
            display_name,
            location: location.unwrap_or_default(),
            missing,
        };
        if is_membership {
            options.memberships.push(choice);
        } else {
            options.containers.push(choice);
        }
    }
    Ok(options)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub provision_account_default: bool,
    pub default_container_item_id: Option<i64>,
    pub retention_days: i32,
    pub delete_requires_approval: bool,
    pub password_reset_delay_days: i32,
}

pub async fn list_targets(pool: &PgPool) -> Result<Vec<TargetRow>, sqlx::Error> {
    type Row = (i64, String, String, bool, Option<i64>, i32, bool, i32);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, kind, name, provision_account_default, default_container_item_id, \
         retention_days, delete_requires_approval, password_reset_delay_days \
         FROM target_systems ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind, name, pa, ct, rd, dra, prd)| TargetRow {
            id,
            kind,
            name,
            provision_account_default: pa,
            default_container_item_id: ct,
            retention_days: rd,
            delete_requires_approval: dra,
            password_reset_delay_days: prd,
        })
        .collect())
}

/// Okuma seridi (ADR-051): katalog yenileme istegi; acik is varsa yenisi acilmaz.
pub async fn request_catalog_refresh(
    pool: &PgPool,
    target: i64,
    by: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO read_jobs (kind, target_system_id, requested_by) \
         VALUES ('catalog_refresh', $1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(target)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Hedef basina son katalog yenileme isi: durum anahtari + zaman + sonuc.
pub async fn last_catalog_refresh(
    pool: &PgPool,
    time_zone: &str,
) -> Result<Vec<(i64, String, String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT ON (target_system_id) target_system_id, status, \
         to_char(COALESCE(finished_at, started_at, created_at) AT TIME ZONE $1, 'YYYY-MM-DD HH24:MI'), \
         COALESCE(result, '') FROM read_jobs WHERE kind = 'catalog_refresh' \
         ORDER BY target_system_id, created_at DESC",
    )
    .bind(time_zone)
    .fetch_all(pool)
    .await
}

// ADR-024: saklama ve silme onayi hedef basina; konteyner ayni hedefin OU/COS'u (FK).
pub async fn save_target(pool: &PgPool, t: &TargetRow) -> Result<(), SaveError> {
    if t.retention_days < 0 || t.password_reset_delay_days < 0 {
        return Err(SaveError::Invalid("err.negative_days"));
    }
    sqlx::query(
        "UPDATE target_systems SET provision_account_default = $2, default_container_item_id = $3, \
         retention_days = $4, delete_requires_approval = $5, password_reset_delay_days = $6 \
         WHERE id = $1",
    )
    .bind(t.id)
    .bind(t.provision_account_default)
    .bind(t.default_container_item_id)
    .bind(t.retention_days)
    .bind(t.delete_requires_approval)
    .bind(t.password_reset_delay_days)
    .execute(pool)
    .await
    .map_err(|e| match e.as_database_error() {
        Some(db) if db.is_foreign_key_violation() => {
            SaveError::Invalid("err.container_wrong_target")
        }
        _ => SaveError::Db(e),
    })?;
    Ok(())
}
// --- END FEATURE: role-department-screens ---

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(name: &str, items: Vec<i64>, settings: Vec<TargetSetting>) -> DefinitionEdit {
        DefinitionEdit {
            name: name.to_string(),
            entitlement_ids: items,
            settings,
        }
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn roles_departments_and_targets_round_trip_with_validation() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;

        // Rol: temel tek, unvan yalnizca birincilde, ayni ad reddedilir.
        let base = create_role(&pool, "base", "Herkes", "").await.unwrap();
        assert!(matches!(
            create_role(&pool, "base", "Herkes2", "").await,
            Err(SaveError::Invalid(_))
        ));
        assert!(matches!(
            create_role(&pool, "primary", "Herkes", "").await,
            Err(SaveError::Invalid(_))
        ));
        let primary = create_role(&pool, "primary", "Sistem Uzmanı", "Sistem Uzmanı")
            .await
            .unwrap();
        let settings = vec![TargetSetting {
            target_id: catalog.ad,
            target_name: String::new(),
            provision_account: Some(true),
            container_item_id: Some(catalog.sistem_uzmanlari_ou),
            email_domain: " example.com ".to_string(),
            upn_suffix: String::new(),
        }];
        save_role(
            &pool,
            primary,
            "Uzman",
            &edit(
                "Sistem Uzmanı",
                vec![catalog.gg_vpn, catalog.gg_sistem_uzmanlari],
                settings.clone(),
            ),
        )
        .await
        .unwrap();
        let detail = load_role(&pool, primary).await.unwrap().unwrap();
        assert_eq!(detail.title, "Uzman");
        assert_eq!(
            detail.def.entitlement_ids,
            vec![
                catalog.gg_sistem_uzmanlari.min(catalog.gg_vpn),
                catalog.gg_sistem_uzmanlari.max(catalog.gg_vpn)
            ]
        );
        let ad_setting = detail
            .def
            .settings
            .iter()
            .find(|s| s.target_id == catalog.ad)
            .unwrap();
        assert_eq!(ad_setting.email_domain, "example.com");
        assert_eq!(
            ad_setting.container_item_id,
            Some(catalog.sistem_uzmanlari_ou)
        );
        // Temel rolde ayar yazilmaz (ADR-007).
        save_role(
            &pool,
            base,
            "",
            &edit("Herkes", vec![catalog.gg_internet], settings),
        )
        .await
        .unwrap();
        let base_detail = load_role(&pool, base).await.unwrap().unwrap();
        assert!(base_detail
            .def
            .settings
            .iter()
            .all(|s| s.container_item_id.is_none()));
        assert!(matches!(
            save_role(&pool, base, "", &edit("  ", vec![], vec![])).await,
            Err(SaveError::Invalid(_))
        ));
        // Konteyner uyelik ogesi olamaz (bilesik FK).
        assert!(matches!(
            save_role(
                &pool,
                primary,
                "",
                &edit("Sistem Uzmanı", vec![catalog.personel_ou], vec![])
            )
            .await,
            Err(SaveError::Db(_))
        ));
        assert_eq!(list_roles(&pool).await.unwrap().len(), 3);

        // Departman agaci: dongu ve derinlik.
        let root = create_department(&pool, "Ankara", "ANK", None)
            .await
            .unwrap();
        let child = create_department(&pool, "Bilgi İşlem", "", Some(root))
            .await
            .unwrap();
        assert!(
            matches!(
                save_department(
                    &pool,
                    root,
                    "ANK",
                    Some(child),
                    &edit("Ankara", vec![], vec![])
                )
                .await,
                Err(SaveError::Invalid(_))
            ),
            "döngü reddedilir"
        );
        let mut leaf = child;
        for i in 0..6 {
            leaf = create_department(&pool, &format!("Seviye {i}"), "", Some(leaf))
                .await
                .unwrap();
        }
        assert!(
            matches!(
                create_department(&pool, "Dokuzuncu", "", Some(leaf)).await,
                Err(SaveError::Invalid(_))
            ),
            "9. seviye reddedilir"
        );
        save_department(
            &pool,
            child,
            "BT",
            Some(root),
            &edit("Bilgi İşlem", vec![catalog.gg_bt_paylasim], vec![]),
        )
        .await
        .unwrap();
        let tree = list_departments(&pool).await.unwrap();
        assert_eq!(tree[0].name, "Ankara");
        assert_eq!(tree[1].indent, "— ");
        assert_eq!(tree[1].code, "BT");
        assert_eq!(
            load_department(&pool, child)
                .await
                .unwrap()
                .unwrap()
                .def
                .entitlement_ids,
            vec![catalog.gg_bt_paylasim]
        );

        // Etkilenen kimlikler: temel rol herkes, departman alt agac.
        assert_eq!(enqueue_affected(&pool, Owner::Role, base).await.unwrap(), 2);
        sqlx::query("UPDATE identities SET department_id = $1 WHERE id = $2")
            .bind(child)
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            enqueue_affected(&pool, Owner::Department, root)
                .await
                .unwrap(),
            1
        );
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE priority = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 4, "iki kimlik x iki hedef, acik is tekrar acilmaz");

        // Hedef varsayilanlari.
        let mut target = list_targets(&pool).await.unwrap().remove(0);
        target.retention_days = 30;
        target.default_container_item_id = Some(catalog.cos_default);
        assert!(
            matches!(
                save_target(&pool, &target).await,
                Err(SaveError::Invalid(_))
            ),
            "başka hedefin COS'u"
        );
        target.default_container_item_id = Some(catalog.personel_ou);
        save_target(&pool, &target).await.unwrap();
        assert_eq!(list_targets(&pool).await.unwrap()[0].retention_days, 30);
        let options = catalog_options(&pool).await.unwrap();
        assert_eq!(options.containers.len(), 4);
        assert_eq!(options.memberships.len(), 8);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
