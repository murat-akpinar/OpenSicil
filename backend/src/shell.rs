// --- START FEATURE: ui-shell ---
// Kabugun her sayfada tasidigi baglam (ADR-096 madde 2): ust bardaki kullanici
// cipi (bas harf avatari + ad + yetki), kenar cubugunun surum satiri ve arama
// kutusunun mevcut sorgusu. Tek struct ve tek yapim noktasi (`Shell::of`), boylece
// base.html'e eklenen her yeni kabuk alani 13 sablon struct'ini degil yalnizca
// burayi buyutur.

use crate::i18n::Lang;
use crate::operator_session::Operator;

/// Cipte yazilacak yetki, en genisten en dara. Operatorde birden fazlasi varsa
/// listedeki ilki gosterilir: "Sistem yoneticisi" olan birine "Denetci" yazmak
/// yanlis olurdu.
const AUTHORITY_ORDER: [&str; 6] = [
    "admin",
    "role_admin",
    "hr",
    "helpdesk",
    "pii_reader",
    "auditor",
];

pub struct Shell {
    pub username: String,
    /// Avatar yazisi: adin ilk iki parcasinin bas harfleri
    pub initials: String,
    pub version: &'static str,
    /// Arama kutusunun doldurulmus hali; arama disindaki sayfalarda bos
    pub query: String,
    /// Kenar cubugundaki "Ayarlar" yalnizca `admin`e gorunur: sayfanin kendisi
    /// zaten reddediyor, menude cikmasi yetkisiz operatore 403 vaat etmek olurdu
    pub is_admin: bool,
    authority: Option<String>,
}

impl Shell {
    pub fn of(operator: &Operator) -> Shell {
        Shell::from_parts(&operator.username, &operator.authorities)
    }

    pub fn from_parts(username: &str, authorities: &[String]) -> Shell {
        Shell {
            username: username.to_string(),
            initials: initials(username),
            version: env!("CARGO_PKG_VERSION"),
            query: String::new(),
            is_admin: authorities
                .iter()
                .any(|a| a == crate::oidc::ADMIN_AUTHORITY),
            authority: top_authority(authorities),
        }
    }

    /// Arama sonucu sayfasi kutuyu dolu gosterir; kullanici sorgusunu kaybetmez.
    pub fn with_query(mut self, query: String) -> Shell {
        self.query = query;
        self
    }

    /// Sablon `shell.authority(lang)` ile cagirir; yetkisiz operator (ADR-095'te
    /// mumkun: yonetim grubunda olmayan AD kullanicisi) icin bos doner.
    pub fn authority(&self, lang: &Lang) -> &'static str {
        match &self.authority {
            Some(a) => lang.key("authority", a),
            None => "",
        }
    }
}

fn top_authority(authorities: &[String]) -> Option<String> {
    AUTHORITY_ORDER
        .iter()
        .find(|wanted| authorities.iter().any(|a| a == *wanted))
        .map(|a| (*a).to_string())
        // Listede olmayan bir yetki gelirse (ileride eklenen) yine de goster
        .or_else(|| authorities.first().cloned())
}

/// Bas harfler: "Murat Akpinar" -> "MA", tek parcali ad -> tek harf.
/// Unicode: `chars()` ile ilk karakter alinir, byte dilimlemesi cok baytli
/// harfte (Ş, Ö, Ç) paniklerdi.
pub fn initials(username: &str) -> String {
    username
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .flat_map(|c| c.to_uppercase())
        .collect()
}
// --- END FEATURE: ui-shell ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_letter_of_the_first_two_parts() {
        assert_eq!(initials("Murat Akpinar"), "MA");
        assert_eq!(initials("ayse"), "A");
        assert_eq!(initials("Ayse Nur Sahin"), "AN");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn initials_handle_multibyte_letters() {
        // Byte dilimlemesi burada panik ederdi
        assert_eq!(initials("şule çelik"), "ŞÇ");
    }

    #[test]
    fn the_widest_authority_wins() {
        let all = vec!["auditor".to_string(), "admin".to_string()];
        assert_eq!(top_authority(&all), Some("admin".to_string()));
        assert_eq!(
            top_authority(&["helpdesk".to_string()]),
            Some("helpdesk".to_string())
        );
        assert_eq!(top_authority(&[]), None);
    }

    #[test]
    fn an_unknown_authority_is_still_shown() {
        let unknown = vec!["future_role".to_string()];
        assert_eq!(top_authority(&unknown), Some("future_role".to_string()));
    }

    #[test]
    fn the_chip_label_comes_from_i18n_and_is_empty_without_authority() {
        let operator_authorities = vec!["hr".to_string()];
        let shell = Shell::from_parts("Murat Akpinar", &operator_authorities);
        assert_eq!(shell.authority(&Lang::Tr), "İnsan kaynakları");
        assert_eq!(shell.authority(&Lang::En), "Human resources");
        assert_eq!(shell.initials, "MA");

        let none = Shell::from_parts("Murat Akpinar", &[]);
        assert_eq!(none.authority(&Lang::Tr), "");
    }

    #[test]
    fn only_admin_sees_the_settings_link() {
        // Yapilandirma sayfasi `admin` disindakini 403 ile reddediyor; menude
        // gostermek olmayan bir kapiyi isaret etmek olurdu.
        assert!(Shell::from_parts("a", &["admin".to_string()]).is_admin);
        assert!(!Shell::from_parts("a", &["role_admin".to_string()]).is_admin);
        assert!(!Shell::from_parts("a", &["hr".to_string(), "auditor".to_string()]).is_admin);
        assert!(!Shell::from_parts("a", &[]).is_admin);
    }
}
