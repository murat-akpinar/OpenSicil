// --- START FEATURE: access-report ---
// Yetki dokumu (ADR-134 madde 6): rol → kisiler ve uyelik ogesi (grup, dagitim
// listesi) → kisiler; erisim gozden gecirmesi icin ayni dokum CSV olarak.
// Yalnizca var olan tablolardan okunur. Uyelik "olmasi gereken"dir: onizlemenin
// `desired_state` hesabi (`change_set::holdings`), hedef sistem okunmaz.
// Okuma her operatorde (auditor dahil), indirme denetime girer.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use sqlx::PgPool;

use crate::activity::{csv_cell, REPORT_TABS};
use crate::i18n::Lang;
use crate::identity_web::{allowed, audit_operator, forbidden, internal, OperatorSession};
use crate::shell::{Shell, Tabs};
use crate::web::{render, AppState};

#[derive(Debug, Clone, PartialEq)]
pub struct Holder {
    pub id: i64,
    pub name: String,
    pub employee_number: String,
    pub username: String,
}

/// Bir rol ya da uyelik ogesi ve onu tasiyan kisiler (ada gore).
#[derive(Debug, PartialEq)]
pub struct Grant {
    pub name: String,
    /// Rolde rol turu (`base`, `primary`, `additional`), ogede hedef sistemin adi
    pub scope: String,
    pub holders: Vec<Holder>,
}

/// (kimlik, sahip) ciftlerini sahibe gore toplar. Adi bilinmeyen sahip ya da
/// kisi atlanir; sira (kapsam, ad), kisiler ada gore.
fn grants(
    pairs: &[(i64, i64)],
    owners: &HashMap<i64, (String, String)>,
    people: &HashMap<i64, Holder>,
) -> Vec<Grant> {
    let mut by_owner: HashMap<i64, Vec<Holder>> = HashMap::new();
    for (identity, owner) in pairs {
        if let (true, Some(person)) = (owners.contains_key(owner), people.get(identity)) {
            by_owner.entry(*owner).or_default().push(person.clone());
        }
    }
    let mut out: Vec<Grant> = by_owner
        .into_iter()
        .map(|(owner, mut holders)| {
            holders.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));
            holders.dedup_by_key(|h| h.id);
            let (name, scope) = owners[&owner].clone();
            Grant {
                name,
                scope,
                holders,
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
    out
}

async fn load(pool: &PgPool, tz: &str) -> Result<(Vec<Grant>, Vec<Grant>), sqlx::Error> {
    let (holdings, people, roles, items) = tokio::try_join!(
        crate::change_set::holdings(pool, tz),
        sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT id, btrim(given_name || ' ' || surname), coalesce(employee_number, ''), \
             coalesce(username, '') FROM identities WHERE deleted_at IS NULL",
        )
        .fetch_all(pool),
        sqlx::query_as::<_, (i64, String, String)>("SELECT id, name, kind FROM roles")
            .fetch_all(pool),
        sqlx::query_as::<_, (i64, String, String)>(
            "SELECT c.id, c.display_name, t.name FROM catalog_items c \
             JOIN target_systems t ON t.id = c.target_system_id WHERE c.is_membership",
        )
        .fetch_all(pool),
    )?;
    let people: HashMap<i64, Holder> = people
        .into_iter()
        .map(|(id, name, employee_number, username)| {
            let holder = Holder {
                id,
                name,
                employee_number,
                username,
            };
            (id, holder)
        })
        .collect();
    let owners = |rows: Vec<(i64, String, String)>| -> HashMap<i64, (String, String)> {
        rows.into_iter().map(|(id, n, s)| (id, (n, s))).collect()
    };
    Ok((
        grants(&holdings.roles, &owners(roles), &people),
        grants(&holdings.items, &owners(items), &people),
    ))
}

/// Tek tablo: once roller, sonra uyelik ogeleri; kisi basina bir satir.
fn to_csv(lang: Lang, roles: &[Grant], items: &[Grant]) -> String {
    let line = |cells: [&str; 6]| {
        let cells: Vec<String> = cells.iter().map(|c| csv_cell(c)).collect();
        cells.join(",") + "\r\n"
    };
    let mut out = String::from("\u{feff}");
    out.push_str(&line(
        [
            "kind",
            "name",
            "scope",
            "person",
            "employee_number",
            "username",
        ]
        .map(|k| lang.key("access", k)),
    ));
    for (kind, list, is_role) in [("kind_role", roles, true), ("kind_item", items, false)] {
        for g in list {
            let scope = match is_role {
                true => lang.key("rolekind", &g.scope),
                false => &g.scope,
            };
            for h in &g.holders {
                out.push_str(&line([
                    lang.key("access", kind),
                    &g.name,
                    scope,
                    &h.name,
                    &h.employee_number,
                    &h.username,
                ]));
            }
        }
    }
    out
}

/// Sayfanin iki karti: rol dokumu ve uyelik dokumu.
struct Section<'a> {
    title: &'static str,
    hint: &'static str,
    is_role: bool,
    grants: &'a [Grant],
}

#[derive(Template)]
#[template(path = "access.html")]
struct AccessTemplate<'a> {
    lang: Lang,
    shell: Shell,
    tabs: Tabs,
    sections: [Section<'a>; 2],
}

#[derive(Deserialize, Default)]
struct Params {
    format: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/reports/access", get(page))
}

async fn page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Query(p): Query<Params>,
) -> Response {
    if !allowed(&op, &crate::shell::AUTHORITY_ORDER) {
        return forbidden(op.lang);
    }
    let tz = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    // ponytail: butun kurum tek istekte bellege alinir; buyuk kurumda yavaslarsa rol/hedef filtresi
    let (roles, items) = match load(&state.pool, &tz).await {
        Ok(loaded) => loaded,
        Err(e) => return internal("yetki dökümü okunamadı", e),
    };
    if p.format.as_deref() != Some("csv") {
        return render(&AccessTemplate {
            lang: op.lang,
            shell: Shell::of(&op),
            tabs: Tabs::new(op.lang, "nav.reports", &REPORT_TABS, "/reports/access"),
            sections: [
                Section {
                    title: "access.roles",
                    hint: "access.roles_hint",
                    is_role: true,
                    grants: &roles,
                },
                Section {
                    title: "access.items",
                    hint: "access.items_hint",
                    is_role: false,
                    grants: &items,
                },
            ],
        });
    }
    let rows: usize = roles.iter().chain(&items).map(|g| g.holders.len()).sum();
    audit_operator(
        &state,
        &op,
        crate::audit::ACCESS_EXPORTED,
        None,
        serde_json::json!({"rows": rows}),
    )
    .await;
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"opensicil-yetki-dokumu.csv\"",
            ),
        ],
        to_csv(op.lang, &roles, &items),
    )
        .into_response()
}
// --- END FEATURE: access-report ---

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(id: i64, name: &str) -> Holder {
        Holder {
            id,
            name: name.into(),
            employee_number: format!("{id:03}"),
            username: String::new(),
        }
    }

    #[test]
    fn pairs_group_by_owner_sorted_and_unknowns_dropped() {
        let people = HashMap::from([(1, holder(1, "Zeynep")), (2, holder(2, "Ali"))]);
        let owners = HashMap::from([
            (10, ("GG-VPN".to_string(), "AD".to_string())),
            (11, ("bt".to_string(), "Zimbra".to_string())),
            (12, ("GG-Internet".to_string(), "AD".to_string())),
        ]);
        // 99 bilinmeyen sahip, 3 bilinmeyen kisi; (1, 10) iki kez gelir
        let pairs = [(1, 10), (2, 10), (1, 10), (1, 11), (3, 12), (1, 99)];
        let got = grants(&pairs, &owners, &people);
        let names: Vec<(&str, Vec<&str>)> = got
            .iter()
            .map(|g| {
                let holders = g.holders.iter().map(|h| h.name.as_str()).collect();
                (g.name.as_str(), holders)
            })
            .collect();
        assert_eq!(
            names,
            [("GG-VPN", vec!["Ali", "Zeynep"]), ("bt", vec!["Zeynep"])]
        );
    }

    #[test]
    fn csv_has_one_row_per_person_and_defuses_formulas() {
        let lang = crate::i18n::DEFAULT;
        let role = Grant {
            name: "=Muhasebe".into(),
            scope: "primary".into(),
            holders: vec![holder(1, "Ali")],
        };
        let item = Grant {
            name: "GG-VPN".into(),
            scope: "Active Directory".into(),
            holders: vec![holder(1, "Ali"), holder(2, "Ayşe")],
        };
        let csv = to_csv(lang, &[role], &[item]);
        let lines: Vec<&str> = csv.strip_prefix('\u{feff}').expect("BOM").lines().collect();
        assert_eq!(
            lines[0],
            "Tür,Ad,Rol türü / hedef sistem,Kişi,Sicil,Kullanıcı adı"
        );
        assert_eq!(lines[1], "Rol,'=Muhasebe,Birincil,Ali,001,");
        assert_eq!(lines[3], "Grup / liste,GG-VPN,Active Directory,Ayşe,002,");
        assert_eq!(lines.len(), 4);
    }

    // Gercek Postgres gerektirir (ADR-070).
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn holders_follow_the_definitions_and_departed_drop_out() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let cat = crate::test_support::seed_example_catalog(&pool).await;
        let [ayse, ali] = crate::test_support::seed_two_identities(&pool).await;
        // Temel rol herkese GG-Internet, ek rol (Ali, suresi dolmus) GG-VPN,
        // departman bt listesi; Ayse ayrilmis
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO roles (kind, name) VALUES ('base', 'Herkes'), ('additional', 'VPN'); \
             INSERT INTO role_entitlements SELECT id, {internet} FROM roles WHERE kind = 'base'; \
             INSERT INTO role_entitlements SELECT id, {vpn} FROM roles WHERE name = 'VPN'; \
             INSERT INTO department_entitlements SELECT id, {bt} FROM departments; \
             INSERT INTO identity_additional_roles (identity_id, role_id, ends_on) \
               SELECT {ali}, id, NULL FROM roles WHERE name = 'VPN'; \
             UPDATE identities SET end_at = now() - interval '1 day' WHERE id = {ayse};",
            internet = cat.gg_internet,
            vpn = cat.gg_vpn,
            bt = cat.list_bt,
        )))
        .execute(&pool)
        .await
        .unwrap();
        let (roles, items) = load(&pool, "Europe/Istanbul").await.unwrap();
        let flat = |list: &[Grant]| {
            list.iter()
                .map(|g| {
                    let people: Vec<String> = g.holders.iter().map(|h| h.name.clone()).collect();
                    (g.name.clone(), people)
                })
                .collect::<Vec<_>>()
        };
        let only_ali = vec!["Ali Kaya".to_string()];
        assert_eq!(
            flat(&roles),
            [
                ("VPN".to_string(), only_ali.clone()),
                ("Herkes".to_string(), only_ali.clone()),
                ("Test Rolü".to_string(), only_ali.clone()),
            ]
        );
        assert_eq!(
            flat(&items),
            [
                ("GG-Internet".to_string(), only_ali.clone()),
                ("GG-VPN".to_string(), only_ali.clone()),
                ("bt".to_string(), only_ali.clone()),
            ]
        );

        // Suresi dolan ek rol rolden de uyelikten de duser
        sqlx::query("UPDATE identity_additional_roles SET ends_on = current_date - 1")
            .execute(&pool)
            .await
            .unwrap();
        let (roles, items) = load(&pool, "Europe/Istanbul").await.unwrap();
        assert!(roles.iter().all(|g| g.name != "VPN"));
        assert!(items.iter().all(|g| g.name != "GG-VPN"));

        // HTTP: auditor okur ve indirir, indirme denetimde; yetkisiz 403
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let get = |uri: &'static str, cookie: String| {
            let app = app.clone();
            async move {
                let res = app
                    .oneshot(
                        Request::builder()
                            .uri(uri)
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = res.status();
                let body = axum::body::to_bytes(res.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let auditor = crate::test_support::operator_cookie(&pool, "denetci", &["auditor"]).await;
        let (status, page) = get("/reports/access", auditor.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains(&format!("href=\"/identities/{ali}\"")));
        assert!(!page.contains(&format!("href=\"/identities/{ayse}\"")));
        assert!(page.contains(r#"aria-current="page""#));
        let (status, csv) = get("/reports/access?format=csv", auditor).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(csv.lines().count(), 1 + 2 + 2, "başlık + 2 rol + 2 üyelik");
        let detail: String =
            sqlx::query_scalar("SELECT detail::text FROM audit_log WHERE event_type = $1")
                .bind(crate::audit::ACCESS_EXPORTED)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&detail).unwrap()["rows"],
            4
        );
        let none = crate::test_support::operator_cookie(&pool, "denetci", &[]).await;
        assert_eq!(get("/reports/access", none).await.0, StatusCode::FORBIDDEN);

        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
