// Yonetilen kapsam (ADR-014, ADR-077): worker kapsamin disina yazmaz, katalog buradan
// dolar. ADR-131: degerler isletme ayarlari tablosunda; backend kayitta ayni kurali
// cagirir, bozuk kapsam tabloya girmez. backend/src/scope.rs ve worker/src/scope.rs
// birebir aynidir (ADR-070) — backend main.rs testi ikisini karsilastirir.

pub const USER_OUS: &str = "AD_MANAGED_USER_OUS";
pub const GROUP_OUS: &str = "AD_MANAGED_GROUP_OUS";
pub const PASSIVE_OU: &str = "AD_PASSIVE_OU";

const FORBIDDEN_CONTAINERS: [&str; 3] = ["cn=users,", "cn=builtin,", "ou=domain controllers,"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedScope {
    pub user_ous: Vec<String>,
    pub passive_ou: Option<String>,
    pub group_ous: Vec<String>,
}

// DN listeleri noktali virgulle ayrilir (DN'in kendisi virgul icerir).
fn split_dns(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Tek alanin kurali: CN=Users, CN=Builtin, OU=Domain Controllers kapsam olamaz
/// (docs/05); kullanici ve grup OU listesi bos olamaz, pasif OU bos olabilir.
pub fn check_field(name: &str, value: &str) -> Result<Vec<String>, String> {
    let dns = split_dns(value);
    if dns.is_empty() && name != PASSIVE_OU {
        return Err(format!(
            "yönetilen kapsam Ayarlar ekranında tanımlı değil: {name}"
        ));
    }
    if name == PASSIVE_OU && dns.len() > 1 {
        return Err(format!("{name} tek DN olmalı"));
    }
    match dns.iter().find(|dn| {
        let lower = dn.to_ascii_lowercase();
        FORBIDDEN_CONTAINERS.iter().any(|c| lower.starts_with(c))
    }) {
        Some(dn) => Err(format!("kapsam olamaz: {dn} (docs/05)")),
        None => Ok(dns),
    }
}

/// Kapsam bozuksa connector baslamaz, is sessizce gecmez.
pub fn parse_scope(lookup: impl Fn(&str) -> Option<String>) -> Result<ManagedScope, String> {
    let get = |name: &str| check_field(name, &lookup(name).unwrap_or_default());
    Ok(ManagedScope {
        user_ous: get(USER_OUS)?,
        passive_ou: get(PASSIVE_OU)?.into_iter().next(),
        group_ous: get(GROUP_OUS)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_rejects_builtin_containers_and_requires_both_lists() {
        let env = |user: &'static str, group: &'static str| {
            move |name: &str| match name {
                USER_OUS => Some(user.to_string()),
                GROUP_OUS => Some(group.to_string()),
                PASSIVE_OU => Some("OU=Pasif,OU=Personel,DC=x".to_string()),
                _ => None,
            }
        };
        let ok = parse_scope(env("OU=Personel,DC=x", "OU=Gruplar,DC=x;OU=Gruplar2,DC=x")).unwrap();
        assert_eq!(ok.group_ous.len(), 2);
        assert_eq!(ok.passive_ou.as_deref(), Some("OU=Pasif,OU=Personel,DC=x"));
        assert!(parse_scope(env("OU=Personel,DC=x", "CN=Users,DC=x")).is_err());
        let empty = parse_scope(env("", "OU=Gruplar,DC=x")).unwrap_err();
        assert!(empty.contains("Ayarlar ekranında tanımlı değil"), "{empty}");
    }

    #[test]
    fn field_rule_allows_empty_passive_ou_but_not_two() {
        assert_eq!(check_field(PASSIVE_OU, " ").unwrap(), Vec::<String>::new());
        assert!(check_field(PASSIVE_OU, "OU=A,DC=x;OU=B,DC=x").is_err());
        assert!(check_field(PASSIVE_OU, "ou=Domain Controllers,DC=x").is_err());
        assert!(check_field(USER_OUS, ";;").is_err());
    }
}
