// --- START FEATURE: operational-settings ---
// ADR-131: isletme ayarlari `operational_settings` tablosunda, Ayarlar ekranindan
// verilir. Anahtar adlari eski ortam degiskeni adlariyla ayni. Dogrulama, okuyan
// tarafin kendi ayristiricisidir: bozuk deger tabloya hic girmez (madde 5).

use std::collections::HashMap;

use sqlx::PgPool;

use crate::scope::{GROUP_OUS, PASSIVE_OU, USER_OUS};

const ZIMBRA_DOMAINS: &str = "ZIMBRA_MANAGED_DOMAINS";
const USERNAME_TEMPLATE: &str = "USERNAME_TEMPLATE";
const EMAIL_LOCAL_TEMPLATE: &str = "EMAIL_LOCAL_TEMPLATE";
const DRY_RUN: &str = "DRY_RUN";
const FIRST_LOGIN: &str = "FIRST_LOGIN_CHANGE_REQUIRED";

/// Ekranin duzenledigi anahtarlar; ADR-131'in sonraki kutucuklari ekler.
pub const EDITABLE: [&str; 16] = [
    crate::change_set::THRESHOLD_KEY,
    USER_OUS,
    GROUP_OUS,
    PASSIVE_OU,
    ZIMBRA_DOMAINS,
    USERNAME_TEMPLATE,
    EMAIL_LOCAL_TEMPLATE,
    DRY_RUN,
    FIRST_LOGIN,
    "OWNERSHIP_MODE_ENABLED",
    "SENSITIVE_MAPPING_ENABLED",
    "HOURLY_DESTRUCTIVE_LIMIT",
    "HOURLY_GRANT_LIMIT",
    "HOURLY_FIRST_PASSWORD_LIMIT",
    "EMERGENCY_QUOTA",
    TIME_ZONE,
];

/// Kurumun saat dilimi; kayitta Postgres'in tanidigi da sorulur (web.rs).
pub const TIME_ZONE: &str = "TZ";

/// Ekrandaki bolumler; kayittan sonra o bolume donulur.
pub const SECTIONS: [&str; 4] = ["scope", "naming", "limits", "execution"];

/// Kaydedilecek bicim; hata metni alanin yaninda gosterilir.
pub fn validate(key: &str, raw: &str) -> Result<String, String> {
    match key {
        crate::change_set::THRESHOLD_KEY => {
            crate::change_set::parse_threshold(raw).map(|n| n.to_string())
        }
        USER_OUS | GROUP_OUS | PASSIVE_OU => {
            crate::scope::check_field(key, raw).map(|dns| dns.join("; "))
        }
        ZIMBRA_DOMAINS => domains(raw),
        USERNAME_TEMPLATE | EMAIL_LOCAL_TEMPLATE => template(raw),
        DRY_RUN | FIRST_LOGIN => boolean(key, raw),
        // Ortak yedi ayar: iki servisin okurken isletdigi kuralin aynisi (ikiz dosya)
        other if EDITABLE.contains(&other) => crate::common_settings::check(other, raw),
        other => Err(format!("{other} ekrandan değiştirilemez")),
    }
}

// Worker'in okudugu bicim (worker main.rs `parse_bool_setting`); kayitta tek yazima iner.
fn boolean(key: &str, raw: &str) -> Result<String, String> {
    match raw.trim() {
        "true" | "1" => Ok("true".to_string()),
        "false" | "0" => Ok("false".to_string()),
        other => Err(format!("{key} true ya da false olmalı, '{other}' geldi")),
    }
}

fn domains(raw: &str) -> Result<String, String> {
    let list: Vec<String> = raw
        .split(';')
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    let bad = list.iter().find(|d| {
        !d.contains('.')
            || d.starts_with(['.', '-'])
            || d.ends_with(['.', '-'])
            || !d
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    });
    match bad {
        Some(d) => Err(format!("alan adı değil: {d}")),
        None => Ok(list.join("; ")),
    }
}

/// Worker `username::render`'in yer tutuculari (ADR-011); ikiz degil, liste kisa
/// ve degisirse ADR'si yazilir. Yer tutucu disindaki metin a-z0-9 ve nokta olmali,
/// yoksa her uretim mudahaleye duserdi.
const PLACEHOLDERS: [&str; 5] = [
    "given",
    "given_first",
    "given_initial",
    "surname",
    "employee_number",
];

fn template(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    let mut rest = value;
    let mut placeholders = 0;
    while let Some(start) = rest.find('{') {
        literal(&rest[..start])?;
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("şablon bozuk, '}}' eksik: {value}"))?;
        let name = &rest[start + 1..start + end];
        if !PLACEHOLDERS.contains(&name) {
            return Err(format!("bilinmeyen yer tutucu: {{{name}}}"));
        }
        placeholders += 1;
        rest = &rest[start + end + 1..];
    }
    literal(rest)?;
    if placeholders == 0 {
        return Err("şablonda en az bir yer tutucu olmalı".to_string());
    }
    Ok(value.to_string())
}

fn literal(text: &str) -> Result<(), String> {
    match text
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '.'))
    {
        Some(c) => Err(format!("yer tutucu dışında izinli olmayan karakter: '{c}'")),
        None => Ok(()),
    }
}

/// Ekranin gordugu hal: kayitli (ya da reddedilen formdaki) degerler ve hatalar.
#[derive(Debug, Default, Clone)]
pub struct View {
    pub values: HashMap<String, String>,
    pub errors: HashMap<String, String>,
}

impl View {
    pub fn value(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    pub fn error(&self, key: &str) -> &str {
        self.errors.get(key).map_or("", String::as_str)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub key: &'static str,
    pub before: String,
    pub after: String,
}

/// Saf: formu dogrular ve kayitli degerlerle karsilastirir. Tek hata bile
/// varsa hicbir degisiklik donmez; formdaki ham degerler ekrana geri gider.
pub fn plan(
    current: &HashMap<String, String>,
    form: &HashMap<String, String>,
) -> Result<Vec<Change>, View> {
    let mut changes = Vec::new();
    let mut view = View {
        values: current.clone(),
        errors: HashMap::new(),
    };
    for key in EDITABLE {
        let Some(raw) = form.get(key) else { continue };
        view.values.insert(key.to_string(), raw.clone());
        match validate(key, raw) {
            Ok(after) => {
                let before = current.get(key).cloned().unwrap_or_default();
                if before != after {
                    changes.push(Change { key, before, after });
                }
            }
            Err(e) => {
                view.errors.insert(key.to_string(), e);
            }
        }
    }
    if view.errors.is_empty() {
        Ok(changes)
    } else {
        Err(view)
    }
}

pub async fn save(pool: &PgPool, changes: &[Change], by: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for c in changes {
        sqlx::query(
            "UPDATE operational_settings SET value = $2, updated_at = now(), updated_by = $3 \
             WHERE key = $1",
        )
        .bind(c.key)
        .bind(&c.after)
        .bind(by)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn plan_lists_only_changed_valid_values() {
        let current = map(&[("CHANGE_SET_THRESHOLD", "10")]);
        assert_eq!(
            plan(&current, &map(&[("CHANGE_SET_THRESHOLD", " 3 ")])).unwrap(),
            vec![Change {
                key: "CHANGE_SET_THRESHOLD",
                before: "10".into(),
                after: "3".into()
            }]
        );
        assert!(plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "10")]))
            .unwrap()
            .is_empty());
        // 0 gecerli: her duzenleme onaya duser (ADR-031)
        assert_eq!(
            plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "0")])).unwrap()[0].after,
            "0"
        );
        // Ekranin duzenlemedigi anahtar formdan gelse de yazilmaz
        assert!(plan(&current, &map(&[("AEAD_MASTER_KEY", "x")]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn scope_naming_and_execution_rules_hold_at_save_time() {
        assert_eq!(
            validate(USER_OUS, " OU=A,DC=x ;OU=B,DC=x;").unwrap(),
            "OU=A,DC=x; OU=B,DC=x"
        );
        assert!(validate(USER_OUS, "")
            .unwrap_err()
            .contains("tanımlı değil"));
        assert!(validate(GROUP_OUS, "CN=Users,DC=x").is_err());
        assert_eq!(validate(PASSIVE_OU, "").unwrap(), "");
        assert_eq!(
            validate(ZIMBRA_DOMAINS, "Okul.k12.tr; ").unwrap(),
            "okul.k12.tr"
        );
        assert_eq!(validate(ZIMBRA_DOMAINS, "").unwrap(), "");
        assert!(validate(ZIMBRA_DOMAINS, "okul").is_err());
        assert!(validate(ZIMBRA_DOMAINS, "ok ul.tr").is_err());
        assert!(validate(USERNAME_TEMPLATE, "{given_initial}{surname}").is_ok());
        assert!(validate(EMAIL_LOCAL_TEMPLATE, "{given}.{surname}.{employee_number}").is_ok());
        for bad in ["{name}.{surname}", "{given", "ali", "{given}-{surname}", ""] {
            assert!(validate(USERNAME_TEMPLATE, bad).is_err(), "{bad}");
        }
        assert_eq!(validate(DRY_RUN, "1").unwrap(), "true");
        assert_eq!(validate(FIRST_LOGIN, "false").unwrap(), "false");
        assert!(validate(DRY_RUN, "evet").is_err());
        assert_eq!(validate("HOURLY_DESTRUCTIVE_LIMIT", " 1 ").unwrap(), "1");
        assert!(validate("HOURLY_DESTRUCTIVE_LIMIT", "0").is_err());
        assert_eq!(validate("OWNERSHIP_MODE_ENABLED", "1").unwrap(), "true");
        assert_eq!(
            validate(TIME_ZONE, "Europe/Istanbul").unwrap(),
            "Europe/Istanbul"
        );
    }

    #[test]
    fn invalid_value_is_rejected_with_its_raw_text_kept() {
        let current = map(&[("CHANGE_SET_THRESHOLD", "10")]);
        let view = plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "on")])).unwrap_err();
        assert_eq!(view.value("CHANGE_SET_THRESHOLD"), "on");
        assert!(view.error("CHANGE_SET_THRESHOLD").contains("tam sayı"));
        assert!(plan(&current, &map(&[("CHANGE_SET_THRESHOLD", "-1")])).is_err());
    }
}
// --- END FEATURE: operational-settings ---
