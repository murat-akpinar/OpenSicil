// Ortak ayarlar (ADR-039): backend ve worker ayni .env degiskenlerini okur;
// etkin ayar tablosu yoktur, yetkili worker'dir, backend yalnizca ayni degeri
// onceden gosterir. backend/src/common_settings.rs ve worker/src/common_settings.rs
// birebir aynidir (ADR-070: bagimsiz crate'ler, paylasilan crate yok) — birini
// degistiren digerini de degistirir; backend main.rs testi ikisini karsilastirir.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonSettings {
    pub ownership_mode_enabled: bool,
    pub hourly_destructive_limit: u32,
    pub hourly_grant_limit: u32,
    pub hourly_first_password_limit: u32,
    pub emergency_quota: u32,
    pub sensitive_mapping_enabled: bool,
    pub time_zone: String,
}

impl CommonSettings {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    // Saf: degiskenleri bir arama fonksiyonundan alir, testler ortami degistirmez.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        Ok(Self {
            ownership_mode_enabled: parse_bool(&lookup, "OWNERSHIP_MODE_ENABLED")?,
            hourly_destructive_limit: parse_limit(&lookup, "HOURLY_DESTRUCTIVE_LIMIT", 1)?,
            hourly_grant_limit: parse_limit(&lookup, "HOURLY_GRANT_LIMIT", 1)?,
            hourly_first_password_limit: parse_limit(&lookup, "HOURLY_FIRST_PASSWORD_LIMIT", 1)?,
            emergency_quota: parse_limit(&lookup, "EMERGENCY_QUOTA", 0)?,
            sensitive_mapping_enabled: parse_bool(&lookup, "SENSITIVE_MAPPING_ENABLED")?,
            time_zone: parse_time_zone(&lookup)?,
        })
    }
}

/// ADR-131: isletme ayarlari tablosu `from_lookup` aramasina verilecek esleme
/// olarak okunur (`|name| map.get(name).cloned()`); onbellek yok, her is/istek okur.
pub async fn load_operational(pool: &sqlx::PgPool) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM operational_settings")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().collect())
}

// Acilis log'unda tek satir; iki servisin ciktisi yan yana konunca sapma gorulur.
impl fmt::Display for CommonSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sahiplenme={} yıkıcı/saat={} verme/saat={} ilk-parola/saat={} acil-kota={} \
             hassas-eşleme={} saat-dilimi={}",
            self.ownership_mode_enabled,
            self.hourly_destructive_limit,
            self.hourly_grant_limit,
            self.hourly_first_password_limit,
            self.emergency_quota,
            self.sensitive_mapping_enabled,
            self.time_zone
        )
    }
}

fn required(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Result<String, String> {
    lookup(name)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("ortam değişkeni eksik: {name}"))
}

fn parse_bool(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Result<bool, String> {
    match required(lookup, name)?.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(format!("{name} true ya da false olmalı, '{other}' geldi")),
    }
}

// Sayaç sınırı mutlak sayıdır (ADR-016); yıkıcı/verme/ilk parola için 0 her işi
// sonsuza kadar bekletir, o yüzden en az 1; acil kota 0 olabilir (kota yok).
fn parse_limit(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
    min: u32,
) -> Result<u32, String> {
    let raw = required(lookup, name)?;
    let value: u32 = raw
        .trim()
        .parse()
        .map_err(|_| format!("{name} tam sayı olmalı, '{raw}' geldi"))?;
    if value < min {
        return Err(format!("{name} en az {min} olmalı, {value} geldi"));
    }
    Ok(value)
}

// IANA adi bicimsel kontrol (Europe/Istanbul, UTC, Etc/GMT+3); tzdb dogrulamasi
// tarihleri yorumlayan modulun isi.
fn parse_time_zone(lookup: &impl Fn(&str) -> Option<String>) -> Result<String, String> {
    let value = required(lookup, "TZ")?.trim().to_string();
    let well_formed = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'+' | b'-'))
        && !value.starts_with('/')
        && !value.ends_with('/');
    if !well_formed {
        return Err(format!("TZ IANA saat dilimi adı olmalı, '{value}' geldi"));
    }
    Ok(value)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashMap;

    // .env.example'daki varsayilanlar; run() testleri ortami bununla kurar.
    pub const ENV_EXAMPLE_DEFAULTS: [(&str, &str); 7] = [
        ("OWNERSHIP_MODE_ENABLED", "false"),
        ("HOURLY_DESTRUCTIVE_LIMIT", "50"),
        ("HOURLY_GRANT_LIMIT", "50"),
        ("HOURLY_FIRST_PASSWORD_LIMIT", "50"),
        ("EMERGENCY_QUOTA", "5"),
        ("SENSITIVE_MAPPING_ENABLED", "false"),
        ("TZ", "Europe/Istanbul"),
    ];

    fn env(overrides: &[(&str, &str)]) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = ENV_EXAMPLE_DEFAULTS
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (k, v) in overrides {
            map.insert(k.to_string(), v.to_string());
        }
        map
    }

    fn parse(overrides: &[(&str, &str)]) -> Result<CommonSettings, String> {
        let map = env(overrides);
        CommonSettings::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn parses_env_example_defaults() {
        let s = parse(&[]).unwrap();
        assert_eq!(
            s,
            CommonSettings {
                ownership_mode_enabled: false,
                hourly_destructive_limit: 50,
                hourly_grant_limit: 50,
                hourly_first_password_limit: 50,
                emergency_quota: 5,
                sensitive_mapping_enabled: false,
                time_zone: "Europe/Istanbul".to_string(),
            }
        );
        assert!(s.to_string().contains("saat-dilimi=Europe/Istanbul"));
    }

    #[test]
    fn accepts_numeric_booleans_and_zero_emergency_quota() {
        let s = parse(&[("OWNERSHIP_MODE_ENABLED", "1"), ("EMERGENCY_QUOTA", "0")]).unwrap();
        assert!(s.ownership_mode_enabled);
        assert_eq!(s.emergency_quota, 0);
    }

    #[test]
    fn rejects_missing_zero_limit_bad_bool_and_bad_tz() {
        let cases: [(&str, &str, &str); 5] = [
            ("HOURLY_DESTRUCTIVE_LIMIT", "", "eksik"),
            ("HOURLY_GRANT_LIMIT", "0", "sıfır sınır"),
            ("HOURLY_FIRST_PASSWORD_LIMIT", "elli", "sayı değil"),
            ("SENSITIVE_MAPPING_ENABLED", "evet", "bool değil"),
            ("TZ", "Europe Istanbul", "boşluklu saat dilimi"),
        ];
        for (name, value, why) in cases {
            let err = parse(&[(name, value)]).expect_err(why);
            assert!(
                err.contains(name),
                "{why}: hata değişken adını söylemeli: {err}"
            );
        }
    }
}
