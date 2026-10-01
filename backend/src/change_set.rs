// --- START FEATURE: change-set ---
// Etki onizlemesi ve degisiklik seti esigi (docs/03 "Değişiklik seti ve etki
// onizlemesi"; ADR-031/037/043). Etkilenen her kimlik ve her hedef sistem icin
// olmasi gereken durum IKI kez hesaplanir — yayimlanmis tanimla ve taslakla — ve
// karsilastirilir. Hesap sistemi okunmaz: sayi tahmin degil kesindir. Hesaplama
// `desired_state`in kendisidir (ADR-038), ikinci bir fark fonksiyonu yoktur.
//
// Sayilan fark (ADR-037): yetki ogesi ekleme/cikarma, hesap acma/silme/
// etkinlestirme/pasiflestirme, OU tasima. Yalnizca oznitelik degisimi (unvan,
// e-posta alan adi, UPN soneki) sayilmaz.
// Sayilmayan kimlikler (ADR-043): bagllantisi gozlem modunda olanlar, mevcut hesap
// ipucu bekleyenler ve iki tarafta da o hedefte hesabi olmayacaklar; onizleme
// bunlari ayrica "gozlemde: M kimlik" diye gosterir.

use std::collections::HashMap;

use sqlx::PgPool;

use crate::desired_state::{
    desired_state, AccountLink, AccountPresence, AdditionalRole, Clock, Date, Model, Origin,
    SingleValued, Source, TargetDefaults, Timeline,
};
use crate::org::{DefinitionEdit, Owner, MAX_DEPTH};

/// Kaydedilmek istenen tanim; yayimlanmis tanimin yerine konur.
pub struct Draft<'a> {
    pub owner: Owner,
    pub id: i64,
    pub edit: &'a DefinitionEdit,
    /// ADR-007: tek degerli ayar yalnizca birincil rolde ve departmanda okunur.
    pub with_settings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemChange {
    pub name: String,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Impact {
    /// Esige giren kimlik sayisi: taslak yayimlaninca hedefte islem uretecekler.
    pub applies: usize,
    /// Gozlemde / ipucu bekleyen / o hedefte hesabi olmayacak kimlikler (bilgi).
    pub observed: usize,
    /// Yetki ogesi dokumu, ada gore sirali.
    pub items: Vec<ItemChange>,
    /// Hesap durumu ya da OU farki ureten kimlik sayisi.
    pub account_changes: usize,
}

impl Impact {
    /// ADR-031: esigi asan set onay bekler; esitlik asma degildir.
    pub fn exceeds(&self, threshold: usize) -> bool {
        self.applies > threshold
    }
}

/// docs/09 boyutlandirma: kucuk kurum varsayilani. Orta ve buyuk kurum yukseltir.
pub const DEFAULT_THRESHOLD: usize = 10;

/// ADR-031: esik backend'in kendi ortam degiskenidir ve mutlak sayidir.
/// Verilmezse varsayilan; 0 her duzenlemeyi onaya dusurur ve gecerlidir.
pub fn threshold_from_env() -> Result<usize, String> {
    let Ok(raw) = std::env::var("CHANGE_SET_THRESHOLD") else {
        return Ok(DEFAULT_THRESHOLD);
    };
    raw.trim()
        .parse()
        .map_err(|_| format!("CHANGE_SET_THRESHOLD tam sayı olmalı, '{raw}' geldi"))
}

pub async fn preview(
    pool: &PgPool,
    time_zone: &str,
    draft: &Draft<'_>,
) -> Result<Impact, sqlx::Error> {
    let ids = crate::org::affected_identities(pool, draft.owner, draft.id).await?;
    if ids.is_empty() {
        return Ok(Impact::default());
    }
    let data = Data::load(pool, &ids, time_zone).await?;
    Ok(data.impact(draft))
}

// ---- toplu yukleme: kimlik sayisindan bagimsiz, sabit sayida sorgu ----

const IDENTITIES_SQL: &str = "SELECT id, to_char(start_date, 'YYYY-MM-DD'), \
     EXTRACT(EPOCH FROM end_at)::bigint, to_char(suspension_start, 'YYYY-MM-DD'), \
     to_char(suspension_end, 'YYYY-MM-DD'), cancelled, emergency_departure, \
     EXTRACT(EPOCH FROM deleted_at)::bigint, EXTRACT(EPOCH FROM now())::bigint, \
     to_char((now() AT TIME ZONE $2)::date, 'YYYY-MM-DD'), department_id, primary_role_id, \
     existing_ad_account_hint IS NOT NULL FROM identities WHERE id = ANY($1) AND deleted_at IS NULL";

const CHAIN_SQL: &str = "WITH RECURSIVE chain AS ( \
       SELECT id AS root, id, parent_id, 0 AS depth FROM departments WHERE id = ANY($1) \
       UNION ALL \
       SELECT c.root, d.id, d.parent_id, c.depth + 1 FROM departments d \
       JOIN chain c ON d.id = c.parent_id WHERE c.depth < $2) \
     SELECT root, id FROM chain ORDER BY root, depth";

const CATALOG_SQL: &str =
    "SELECT id, target_system_id, display_name FROM catalog_items WHERE missing_since IS NULL";

type IdentityRow = (
    i64,
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
    bool,
);

struct Person {
    timeline: Timeline,
    department: i64,
    primary_role: i64,
    hint: bool,
}

/// (sahip id, hedef) → yetki ogeleri / tek degerli ayar
type ByOwner<T> = HashMap<(i64, i64), T>;

struct Data {
    clock: Clock,
    people: Vec<(i64, Person)>,
    chains: HashMap<i64, Vec<i64>>,
    additional: HashMap<i64, Vec<(i64, Option<Date>)>>,
    titles: HashMap<i64, Option<String>>,
    base_roles: Vec<i64>,
    role_items: ByOwner<Vec<i64>>,
    dept_items: ByOwner<Vec<i64>>,
    role_settings: ByOwner<SingleValued>,
    dept_settings: ByOwner<SingleValued>,
    /// katalog ogesi → (hedef, gosterim adi); kayip oge listede yoktur (docs/03)
    catalog: HashMap<i64, (i64, String)>,
    targets: Vec<(i64, TargetDefaults)>,
    links: HashMap<(i64, i64), (AccountLink, bool)>,
}

fn date(text: &str) -> Result<Date, sqlx::Error> {
    Date::from_iso(text).ok_or_else(|| sqlx::Error::Decode(text.into()))
}

fn people_from(rows: &[IdentityRow]) -> Result<Vec<(i64, Person)>, sqlx::Error> {
    rows.iter()
        .map(|r| {
            Ok((
                r.0,
                Person {
                    timeline: Timeline {
                        start_date: date(&r.1)?,
                        end_at: r.2,
                        suspension_start: r.3.as_deref().map(date).transpose()?,
                        suspension_end: r.4.as_deref().map(date).transpose()?,
                        cancelled: r.5,
                        emergency_departure: r.6,
                        deleted_at: r.7,
                    },
                    department: r.10,
                    primary_role: r.11,
                    hint: r.12,
                },
            ))
        })
        .collect()
}

impl Data {
    async fn load(pool: &PgPool, ids: &[i64], time_zone: &str) -> Result<Data, sqlx::Error> {
        let rows: Vec<IdentityRow> = sqlx::query_as(IDENTITIES_SQL)
            .bind(ids)
            .bind(time_zone)
            .fetch_all(pool)
            .await?;
        let clock = match rows.first() {
            Some(r) => Clock {
                now: r.8,
                today: date(&r.9)?,
            },
            None => return Ok(Data::empty()),
        };
        let people = people_from(&rows)?;
        Ok(Data {
            clock,
            chains: load_chains(pool, &people).await?,
            additional: load_additional(pool, ids).await?,
            titles: load_titles(pool).await?,
            base_roles: sqlx::query_scalar("SELECT id FROM roles WHERE kind = 'base'")
                .fetch_all(pool)
                .await?,
            role_items: group_items(pool, Owner::Role).await?,
            dept_items: group_items(pool, Owner::Department).await?,
            role_settings: load_settings(pool, Owner::Role).await?,
            dept_settings: load_settings(pool, Owner::Department).await?,
            catalog: load_catalog(pool).await?,
            targets: load_targets(pool).await?,
            links: load_links(pool, ids).await?,
            people,
        })
    }

    fn empty() -> Data {
        Data {
            clock: Clock {
                now: 0,
                today: Date {
                    year: 1970,
                    month: 1,
                    day: 1,
                },
            },
            people: Vec::new(),
            chains: HashMap::new(),
            additional: HashMap::new(),
            titles: HashMap::new(),
            base_roles: Vec::new(),
            role_items: HashMap::new(),
            dept_items: HashMap::new(),
            role_settings: HashMap::new(),
            dept_settings: HashMap::new(),
            catalog: HashMap::new(),
            targets: Vec::new(),
            links: HashMap::new(),
        }
    }
}

async fn load_chains(
    pool: &PgPool,
    people: &[(i64, Person)],
) -> Result<HashMap<i64, Vec<i64>>, sqlx::Error> {
    let departments: Vec<i64> = people.iter().map(|(_, p)| p.department).collect();
    let rows: Vec<(i64, i64)> = sqlx::query_as(CHAIN_SQL)
        .bind(&departments)
        .bind(MAX_DEPTH)
        .fetch_all(pool)
        .await?;
    let mut chains: HashMap<i64, Vec<i64>> = HashMap::new();
    for (root, id) in rows {
        chains.entry(root).or_default().push(id);
    }
    Ok(chains)
}

async fn load_additional(
    pool: &PgPool,
    ids: &[i64],
) -> Result<HashMap<i64, Vec<(i64, Option<Date>)>>, sqlx::Error> {
    let rows: Vec<(i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT identity_id, role_id, to_char(ends_on, 'YYYY-MM-DD') \
         FROM identity_additional_roles WHERE identity_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut map: HashMap<i64, Vec<(i64, Option<Date>)>> = HashMap::new();
    for (identity, role, ends_on) in rows {
        let ends_on = ends_on.as_deref().map(date).transpose()?;
        map.entry(identity).or_default().push((role, ends_on));
    }
    Ok(map)
}

async fn load_titles(pool: &PgPool) -> Result<HashMap<i64, Option<String>>, sqlx::Error> {
    let rows: Vec<(i64, Option<String>)> = sqlx::query_as("SELECT id, title FROM roles")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

async fn group_items(pool: &PgPool, owner: Owner) -> Result<ByOwner<Vec<i64>>, sqlx::Error> {
    let sql = match owner {
        Owner::Role => {
            "SELECT e.role_id, c.target_system_id, e.catalog_item_id FROM role_entitlements e \
             JOIN catalog_items c ON c.id = e.catalog_item_id WHERE c.missing_since IS NULL"
        }
        Owner::Department => {
            "SELECT e.department_id, c.target_system_id, e.catalog_item_id \
             FROM department_entitlements e JOIN catalog_items c ON c.id = e.catalog_item_id \
             WHERE c.missing_since IS NULL"
        }
    };
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(sql).fetch_all(pool).await?;
    let mut map: ByOwner<Vec<i64>> = HashMap::new();
    for (owner_id, target, item) in rows {
        map.entry((owner_id, target)).or_default().push(item);
    }
    Ok(map)
}

async fn load_settings(pool: &PgPool, owner: Owner) -> Result<ByOwner<SingleValued>, sqlx::Error> {
    let sql = match owner {
        Owner::Role => {
            "SELECT role_id, target_system_id, provision_account, container_item_id, \
             email_domain, upn_suffix FROM role_target_settings"
        }
        Owner::Department => {
            "SELECT department_id, target_system_id, provision_account, container_item_id, \
             email_domain, upn_suffix FROM department_target_settings"
        }
    };
    type Row = (
        i64,
        i64,
        Option<bool>,
        Option<i64>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(sql).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(owner_id, target, provision_account, container, email_domain, upn_suffix)| {
                (
                    (owner_id, target),
                    SingleValued {
                        provision_account,
                        container,
                        email_domain,
                        upn_suffix,
                    },
                )
            },
        )
        .collect())
}

async fn load_catalog(pool: &PgPool) -> Result<HashMap<i64, (i64, String)>, sqlx::Error> {
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(CATALOG_SQL).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, target, name)| (id, (target, name)))
        .collect())
}

async fn load_targets(pool: &PgPool) -> Result<Vec<(i64, TargetDefaults)>, sqlx::Error> {
    let rows: Vec<(i64, bool, Option<i64>, i32, bool, i32)> = sqlx::query_as(
        "SELECT id, provision_account_default, default_container_item_id, retention_days, \
         delete_requires_approval, password_reset_delay_days FROM target_systems ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, provision_account, container, retention, approval, delay)| {
                (
                    id,
                    TargetDefaults {
                        provision_account,
                        container,
                        retention_days: retention.max(0) as u32,
                        delete_requires_approval: approval,
                        password_reset_delay_days: delay.max(0) as u32,
                    },
                )
            },
        )
        .collect())
}

async fn load_links(
    pool: &PgPool,
    ids: &[i64],
) -> Result<HashMap<(i64, i64), (AccountLink, bool)>, sqlx::Error> {
    let rows: Vec<(i64, i64, String, Option<bool>, bool, String)> = sqlx::query_as(
        "SELECT identity_id, target_system_id, origin, verified_unused, deletion_approved, mode \
         FROM account_links WHERE identity_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(identity, target, origin, verified_unused, deletion_approved, mode)| {
                let link = AccountLink {
                    origin: if origin == "adopted" {
                        Origin::Adopted
                    } else {
                        Origin::Provisioned
                    },
                    verified_unused,
                    deletion_approved,
                };
                ((identity, target), (link, mode == "observed"))
            },
        )
        .collect())
}

// ---- model kurulumu ve fark ----

impl Data {
    // Taslak eslesiyorsa yayimlanmis kaynagin yerine gecer.
    fn source(&self, owner: Owner, id: i64, target: i64, draft: Option<&Draft<'_>>) -> Source {
        let (items, settings) = match owner {
            Owner::Role => (&self.role_items, &self.role_settings),
            Owner::Department => (&self.dept_items, &self.dept_settings),
        };
        let matches = draft.filter(|d| d.owner == owner && d.id == id);
        let Some(draft) = matches else {
            return Source {
                entitlements: items.get(&(id, target)).cloned().unwrap_or_default(),
                settings: settings.get(&(id, target)).cloned().unwrap_or_default(),
            };
        };
        Source {
            entitlements: draft
                .edit
                .entitlement_ids
                .iter()
                .filter(|item| self.catalog.get(item).is_some_and(|(t, _)| *t == target))
                .copied()
                .collect(),
            settings: draft_settings(draft, target),
        }
    }

    fn model(
        &self,
        identity: i64,
        person: &Person,
        target: &(i64, TargetDefaults),
        d: Option<&Draft<'_>>,
    ) -> Model {
        let chain = self
            .chains
            .get(&person.department)
            .cloned()
            .unwrap_or_default();
        Model {
            base_entitlements: self
                .base_roles
                .iter()
                .flat_map(|role| self.source(Owner::Role, *role, target.0, d).entitlements)
                .collect(),
            department_chain: chain
                .iter()
                .map(|dept| self.source(Owner::Department, *dept, target.0, d))
                .collect(),
            primary_role: self.source(Owner::Role, person.primary_role, target.0, d),
            title: self.titles.get(&person.primary_role).cloned().flatten(),
            additional_roles: self
                .additional
                .get(&identity)
                .into_iter()
                .flatten()
                .map(|(role, ends_on)| AdditionalRole {
                    entitlements: self.source(Owner::Role, *role, target.0, d).entitlements,
                    ends_on: *ends_on,
                })
                .collect(),
            target: target.1.clone(),
        }
    }
}

fn draft_settings(draft: &Draft<'_>, target: i64) -> SingleValued {
    if !draft.with_settings {
        return SingleValued::default();
    }
    draft
        .edit
        .settings
        .iter()
        .find(|s| s.target_id == target)
        .map(|s| SingleValued {
            provision_account: s.provision_account,
            container: s.container_item_id,
            email_domain: blank_to_none(&s.email_domain),
            upn_suffix: blank_to_none(&s.upn_suffix),
        })
        .unwrap_or_default()
}

fn blank_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

struct Diff {
    /// ADR-043: fark var ama hedefte islem uretmez; esige girmez, "gozlemde" sayilir.
    excluded: bool,
    /// Hesap durumu ya da OU farki (ADR-037); yalnizca oznitelik sayilmaz.
    account: bool,
    added: Vec<i64>,
    removed: Vec<i64>,
}

impl Data {
    // None = bu hedefte hicbir fark yok.
    fn diff(
        &self,
        identity: i64,
        person: &Person,
        target: &(i64, TargetDefaults),
        draft: &Draft<'_>,
    ) -> Option<Diff> {
        let link = self.links.get(&(identity, target.0));
        let state = |d: Option<&Draft<'_>>| {
            desired_state(
                &person.timeline,
                &self.model(identity, person, target, d),
                link.map(|(l, _)| l),
                &self.clock,
            )
        };
        let (published, drafted) = (state(None), state(Some(draft)));
        let added: Vec<i64> = drafted
            .memberships
            .difference(&published.memberships)
            .copied()
            .collect();
        let removed: Vec<i64> = published
            .memberships
            .difference(&drafted.memberships)
            .copied()
            .collect();
        let account =
            published.account != drafted.account || published.container != drafted.container;
        if added.is_empty() && removed.is_empty() && !account {
            return None;
        }
        let no_account = AccountPresence::NotProvisioned;
        Some(Diff {
            excluded: link.is_some_and(|(_, observed)| *observed)
                || (link.is_none() && person.hint)
                || (published.account == no_account && drafted.account == no_account),
            account,
            added,
            removed,
        })
    }

    fn impact(&self, draft: &Draft<'_>) -> Impact {
        let mut counts: HashMap<i64, (usize, usize)> = HashMap::new();
        let mut impact = Impact::default();
        for (identity, person) in &self.people {
            let (mut changed, mut skipped, mut account) = (false, false, false);
            for target in &self.targets {
                let Some(diff) = self.diff(*identity, person, target, draft) else {
                    continue;
                };
                if diff.excluded {
                    skipped = true;
                    continue;
                }
                changed = true;
                account |= diff.account;
                for item in diff.added {
                    counts.entry(item).or_default().0 += 1;
                }
                for item in diff.removed {
                    counts.entry(item).or_default().1 += 1;
                }
            }
            impact.applies += usize::from(changed);
            impact.observed += usize::from(skipped && !changed);
            impact.account_changes += usize::from(account);
        }
        impact.items = self.item_changes(counts);
        impact
    }

    fn item_changes(&self, counts: HashMap<i64, (usize, usize)>) -> Vec<ItemChange> {
        let mut items: Vec<ItemChange> = counts
            .into_iter()
            .map(|(item, (added, removed))| ItemChange {
                name: self
                    .catalog
                    .get(&item)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| format!("#{item}")),
                added,
                removed,
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }
}
// --- END FEATURE: change-set ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org::TargetSetting;

    fn edit(items: Vec<i64>, settings: Vec<TargetSetting>) -> DefinitionEdit {
        DefinitionEdit {
            name: "Test Rolü".to_string(),
            entitlement_ids: items,
            settings,
        }
    }

    fn setting(target: i64, provision: Option<bool>, container: Option<i64>) -> TargetSetting {
        TargetSetting {
            target_id: target,
            target_name: String::new(),
            provision_account: provision,
            container_item_id: container,
            email_domain: String::new(),
            upn_suffix: String::new(),
        }
    }

    #[test]
    fn exceeds_only_above_threshold() {
        let impact = |applies| Impact {
            applies,
            ..Impact::default()
        };
        assert!(!impact(10).exceeds(10), "eşitlik aşma değildir");
        assert!(impact(11).exceeds(10));
        assert!(impact(1).exceeds(0), "eşik 0: her düzenleme onaya düşer");
    }

    // ADR-037: yetki ekleme/cikarma ve hesap durumu sayilir, yalnizca oznitelik sayilmaz.
    // ADR-043: gozlem modundaki ve o hedefte hesabi olmayacak kimlikler esige girmez.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn counts_entitlement_and_account_differences_but_not_observed_identities() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let catalog = crate::test_support::seed_example_catalog(&pool).await;
        let role: i64 = sqlx::query_scalar("SELECT primary_role_id FROM identities WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
        let tz = "Europe/Istanbul";
        fn draft_of(role: i64, edit: &DefinitionEdit) -> Draft<'_> {
            Draft {
                owner: Owner::Role,
                id: role,
                edit,
                with_settings: true,
            }
        }

        // Rol GG-VPN kazaniyor ve OU hedef varsayilanindan (Personel) ayriliyor.
        let ou = Some(catalog.sistem_uzmanlari_ou);
        let add = edit(
            vec![catalog.gg_vpn],
            vec![setting(catalog.ad, Some(true), ou)],
        );
        let impact = preview(&pool, tz, &draft_of(role, &add)).await.unwrap();
        assert_eq!((impact.applies, impact.observed), (2, 0));
        assert_eq!(
            impact.items,
            vec![ItemChange {
                name: "GG-VPN".to_string(),
                added: 2,
                removed: 0
            }]
        );
        assert_eq!(
            impact.account_changes, 2,
            "OU Personel'den SistemUzmanlari'na"
        );

        // Yayimla, sonra yalnizca oznitelik degistir: sayilmaz (ADR-037).
        crate::org::save_role(&pool, role, "", &add).await.unwrap();
        let attribute_only = DefinitionEdit {
            settings: vec![TargetSetting {
                email_domain: "example.com".to_string(),
                ..setting(catalog.ad, Some(true), ou)
            }],
            ..edit(vec![catalog.gg_vpn], vec![])
        };
        let impact = preview(&pool, tz, &draft_of(role, &attribute_only))
            .await
            .unwrap();
        assert_eq!(impact, Impact::default(), "yalnızca öznitelik sayılmaz");

        // Cikarma: GG-VPN gidiyor, GG-Internet geliyor.
        let swap = edit(
            vec![catalog.gg_internet],
            vec![setting(catalog.ad, Some(true), ou)],
        );
        let impact = preview(&pool, tz, &draft_of(role, &swap)).await.unwrap();
        assert_eq!(impact.applies, 2);
        let names: Vec<(&str, usize, usize)> = impact
            .items
            .iter()
            .map(|i| (i.name.as_str(), i.added, i.removed))
            .collect();
        assert_eq!(names, vec![("GG-Internet", 2, 0), ("GG-VPN", 0, 2)]);

        // ADR-043: birinin baglantisi gozlem modunda → esige girmez, "gözlemde" sayilir.
        sqlx::query(
            "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode) \
             VALUES ($1, $2, 'guid-1', 'adopted', 'observed')",
        )
        .bind(ids[0])
        .bind(catalog.ad)
        .execute(&pool)
        .await
        .unwrap();
        let impact = preview(&pool, tz, &draft_of(role, &swap)).await.unwrap();
        assert_eq!((impact.applies, impact.observed), (1, 1));

        // ADR-043: hesap acilsin = hayir ve baglantisi yok → o hedefte islem yok.
        let no_account = edit(
            vec![catalog.gg_internet],
            vec![setting(catalog.ad, Some(false), None)],
        );
        let impact = preview(&pool, tz, &draft_of(role, &no_account))
            .await
            .unwrap();
        assert_eq!(
            (impact.applies, impact.observed),
            (1, 1),
            "bağlantısı olan gözlemde, olmayan hesapsız"
        );

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
