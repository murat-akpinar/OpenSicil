// Ortak ayarlar (kurallar ADR-039, yeri ADR-131): backend ve worker ayni
// `operational_settings` satirlarini her is/istekte okur; yetkili worker'dir,
// backend ayni degeri onceden gosterir. backend/src/common_settings.rs ve worker/src/common_settings.rs
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
    // Saf: degiskenleri bir arama fonksiyonundan alir, testler ortami degistirmez.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        Ok(Self {
            ownership_mode_enabled: parse_bool(&lookup, "OWNERSHIP_MODE_ENABLED")?,
            hourly_destructive_limit: parse_limit(&lookup, "HOURLY_DESTRUCTIVE_LIMIT", LIMIT_MIN)?,
            hourly_grant_limit: parse_limit(&lookup, "HOURLY_GRANT_LIMIT", LIMIT_MIN)?,
            hourly_first_password_limit: parse_limit(
                &lookup,
                "HOURLY_FIRST_PASSWORD_LIMIT",
                LIMIT_MIN,
            )?,
            emergency_quota: parse_limit(&lookup, "EMERGENCY_QUOTA", QUOTA_MIN)?,
            sensitive_mapping_enabled: parse_bool(&lookup, "SENSITIVE_MAPPING_ENABLED")?,
            time_zone: parse_time_zone(&lookup)?,
        })
    }
}

/// Ayarlar ekrani kayitta ayni kurali cagirir (ADR-131 madde 5); kaydedilecek
/// bicimi doner. Worker cagirmaz, okurken `from_lookup` ayni kurali isletir.
#[allow(dead_code)]
pub fn check(name: &str, raw: &str) -> Result<String, String> {
    let lookup = |n: &str| (n == name).then(|| raw.to_string());
    match name {
        "OWNERSHIP_MODE_ENABLED" | "SENSITIVE_MAPPING_ENABLED" => {
            parse_bool(&lookup, name).map(|b| b.to_string())
        }
        "HOURLY_DESTRUCTIVE_LIMIT" | "HOURLY_GRANT_LIMIT" | "HOURLY_FIRST_PASSWORD_LIMIT" => {
            parse_limit(&lookup, name, LIMIT_MIN).map(|n| n.to_string())
        }
        "EMERGENCY_QUOTA" => parse_limit(&lookup, name, QUOTA_MIN).map(|n| n.to_string()),
        "TZ" => parse_time_zone(&lookup),
        other => Err(format!("{other} ortak ayar değil")),
    }
}

/// ADR-061 madde 3: binary'nin bekledigi en yuksek migration (`backend/migrations/NNNN_*`).
/// Veritabani bundan eskiyse servis acilmaz; backend testi sabiti dizinle karsilastirir.
pub const SCHEMA_VERSION: i64 = 36;

/// Gece taramasinin saatleri (ADR-124 kurallari, yeri ADR-131). Worker okur; backend
/// kayitta ayni kurali cagirdigi icin ikiz dosyada.
pub const SCAN_AT: &str = "RECONCILE_SCAN_AT";
const MAX_SCAN_TIMES: usize = 24;

/// Virgullu `HH:MM` listesi: en az bir, en cok 24 deger, yinelenen saat yok.
/// Bozuk deger SQL'e hic gitmez.
pub fn parse_scan_times(raw: &str) -> Result<Vec<String>, String> {
    let times: Vec<String> = raw.split(',').map(|t| t.trim().to_string()).collect();
    if let Some(bad) = times.iter().find(|t| !is_hh_mm(t)) {
        return Err(format!("{SCAN_AT} HH:MM listesi olmalı, '{bad}' geldi"));
    }
    if times.len() > MAX_SCAN_TIMES {
        return Err(format!("{SCAN_AT} en çok {MAX_SCAN_TIMES} saat alır"));
    }
    let mut sorted = times.clone();
    sorted.sort();
    sorted.dedup();
    if sorted.len() != times.len() {
        return Err(format!("{SCAN_AT} aynı saati iki kez içeriyor"));
    }
    Ok(times)
}

fn is_hh_mm(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && [0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit())
        && (b[0] - b'0') * 10 + (b[1] - b'0') < 24
        && b[3] < b'6'
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
        .ok_or_else(|| format!("ayar eksik: {name}"))
}

fn parse_bool(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Result<bool, String> {
    match required(lookup, name)?.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(format!("{name} true ya da false olmalı, '{other}' geldi")),
    }
}

const LIMIT_MIN: u32 = 1;
const QUOTA_MIN: u32 = 0;

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

    // 0034_operational_settings.sql'in seed'i; `seed_matches_defaults` ikisini karsilastirir.
    pub const SEED_DEFAULTS: [(&str, &str); 7] = [
        ("OWNERSHIP_MODE_ENABLED", "false"),
        ("HOURLY_DESTRUCTIVE_LIMIT", "50"),
        ("HOURLY_GRANT_LIMIT", "50"),
        ("HOURLY_FIRST_PASSWORD_LIMIT", "50"),
        ("EMERGENCY_QUOTA", "5"),
        ("SENSITIVE_MAPPING_ENABLED", "false"),
        ("TZ", "Europe/Istanbul"),
    ];

    fn env(overrides: &[(&str, &str)]) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = SEED_DEFAULTS
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
    fn parses_seed_defaults() {
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
    fn check_normalizes_one_value_with_the_same_rules() {
        assert_eq!(check("OWNERSHIP_MODE_ENABLED", " 1 ").unwrap(), "true");
        assert_eq!(check("HOURLY_GRANT_LIMIT", " 7").unwrap(), "7");
        assert_eq!(check("EMERGENCY_QUOTA", "0").unwrap(), "0");
        assert_eq!(check("TZ", " UTC ").unwrap(), "UTC");
        assert!(check("HOURLY_DESTRUCTIVE_LIMIT", "0").is_err());
        assert!(check("TZ", "Europe Istanbul").is_err());
        assert!(check("DRY_RUN", "true").is_err());
    }

    #[test]
    fn scan_times_are_hh_mm_lists_without_duplicates() {
        assert_eq!(parse_scan_times("02:00").unwrap(), ["02:00"]);
        assert_eq!(
            parse_scan_times("02:00, 10:00,18:00").unwrap(),
            ["02:00", "10:00", "18:00"]
        );
        for bad in [
            "9",
            "25:00",
            "02:0",
            "02:00,",
            "",
            "02:00,02:00",
            "12:60",
            "ab:cd",
        ] {
            assert!(parse_scan_times(bad).is_err(), "{bad}");
        }
        let hourly: Vec<String> = (0..24).map(|h| format!("{h:02}:00")).collect();
        assert_eq!(parse_scan_times(&hourly.join(",")).unwrap().len(), 24);
        let too_many = format!("{},23:30", hourly.join(","));
        assert!(parse_scan_times(&too_many).is_err());
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

    // ADR-131: migration'in seed'i bugunku varsayilanlarin aynisi; iki servis bu satiri okur.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn seed_matches_defaults() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let map = load_operational(&pool).await.unwrap();
        for (key, value) in SEED_DEFAULTS {
            assert_eq!(map.get(key).map(String::as_str), Some(value), "{key}");
        }
        let seeded = CommonSettings::from_lookup(|name| map.get(name).cloned()).unwrap();
        assert_eq!(seeded, parse(&[]).unwrap());
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
