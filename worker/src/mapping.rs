// --- START FEATURE: attribute-mapping ---
// Oznitelik eslemesinin worker tarafi (ADR-012/029/034/040/082): satirlari oku,
// izinli liste ve hassas kaynak kapisiyla dogrula (ihlal = mudahale), kaynaklari
// degerlendir, hesap acilisi icin oznitelikleri ve mevcut hesap icin farki uret.

use std::collections::HashMap;

use sqlx::PgPool;

use crate::mapping_rules;

// national_id_enc: backend national_id::encrypt bicimi — ilk bayt anahtar surumu.
const NATIONAL_ID_KEY_VERSION: u8 = 1;

#[derive(Debug, Clone)]
pub struct MappingRow {
    pub attribute: String,
    pub source_kind: String,
    pub source_text: Option<String>,
    pub transform: String,
    pub write_if_empty: bool,
}

pub async fn load_rows(pool: &PgPool, target: i64) -> Result<Vec<MappingRow>, String> {
    let rows: Vec<(String, String, Option<String>, String, bool)> = sqlx::query_as(
        "SELECT target_attribute, source_kind, source_text, transform, write_if_empty \
         FROM attribute_mappings WHERE target_system_id = $1 ORDER BY target_attribute",
    )
    .bind(target)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("eşleme satırları okunamadı: {e}"))?;
    Ok(rows
        .into_iter()
        .map(
            |(attribute, source_kind, source_text, transform, write_if_empty)| MappingRow {
                attribute,
                source_kind,
                source_text,
                transform,
                write_if_empty,
            },
        )
        .collect())
}

// ADR-029: yetkili worker'dir. Backend ne yazmis olsa da burada durur.
pub fn validate(
    rows: &[MappingRow],
    target_kind: &str,
    sensitive_enabled: bool,
) -> Result<(), String> {
    for row in rows {
        if !mapping_rules::attribute_allowed(target_kind, &row.attribute) {
            return Err(format!(
                "eşleme satırı reddedildi: {} izinli listede değil (ADR-029)",
                row.attribute
            ));
        }
        match mapping_rules::source_is_sensitive(&row.source_kind) {
            None => {
                return Err(format!(
                    "eşleme satırı reddedildi: bilinmeyen kaynak {}",
                    row.source_kind
                ))
            }
            Some(true) if !sensitive_enabled => {
                return Err(format!(
                    "eşleme satırı reddedildi: {} hassas kaynak, SENSITIVE_MAPPING_ENABLED kapalı (ADR-029)",
                    row.attribute
                ))
            }
            Some(_) => {}
        }
        if !mapping_rules::transform_known(&row.transform) {
            return Err(format!(
                "eşleme satırı reddedildi: bilinmeyen dönüşüm {}",
                row.transform
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Set(String),
    /// Kaynak bos: hedef temizlenir (ADR-012)
    Clear,
    /// Hesaplanamadi (yonetici hedefte yok): dokunulmaz (ADR-040)
    Undetermined,
}

/// Kaynak degerleri; anahtarlar mapping_rules::SOURCES ile ayni.
#[derive(Debug, Default)]
pub struct Sources {
    pub values: HashMap<&'static str, String>,
    /// None: etkin yonetici yok (Clear); Some(None): belirsiz; Some(Some(dn)): deger
    pub manager_dn: Option<Option<String>>,
}

const TEMPLATE_TOKENS: &[(&str, &str)] = &[
    ("{given}", "given_name"),
    ("{surname}", "surname"),
    ("{employee_number}", "employee_number"),
    ("{department}", "department_name"),
    ("{root_department}", "root_department_name"),
    ("{title}", "title"),
    ("{username}", "username"),
    ("{email}", "email"),
    ("{upn}", "upn"),
];

fn render_template(template: &str, values: &HashMap<&'static str, String>) -> String {
    let mut out = template.to_string();
    for (token, key) in TEMPLATE_TOKENS {
        out = out.replace(token, values.get(key).map(String::as_str).unwrap_or(""));
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn evaluate(row: &MappingRow, s: &Sources) -> Value {
    let raw = match row.source_kind.as_str() {
        "constant" => row.source_text.clone().unwrap_or_default(),
        "template" => render_template(row.source_text.as_deref().unwrap_or(""), &s.values),
        "manager_account" => {
            return match &s.manager_dn {
                None => Value::Clear,
                Some(None) => Value::Undetermined,
                Some(Some(dn)) => Value::Set(dn.clone()),
            }
        }
        key => s.values.get(key).cloned().unwrap_or_default(),
    };
    let value = mapping_rules::apply_transform(&row.transform, &raw);
    if value.is_empty() {
        Value::Clear
    } else {
        Value::Set(value)
    }
}

/// Hesap acilisi: yalnizca dolu degerler tek `add`'e girer.
pub fn initial_attributes(rows: &[MappingRow], s: &Sources) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|row| match evaluate(row, s) {
            Value::Set(v) => Some((row.attribute.clone(), v)),
            _ => None,
        })
        .collect()
}

/// Mevcut hesap: (oznitelik, Some(yeni) | None = sil). ADR-034 "sadece bossa
/// yaz": hedef doluysa korunur, sapma sayilmaz.
pub fn changes(
    rows: &[MappingRow],
    s: &Sources,
    current: &HashMap<String, Vec<String>>,
) -> Vec<(String, Option<String>)> {
    rows.iter()
        .filter_map(|row| {
            let existing = current
                .get(&row.attribute)
                .and_then(|v| v.first())
                .filter(|v| !v.is_empty());
            match evaluate(row, s) {
                Value::Undetermined => None,
                _ if row.write_if_empty && existing.is_some() => None,
                Value::Set(v) if existing != Some(&v) => Some((row.attribute.clone(), Some(v))),
                Value::Clear if existing.is_some() => Some((row.attribute.clone(), None)),
                _ => None,
            }
        })
        .collect()
}

// Kimlik no yalnizca eslenmisse ve ayar acikken cozulur (ADR-010/029).
pub fn decrypt_national_id(key: &[u8; crate::crypto::KEY_LEN], enc: &[u8]) -> Option<String> {
    match enc.split_first() {
        Some((&NATIONAL_ID_KEY_VERSION, rest)) => crate::crypto::decrypt(key, rest)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok()),
        _ => None,
    }
}
// --- END FEATURE: attribute-mapping ---

#[cfg(test)]
mod tests {
    use super::*;

    fn row(attribute: &str, source_kind: &str, text: Option<&str>) -> MappingRow {
        MappingRow {
            attribute: attribute.to_string(),
            source_kind: source_kind.to_string(),
            source_text: text.map(str::to_string),
            transform: "none".to_string(),
            write_if_empty: false,
        }
    }

    fn sources() -> Sources {
        let mut values = HashMap::new();
        values.insert("given_name", "Ayşe".to_string());
        values.insert("surname", "Yılmaz".to_string());
        values.insert("title", String::new());
        values.insert("mobile_phone", "+905321234567".to_string());
        Sources {
            values,
            manager_dn: Some(None),
        }
    }

    #[test]
    fn validate_enforces_allowlist_sources_transforms_and_sensitive_gate() {
        let ok = [row("givenName", "given_name", None)];
        assert!(validate(&ok, "ad", false).is_ok());
        let bad_attr = [row("userAccountControl", "constant", Some("512"))];
        assert!(validate(&bad_attr, "ad", true)
            .unwrap_err()
            .contains("izinli listede değil"));
        let bad_source = [row("description", "password", None)];
        assert!(validate(&bad_source, "ad", true)
            .unwrap_err()
            .contains("bilinmeyen kaynak"));
        let sensitive = [row("mobile", "mobile_phone", None)];
        assert!(validate(&sensitive, "ad", false)
            .unwrap_err()
            .contains("hassas"));
        assert!(validate(&sensitive, "ad", true).is_ok());
        let mut bad_transform = row("description", "constant", Some("x"));
        bad_transform.transform = "upper".to_string();
        assert!(validate(&[bad_transform], "ad", true)
            .unwrap_err()
            .contains("bilinmeyen dönüşüm"));
    }

    #[test]
    fn evaluate_handles_template_constant_empty_and_manager() {
        let s = sources();
        assert_eq!(
            evaluate(
                &row("displayName", "template", Some("{given}  {surname}")),
                &s
            ),
            Value::Set("Ayşe Yılmaz".to_string())
        );
        assert_eq!(
            evaluate(&row("description", "constant", Some(" Personel ")), &s),
            Value::Set("Personel".to_string())
        );
        assert_eq!(evaluate(&row("title", "title", None), &s), Value::Clear);
        assert_eq!(
            evaluate(&row("manager", "manager_account", None), &s),
            Value::Undetermined
        );
        let mut phone = row("mobile", "mobile_phone", None);
        phone.transform = "phone_national".to_string();
        assert_eq!(evaluate(&phone, &s), Value::Set("05321234567".to_string()));
    }

    #[test]
    fn changes_respect_write_if_empty_and_skip_undetermined() {
        let s = sources();
        let mut office = row("physicalDeliveryOfficeName", "constant", Some("Ankara"));
        office.write_if_empty = true;
        let rows = [
            row("givenName", "given_name", None),
            row("title", "title", None),
            row("manager", "manager_account", None),
            office,
        ];
        let current: HashMap<String, Vec<String>> = HashMap::from([
            ("givenName".to_string(), vec!["Ayse".to_string()]),
            ("title".to_string(), vec!["Eski Unvan".to_string()]),
            ("manager".to_string(), vec!["CN=Eski".to_string()]),
            (
                "physicalDeliveryOfficeName".to_string(),
                vec!["Elle".to_string()],
            ),
        ]);
        let diff = changes(&rows, &s, &current);
        assert_eq!(
            diff,
            vec![
                ("givenName".to_string(), Some("Ayşe".to_string())),
                ("title".to_string(), None),
            ],
            "yönetici belirsiz: dokunulmaz; ofis dolu + sadece boşsa yaz: korunur"
        );
        // ADR-034: "sadece bossa yaz" bos degeri doldurur; doluyken fark listesine hic
        // girmez — gozlem farki ve yazma ayni listeden, sapma raporlanmaz
        let mut empty_office = current.clone();
        empty_office.remove("physicalDeliveryOfficeName");
        assert!(changes(&rows, &s, &empty_office).contains(&(
            "physicalDeliveryOfficeName".to_string(),
            Some("Ankara".to_string())
        )));
        assert_eq!(
            initial_attributes(&rows, &s),
            vec![
                ("givenName".to_string(), "Ayşe".to_string()),
                (
                    "physicalDeliveryOfficeName".to_string(),
                    "Ankara".to_string()
                ),
            ]
        );
    }

    #[test]
    fn national_id_requires_known_version_byte() {
        let key = [5u8; crate::crypto::KEY_LEN];
        let mut enc = vec![NATIONAL_ID_KEY_VERSION];
        enc.extend(crate::crypto::encrypt(&key, b"10000000146"));
        assert_eq!(
            decrypt_national_id(&key, &enc).as_deref(),
            Some("10000000146")
        );
        assert!(decrypt_national_id(&key, &[9, 1, 2]).is_none());
        assert!(decrypt_national_id(&[6u8; crate::crypto::KEY_LEN], &enc).is_none());
    }
}
