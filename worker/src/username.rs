// --- START FEATURE: username-generation ---
// Kullanici adi ve e-posta uretimi (ADR-011, 022, 035, 042, 058): kurulum
// sablonu + sabit normallestirme (Turkce kucuk harf, aksan atma, a-z0-9),
// 20/64 karakter siniri, cakisma icin en kucuk n. Cakisma yalnizca veritabani
// (silinmemis kimlikler, kullanilmis ad kaydi) ve AD'de aranir; Zimbra kontrolu
// Zimbra bolumunde eklenir (ADR-058: erisilemezken zaten atlanir).
// Bagli olmayan AD hesabiyla ya da kullanilmis adla cakisma n+1 degil MUDAHALEdir.

use ldap3::{ldap_escape, Ldap, Scope};
use sqlx::PgPool;

use crate::ad;
use crate::writes::WriteError;

pub const USERNAME_MAX_LEN: usize = 20;
pub const EMAIL_LOCAL_MAX_LEN: usize = 64;
const SUFFIX_RESERVE: usize = 2;
const MAX_SUFFIX: u32 = 99;
const DEFAULT_TEMPLATE: &str = "{given_first}.{surname}";

pub struct Templates {
    pub username: String,
    pub email_local: String,
}

impl Templates {
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Templates {
        let get = |name: &str| {
            lookup(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_TEMPLATE.to_string())
        };
        Templates {
            username: get("USERNAME_TEMPLATE"),
            email_local: get("EMAIL_LOCAL_TEMPLATE"),
        }
    }
}

pub struct NameInput<'a> {
    pub given_names: &'a str,
    pub surname: &'a str,
    pub employee_number: Option<&'a str>,
}

// ADR-011 normallestirme: Turkce kucuk harf (I → i, i̇ → i), cgiosu donusumu,
// diger aksanlar NFD ile atilir, a-z0-9 disindaki her sey atilir.
// Kural ikiz dosyada (`normalize.rs`): backend okunur adres icin ayni kurali okur (ADR-107).
pub use crate::normalize::normalize_component;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GivenForm {
    First,
    Initial,
}

fn placeholder_value(name: &str, input: &NameInput<'_>, form: GivenForm) -> Option<String> {
    let given_first = input.given_names.split_whitespace().next().unwrap_or("");
    let value = match name {
        "given" => normalize_component(input.given_names),
        "given_first" => match form {
            GivenForm::First => normalize_component(given_first),
            GivenForm::Initial => normalize_component(given_first).chars().take(1).collect(),
        },
        "given_initial" => normalize_component(given_first).chars().take(1).collect(),
        "surname" => normalize_component(input.surname),
        "employee_number" => normalize_component(input.employee_number.unwrap_or("")),
        _ => return None,
    };
    Some(value)
}

// Sablonu doldurur; ayiricilar korunur, art arda noktalar teke iner.
fn render(template: &str, input: &NameInput<'_>, form: GivenForm) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("şablon bozuk: {template}"))?;
        let name = &rest[start + 1..start + end];
        let value = placeholder_value(name, input, form)
            .ok_or_else(|| format!("bilinmeyen yer tutucu: {{{name}}}"))?;
        out.push_str(&value);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let collapsed = collapse_dots(&out);
    validate_shape(&collapsed)?;
    Ok(collapsed)
}

fn collapse_dots(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '.' && out.ends_with('.') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('.').to_string()
}

fn validate_shape(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("ad boş kaldı; elle girin".to_string());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.')
    {
        return Err(format!("izinli olmayan karakter kaldı: {name}"));
    }
    Ok(())
}

// ADR-011 uzunluk: 20'yi asarsa once bas harf, hala asiyorsa soyad kesilir,
// cakisma soneki icin yer birakilir.
pub fn base_username(templates: &Templates, input: &NameInput<'_>) -> Result<String, String> {
    let full = render(&templates.username, input, GivenForm::First)?;
    if full.len() <= USERNAME_MAX_LEN {
        return Ok(full);
    }
    let initial = render(&templates.username, input, GivenForm::Initial)?;
    if initial.len() <= USERNAME_MAX_LEN {
        return Ok(initial);
    }
    let excess = initial.len() - (USERNAME_MAX_LEN - SUFFIX_RESERVE);
    let surname = normalize_component(input.surname);
    let shortened = surname
        .chars()
        .take(surname.len().saturating_sub(excess).max(1))
        .collect::<String>();
    let trimmed = NameInput {
        surname: &shortened,
        ..*input
    };
    render(&templates.username, &trimmed, GivenForm::Initial)
}

pub fn base_email_local(templates: &Templates, input: &NameInput<'_>) -> Result<String, String> {
    let full = render(&templates.email_local, input, GivenForm::First)?;
    Ok(full.chars().take(EMAIL_LOCAL_MAX_LEN).collect())
}

// ADR-022: elle girilen ad ayni normallestirme ve dogrulamadan gecer.
pub fn validate_manual(raw: &str) -> Result<String, String> {
    let normalized = raw
        .split('.')
        .map(normalize_component)
        .collect::<Vec<_>>()
        .join(".");
    let name = collapse_dots(&normalized);
    validate_shape(&name)?;
    if name.len() > USERNAME_MAX_LEN {
        return Err(format!(
            "kullanıcı adı en fazla {USERNAME_MAX_LEN} karakter"
        ));
    }
    Ok(name)
}

pub fn with_suffix(base: &str, n: u32, max_len: usize) -> String {
    if n <= 1 {
        return base.to_string();
    }
    let suffix = n.to_string();
    let keep = max_len.saturating_sub(suffix.len()).min(base.len());
    format!("{}{suffix}", &base[..keep])
}

pub struct Candidate {
    pub username: String,
    pub email_local: String,
    /// Elle girilen ad: cakismada n eklenmez, mudahale (ADR-022)
    pub manual: bool,
    /// Operator "farkli kisi, siradaki adi ver" dedi (ADR-022 2. madde, ADR-042):
    /// bagli olmayan hesap / kullanilmis ad cakismasinda da n + 1 denenir
    pub override_conflicts: bool,
}

pub struct Context<'a> {
    pub base_dn: &'a str,
    pub upn_suffix: &'a str,
    pub email_domain: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    Ok {
        username: String,
        email_local: String,
    },
    NeedsIntervention(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Conflict {
    None,
    /// OpenSicil'in kendi kimligi ya da bagli hesabi: n + 1 denenir
    Linked(String),
    /// Kullanilmis ad ya da bagli olmayan hedef hesabi: insan karari
    Intervention(String),
}

async fn db_conflict(
    pool: &PgPool,
    username: &str,
    email: Option<&str>,
) -> Result<Conflict, sqlx::Error> {
    let identity: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM identities WHERE deleted_at IS NULL \
         AND (lower(username) = lower($1) OR ($2::text IS NOT NULL AND lower(email) = lower($2))) LIMIT 1",
    )
    .bind(username)
    .bind(email)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = identity {
        return Ok(Conflict::Linked(format!("kimlik #{id} bu adı taşıyor")));
    }
    let burned: Option<(String, String)> = sqlx::query_as(
        "SELECT kind, to_char(burned_at, 'YYYY-MM-DD') FROM used_names WHERE released_at IS NULL \
         AND ((kind = 'username' AND lower(name) = lower($1)) \
           OR ($2::text IS NOT NULL AND kind = 'email' AND lower(name) = lower($2))) LIMIT 1",
    )
    .bind(username)
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(match burned {
        Some((kind, date)) => Conflict::Intervention(format!(
            "{kind} kullanılmış ad kaydında ({date} tarihinde yakıldı); Sistem yöneticisi serbest bırakabilir"
        )),
        None => Conflict::None,
    })
}

// ADR-011 AD kontrolu: sAMAccountName, userPrincipalName, mail, proxyAddresses.
// Bulunan hesap OpenSicil'e bagliysa n + 1, degilse mudahale (ADR-022).
async fn ad_conflict(
    pool: &PgPool,
    ldap: &mut Ldap,
    ctx: &Context<'_>,
    username: &str,
    email: Option<&str>,
) -> Result<Conflict, WriteError> {
    let upn = ldap_escape(format!("{username}@{}", ctx.upn_suffix));
    let mut filter = format!(
        "(|(sAMAccountName={})(userPrincipalName={upn})",
        ldap_escape(username)
    );
    if let Some(email) = email {
        let escaped = ldap_escape(email);
        filter.push_str(&format!("(mail={escaped})(proxyAddresses=smtp:{escaped})"));
    }
    filter.push(')');
    let found = ad::search(ldap, ctx.base_dn, Scope::Subtree, &filter, &["objectGUID"]).await?;
    // Ilk eslesme yeter: bagli degilse zaten mudahale, bagliysa n + 1
    let Some(entry) = found.into_iter().next() else {
        return Ok(Conflict::None);
    };
    let guid = entry
        .bin_attrs
        .get("objectGUID")
        .and_then(|v| v.first())
        .and_then(|b| ad::guid_to_string(b));
    let linked = match guid {
        Some(guid) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT FROM account_links WHERE external_id = $1)",
        )
        .bind(guid)
        .fetch_one(pool)
        .await
        .map_err(|e| WriteError::Failed(e.to_string()))?,
        None => false,
    };
    if linked {
        Ok(Conflict::Linked(format!("bağlı hesap {}", entry.dn)))
    } else {
        Ok(Conflict::Intervention(format!(
            "{username} AD'de var ({}), OpenSicil'e bağlı değil",
            entry.dn
        )))
    }
}

pub async fn resolve(
    pool: &PgPool,
    ldap: &mut Ldap,
    ctx: &Context<'_>,
    candidate: &Candidate,
) -> Result<Resolution, WriteError> {
    for n in 1..=MAX_SUFFIX {
        let username = with_suffix(&candidate.username, n, USERNAME_MAX_LEN);
        let email_local = with_suffix(&candidate.email_local, n, EMAIL_LOCAL_MAX_LEN);
        let email = ctx.email_domain.map(|d| format!("{email_local}@{d}"));
        let conflict = match db_conflict(pool, &username, email.as_deref())
            .await
            .map_err(|e| WriteError::Failed(e.to_string()))?
        {
            Conflict::None => ad_conflict(pool, ldap, ctx, &username, email.as_deref()).await?,
            other => other,
        };
        match conflict {
            Conflict::None => {
                return Ok(Resolution::Ok {
                    username,
                    email_local,
                })
            }
            Conflict::Intervention(_) if candidate.override_conflicts && !candidate.manual => {
                continue
            }
            Conflict::Intervention(reason) => return Ok(Resolution::NeedsIntervention(reason)),
            Conflict::Linked(reason) if candidate.manual => {
                return Ok(Resolution::NeedsIntervention(format!(
                    "elle girilen ad çakışıyor: {reason}"
                )))
            }
            Conflict::Linked(_) => continue,
        }
    }
    Ok(Resolution::NeedsIntervention(format!(
        "{} için {MAX_SUFFIX} sonek de dolu",
        candidate.username
    )))
}
// --- END FEATURE: username-generation ---

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Templates {
        Templates::from_lookup(|_| None)
    }

    fn input<'a>(given: &'a str, surname: &'a str) -> NameInput<'a> {
        NameInput {
            given_names: given,
            surname,
            employee_number: Some("00123"),
        }
    }

    #[test]
    fn adr_011_examples() {
        let t = defaults();
        let cases = [
            ("Ahmet", "Yılmaz", "ahmet.yilmaz", "ahmet.yilmaz"),
            (
                "Mehmet Ali",
                "Yıldırım",
                "mehmet.yildirim",
                "mehmet.yildirim",
            ),
            (
                "Abdurrahman",
                "Karaosmanoğlu",
                "a.karaosmanoglu",
                "abdurrahman.karaosmanoglu",
            ),
            ("İbrahim", "Işık", "ibrahim.isik", "ibrahim.isik"),
            ("Émile", "O'Brien", "emile.obrien", "emile.obrien"),
        ];
        for (given, surname, username, email) in cases {
            let i = input(given, surname);
            assert_eq!(
                base_username(&t, &i).unwrap(),
                username,
                "{given} {surname}"
            );
            assert_eq!(
                base_email_local(&t, &i).unwrap(),
                email,
                "{given} {surname}"
            );
        }
    }

    #[test]
    fn long_surname_is_truncated_leaving_room_for_suffix() {
        let name = base_username(
            &defaults(),
            &input("Abdurrahman", "Karaosmanoğlugilleroğlu"),
        )
        .unwrap();
        assert!(name.len() <= USERNAME_MAX_LEN - SUFFIX_RESERVE, "{name}");
        assert!(name.starts_with("a.karaosmanog"), "{name}");
        assert_eq!(
            with_suffix(&name, 12, USERNAME_MAX_LEN).len(),
            name.len() + 2
        );
    }

    #[test]
    fn empty_or_foreign_script_needs_manual_entry() {
        assert!(base_username(&defaults(), &input("Иван", "Петров")).is_err());
        assert!(base_username(&defaults(), &input("", "")).is_err());
    }

    #[test]
    fn custom_templates_and_placeholders() {
        let t = Templates::from_lookup(|name| match name {
            "USERNAME_TEMPLATE" => Some("{given_initial}{surname}".to_string()),
            "EMAIL_LOCAL_TEMPLATE" => Some("{given}.{surname}.{employee_number}".to_string()),
            _ => None,
        });
        let i = input("Mehmet Ali", "Yıldırım");
        assert_eq!(base_username(&t, &i).unwrap(), "myildirim");
        assert_eq!(
            base_email_local(&t, &i).unwrap(),
            "mehmetali.yildirim.00123"
        );
        let bad = Templates {
            username: "{unknown}".to_string(),
            email_local: DEFAULT_TEMPLATE.to_string(),
        };
        assert!(base_username(&bad, &i).is_err());
    }

    #[test]
    fn manual_entry_is_normalized_and_validated() {
        assert_eq!(validate_manual("Ayşe.YILMAZ").unwrap(), "ayse.yilmaz");
        assert_eq!(validate_manual("..a..b..").unwrap(), "a.b");
        assert!(validate_manual("").is_err());
        assert!(validate_manual(&"a".repeat(21)).is_err());
    }

    #[test]
    fn suffix_fits_in_limit() {
        assert_eq!(with_suffix("ahmet.yilmaz", 1, 20), "ahmet.yilmaz");
        assert_eq!(with_suffix("ahmet.yilmaz", 2, 20), "ahmet.yilmaz2");
        assert_eq!(
            with_suffix("abcdefghijklmnopqrst", 12, 20),
            "abcdefghijklmnopqr12"
        );
    }

    // Lab Samba AD + Postgres: mevcut.personel bagli degil → mudahale; bos ad → uretilir;
    // silinmemis kimlik → n + 1; kullanilmis ad → mudahale; elle giris cakisinca mudahale.
    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn resolves_against_database_and_lab_ad() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let cfg = ad::AdConfig {
            urls: ad::parse_urls(&var("AD_LAB_URL")),
            bind_dn: var("AD_LAB_BIND_DN"),
            password: var("AD_LAB_PASSWORD"),
            ca_file: var("AD_CA_FILE"),
        };
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_example_model(&pool).await;
        sqlx::query("UPDATE identities SET username = 'ayse.yilmaz', email = 'ayse.yilmaz@opensicil.lab' WHERE id = $1")
            .bind(ids.identity)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO used_names (name, kind) VALUES ('yanik.ad', 'username')")
            .execute(&pool)
            .await
            .unwrap();
        let mut ldap = ad::connect(&cfg).await.expect("lab AD");
        let ctx = Context {
            base_dn: "DC=opensicil,DC=lab",
            upn_suffix: "opensicil.lab",
            email_domain: Some("opensicil.lab"),
        };
        let candidate = |username: &str, manual: bool| Candidate {
            username: username.to_string(),
            email_local: username.to_string(),
            manual,
            override_conflicts: false,
        };
        let next_name = |username: &str| Candidate {
            override_conflicts: true,
            ..candidate(username, false)
        };

        let fresh = resolve(&pool, &mut ldap, &ctx, &candidate("yeni.kisi", false))
            .await
            .unwrap();
        assert_eq!(
            fresh,
            Resolution::Ok {
                username: "yeni.kisi".into(),
                email_local: "yeni.kisi".into()
            }
        );
        let bumped = resolve(&pool, &mut ldap, &ctx, &candidate("ayse.yilmaz", false))
            .await
            .unwrap();
        assert_eq!(
            bumped,
            Resolution::Ok {
                username: "ayse.yilmaz2".into(),
                email_local: "ayse.yilmaz2".into()
            }
        );
        assert!(
            matches!(
                resolve(&pool, &mut ldap, &ctx, &candidate("ayse.yilmaz", true))
                    .await
                    .unwrap(),
                Resolution::NeedsIntervention(_)
            ),
            "elle girilen ad çakışınca n eklenmez"
        );
        assert!(
            matches!(
                resolve(&pool, &mut ldap, &ctx, &candidate("yanik.ad", false))
                    .await
                    .unwrap(),
                Resolution::NeedsIntervention(_)
            ),
            "kullanılmış ad müdahaledir (ADR-042)"
        );
        // ADR-035: serbest birakilan ad yeniden uretilir (yakma kaydi artik engellemez).
        sqlx::query(
            "INSERT INTO used_names (name, kind, released_at, release_reason) \
             VALUES ('serbest.ad', 'username', now(), 'aynı kişi geri döndü')",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            resolve(&pool, &mut ldap, &ctx, &candidate("serbest.ad", false))
                .await
                .unwrap(),
            Resolution::Ok {
                username: "serbest.ad".into(),
                email_local: "serbest.ad".into()
            },
            "serbest bırakılan ad sonek almadan yeniden verilir"
        );
        let unlinked = resolve(&pool, &mut ldap, &ctx, &candidate("mevcut.personel", false))
            .await
            .unwrap();
        assert!(
            matches!(&unlinked, Resolution::NeedsIntervention(r) if r.contains("bağlı değil")),
            "{unlinked:?}"
        );
        // ADR-022 2. madde: "farkli kisi, siradaki adi ver" → bagli olmayan hesap ve
        // kullanilmis ad cakismasinda n + 1.
        for (name, expected) in [
            ("mevcut.personel", "mevcut.personel2"),
            ("yanik.ad", "yanik.ad2"),
        ] {
            assert_eq!(
                resolve(&pool, &mut ldap, &ctx, &next_name(name))
                    .await
                    .unwrap(),
                Resolution::Ok {
                    username: expected.into(),
                    email_local: expected.into()
                }
            );
        }

        ldap.unbind().await.ok();
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
