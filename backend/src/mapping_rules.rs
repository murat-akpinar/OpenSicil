// --- START FEATURE: attribute-mapping ---
// Oznitelik eslemesi kurallari (ADR-012, ADR-029, ADR-034, ADR-082): hedef basina
// kodda sabit izinli liste, kaynak anahtarlari (hassas isaretiyle) ve sabit
// donusumler. backend/src ve worker/src kopyalari birebir aynidir (ADR-070 ikiz
// dosya; backend main.rs testi karsilastirir). Ekran bu listeyi sunar, yetkili
// worker'dir: listede olmayan satir worker'da reddedilir.

pub const AD_ATTRIBUTES: &[&str] = &[
    "givenName",
    "sn",
    "initials",
    "displayName",
    "description",
    "mail",
    "department",
    "title",
    "manager",
    "company",
    "employeeID",
    "employeeNumber",
    "employeeType",
    "physicalDeliveryOfficeName",
    "telephoneNumber",
    "mobile",
    "streetAddress",
    "l",
    "st",
    "postalCode",
    "co",
    "c",
    "extensionAttribute1",
    "extensionAttribute2",
    "extensionAttribute3",
    "extensionAttribute4",
    "extensionAttribute5",
    "extensionAttribute6",
    "extensionAttribute7",
    "extensionAttribute8",
    "extensionAttribute9",
    "extensionAttribute10",
    "extensionAttribute11",
    "extensionAttribute12",
    "extensionAttribute13",
    "extensionAttribute14",
    "extensionAttribute15",
];

pub const ZIMBRA_ATTRIBUTES: &[&str] = &[
    "displayName",
    "givenName",
    "sn",
    "initials",
    "description",
    "company",
    "title",
    "telephoneNumber",
    "mobile",
    "street",
    "l",
    "st",
    "postalCode",
    "co",
    "zimbraNotes",
];

pub fn allowed_attributes(target_kind: &str) -> &'static [&'static str] {
    match target_kind {
        "ad" => AD_ATTRIBUTES,
        "zimbra" => ZIMBRA_ATTRIBUTES,
        _ => &[],
    }
}

pub fn attribute_allowed(target_kind: &str, attribute: &str) -> bool {
    allowed_attributes(target_kind).contains(&attribute)
}

/// (anahtar, ekran etiketi, hassas mi — ADR-012: varsayilan hicbir yere eslenmez)
pub const SOURCES: &[(&str, &str, bool)] = &[
    ("given_name", "Ad", false),
    ("surname", "Soyad", false),
    ("employee_number", "Sicil no", false),
    ("email", "E-posta", false),
    ("username", "Kullanıcı adı", false),
    ("upn", "UPN", false),
    ("department_name", "Departman adı", false),
    ("root_department_name", "Kök departman adı", false),
    ("title", "Birincil rol unvanı", false),
    ("manager_account", "Yöneticinin hedefteki hesabı", false),
    ("employment_type", "Çalışma tipi", false),
    ("start_date", "Başlangıç tarihi", false),
    ("end_date", "Bitiş tarihi", false),
    ("mobile_phone", "Cep telefonu", true),
    ("national_id", "Ulusal kimlik numarası", true),
    ("constant", "Sabit metin", false),
    ("template", "Şablon: {given} {surname} {employee_number} {department} {root_department} {title} {username} {email} {upn}", false),
];

/// None: bilinmeyen kaynak.
pub fn source_is_sensitive(key: &str) -> Option<bool> {
    SOURCES
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, _, sensitive)| *sensitive)
}

pub fn source_label(key: &str) -> &'static str {
    SOURCES
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, label, _)| *label)
        .unwrap_or("?")
}

/// (anahtar, ekran etiketi) — ADR-012 sabit donusum listesi.
pub const TRANSFORMS: &[(&str, &str)] = &[
    ("none", "yok"),
    ("lower", "küçük harf"),
    ("ascii", "ascii"),
    ("phone_e164", "telefon: E.164"),
    ("phone_e164_no_plus", "telefon: E.164, artı işaretsiz"),
    ("phone_national", "telefon: ulusal"),
    (
        "phone_international_spaced",
        "telefon: uluslararası boşluklu",
    ),
    ("date_iso", "tarih: ISO 8601"),
];

pub fn transform_known(key: &str) -> bool {
    TRANSFORMS.iter().any(|(k, _)| *k == key)
}

pub fn transform_label(key: &str) -> &'static str {
    TRANSFORMS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, label)| *label)
        .unwrap_or("?")
}

const TR_COUNTRY_CODE: &str = "+90";

/// Bilinmeyen donusum degeri degistirmez (worker satiri zaten reddeder).
pub fn apply_transform(key: &str, value: &str) -> String {
    let value = value.trim();
    match key {
        "lower" => value.to_lowercase(),
        "ascii" => ascii_fold(value),
        "phone_e164" => value.to_string(),
        "phone_e164_no_plus" => value.trim_start_matches('+').to_string(),
        "phone_national" => match value.strip_prefix(TR_COUNTRY_CODE) {
            Some(rest) => format!("0{rest}"),
            None => value.trim_start_matches('+').to_string(),
        },
        "phone_international_spaced" => spaced_phone(value),
        _ => value.to_string(),
    }
}

// ADR-011 normallestirmesinin ikiz dosyaya sigan hali: Turkce harfler eslenir,
// ASCII harf/rakam disindaki her sey (bosluk dahil) atilir, kucuk harf.
fn ascii_fold(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'I' | 'İ' | 'ı' => 'i',
            'Ç' | 'ç' => 'c',
            'Ğ' | 'ğ' => 'g',
            'Ö' | 'ö' => 'o',
            'Ş' | 'ş' => 's',
            'Ü' | 'ü' => 'u',
            other => other,
        })
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

// +90 532 123 45 67 (TR 10 hane: 3-3-2-2); diger ulke kodlari E.164 kalir.
fn spaced_phone(value: &str) -> String {
    let Some(rest) = value.strip_prefix(TR_COUNTRY_CODE) else {
        return value.to_string();
    };
    if rest.len() != 10 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return value.to_string();
    }
    format!(
        "{TR_COUNTRY_CODE} {} {} {} {}",
        &rest[..3],
        &rest[3..6],
        &rest[6..8],
        &rest[8..]
    )
}
// --- END FEATURE: attribute-mapping ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_excludes_identity_and_privilege_attributes() {
        for forbidden in [
            "sAMAccountName",
            "userPrincipalName",
            "cn",
            "userAccountControl",
            "pwdLastSet",
            "accountExpires",
            "unicodePwd",
            "member",
            "memberOf",
            "primaryGroupID",
        ] {
            assert!(!attribute_allowed("ad", forbidden), "{forbidden}");
        }
        assert!(attribute_allowed("ad", "extensionAttribute15"));
        assert!(attribute_allowed("zimbra", "zimbraNotes"));
        assert!(!attribute_allowed("zimbra", "zimbraIsAdminAccount"));
        assert!(!attribute_allowed("ldap", "cn"));
    }

    #[test]
    fn sources_mark_sensitive_and_unknown() {
        assert_eq!(source_is_sensitive("national_id"), Some(true));
        assert_eq!(source_is_sensitive("mobile_phone"), Some(true));
        assert_eq!(source_is_sensitive("given_name"), Some(false));
        assert_eq!(source_is_sensitive("password"), None);
        assert_eq!(source_label("title"), "Birincil rol unvanı");
    }

    #[test]
    fn transforms_follow_adr_012_table() {
        let phone = "+905321234567";
        assert_eq!(apply_transform("none", " Ahmet Yılmaz "), "Ahmet Yılmaz");
        assert_eq!(apply_transform("lower", "Ahmet Yılmaz"), "ahmet yılmaz");
        assert_eq!(apply_transform("ascii", "Ahmet Yılmaz"), "ahmetyilmaz");
        assert_eq!(apply_transform("ascii", "Şule İçöz"), "suleicoz");
        assert_eq!(apply_transform("phone_e164", phone), phone);
        assert_eq!(apply_transform("phone_e164_no_plus", phone), "905321234567");
        assert_eq!(apply_transform("phone_national", phone), "05321234567");
        assert_eq!(
            apply_transform("phone_national", "+4930123456"),
            "4930123456"
        );
        assert_eq!(
            apply_transform("phone_international_spaced", phone),
            "+90 532 123 45 67"
        );
        assert_eq!(
            apply_transform("phone_international_spaced", "+4930123456"),
            "+4930123456"
        );
        assert_eq!(apply_transform("date_iso", "2026-10-01"), "2026-10-01");
        assert!(transform_known("lower") && !transform_known("upper"));
        assert_eq!(transform_label("ascii"), "ascii");
    }
}
