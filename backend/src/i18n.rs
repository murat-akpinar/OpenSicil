// --- START FEATURE: ui-i18n ---
// Arayuz dili (ADR-067 madde 1, ADR-088 madde 6, ADR-089): iki gomulu TOML ve
// sablonlarda `lang.t("anahtar")`. Yeni crate yok: dosyalar `key = "deger"`
// satirlarindan olusur, ayristirma asagida. Eksik ya da fazla anahtar testle
// yakalanir (iki dosyanin anahtar kumesi birebir ayni olmali).

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::http::{header, HeaderMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Tr = 0,
    En = 1,
}

pub const DEFAULT: Lang = Lang::Tr;

const TR_SOURCE: &str = include_str!("../../frontend/i18n/tr.toml");
const EN_SOURCE: &str = include_str!("../../frontend/i18n/en.toml");

impl Lang {
    pub fn from_code(code: &str) -> Lang {
        match code.trim() {
            "en" => Lang::En,
            _ => Lang::Tr,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Tr => "tr",
            Lang::En => "en",
        }
    }

    /// Dil secicisi tek dugme: obur dile gecirir, uzerinde obur dilin adi yazar.
    pub fn other_code(self) -> &'static str {
        match self {
            Lang::Tr => "en",
            Lang::En => "tr",
        }
    }

    pub fn other_label(self) -> &'static str {
        match self {
            Lang::Tr => "EN",
            Lang::En => "TR",
        }
    }

    /// Metin; anahtar yoksa anahtarin kendisi doner (ekranda goze carpar, test yakalar).
    pub fn t(self, key: &'static str) -> &'static str {
        self.lookup(key).unwrap_or(key)
    }

    /// Veritabani anahtarinin ekran karsiligi: `lang.key("state", "active")`.
    pub fn key(self, group: &str, name: &str) -> &'static str {
        self.lookup(&format!("{group}.{name}")).unwrap_or("?")
    }

    /// Metindeki `{}` yer tutucularini sirayla doldurur.
    pub fn tn(self, key: &'static str, args: &[&str]) -> String {
        let mut text = self.t(key).to_string();
        for arg in args {
            text = text.replacen("{}", arg, 1);
        }
        text
    }

    pub fn t1(self, key: &'static str, arg: impl std::fmt::Display) -> String {
        self.tn(key, &[&arg.to_string()])
    }

    /// Oturum yokken (giris, parola, Yapilandirma) dil tarayicidan gelir.
    pub fn from_headers(headers: &HeaderMap) -> Lang {
        Lang::from_accept_language(
            headers
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|v| v.to_str().ok()),
        )
    }

    // q degerleri yok sayilir: iki dil icin siralama yeterli.
    pub fn from_accept_language(value: Option<&str>) -> Lang {
        let header = value.unwrap_or_default().to_ascii_lowercase();
        for tag in header.split(',') {
            let tag = tag.split(';').next().unwrap_or("").trim();
            if tag.starts_with("en") {
                return Lang::En;
            }
            if tag.starts_with("tr") {
                return Lang::Tr;
            }
        }
        DEFAULT
    }

    fn lookup(self, key: &str) -> Option<&'static str> {
        static TABLES: OnceLock<[HashMap<&'static str, String>; 2]> = OnceLock::new();
        let tables = TABLES.get_or_init(|| [parse(TR_SOURCE), parse(EN_SOURCE)]);
        tables[self as usize].get(key).map(String::as_str)
    }
}

// Kullanilan TOML alt kumesi: `anahtar = "deger"` (ya da tek tirnakli, kacissiz),
// `#` yorum, bos satir. Bozuk satir atlanir; anahtar eksik kalir ve test yakalar.
fn parse(source: &'static str) -> HashMap<&'static str, String> {
    source.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &'static str) -> Option<(&'static str, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let value = value.trim();
    let text = match value.strip_prefix('\'') {
        Some(rest) => rest.strip_suffix('\'')?.to_string(),
        None => unescape(value.strip_prefix('"')?.strip_suffix('"')?),
    };
    Some((key.trim(), text))
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('n')) => {
                out.push('\n');
                chars.next();
            }
            ('\\', Some(next @ ('"' | '\\'))) => {
                out.push(next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}
// --- END FEATURE: ui-i18n ---

#[cfg(test)]
mod tests {
    use super::*;

    // Sablonlarda ve kodda gecen her anahtar iki dosyada da olmali; ayrica
    // `lang.key(grup, ad)` ile cozulen veritabani anahtarlari (grup uyeleri).
    const GROUPS: &[(&str, &[&str])] = &[
        (
            "state",
            &["pending", "active", "suspended", "departed", "deleted"],
        ),
        (
            "job",
            &[
                "queued",
                "running",
                "succeeded",
                "needs_intervention",
                "unknown",
            ],
        ),
        ("link", &["provisioned", "adopted", "managed", "observed"]),
        (
            "employment",
            &["permanent", "contract", "intern", "outsourced"],
        ),
        ("rolekind", &["base", "primary", "additional"]),
        ("usedkind", &["username", "email"]),
        (
            "upcomingkind",
            &["end", "role_end", "suspension_start", "suspension_return"],
        ),
    ];

    fn keys(lang: Lang) -> Vec<&'static str> {
        let source = match lang {
            Lang::Tr => TR_SOURCE,
            Lang::En => EN_SOURCE,
        };
        source
            .lines()
            .filter_map(parse_line)
            .map(|(k, _)| k)
            .collect()
    }

    #[test]
    fn both_files_have_the_same_keys() {
        let mut tr = keys(Lang::Tr);
        let mut en = keys(Lang::En);
        tr.sort_unstable();
        en.sort_unstable();
        let only_tr: Vec<_> = tr.iter().filter(|k| !en.contains(k)).collect();
        let only_en: Vec<_> = en.iter().filter(|k| !tr.contains(k)).collect();
        assert!(
            only_tr.is_empty() && only_en.is_empty(),
            "anahtar kümeleri ayrıştı: yalnızca tr={only_tr:?}, yalnızca en={only_en:?}"
        );
        assert!(tr.len() > 100, "tr.toml beklenenden kısa: {}", tr.len());
    }

    #[test]
    fn no_duplicate_keys() {
        for lang in [Lang::Tr, Lang::En] {
            let all = keys(lang);
            let mut unique = all.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(
                all.len(),
                unique.len(),
                "{}: tekrar eden anahtar",
                lang.code()
            );
        }
    }

    // Sablon ve kod taramasi: `.t("…")`, `.t1("…")`, `.tn("…")` cagrilarindaki
    // anahtarlar ile `mapping_rules` etiketleri iki dosyada da bulunmali.
    #[test]
    fn every_key_used_in_templates_and_code_exists() {
        let mut missing: Vec<String> = Vec::new();
        let mut found = 0;
        for (file, text) in sources() {
            for key in used_keys(&text) {
                found += 1;
                for lang in [Lang::Tr, Lang::En] {
                    if lang.lookup(&key).is_none() {
                        missing.push(format!("{file}: {key} ({})", lang.code()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "eksik anahtarlar: {missing:#?}");
        // Tarama bos donerse test ise yaramaz: sablonlardaki cagri sayisi beklenen mertebede mi
        assert!(found > 200, "tarama beklenenden az anahtar buldu: {found}");
    }

    #[test]
    fn mapping_rules_labels_are_known_keys() {
        for (_, label, _) in crate::mapping_rules::SOURCES {
            assert_ne!(Lang::En.t(label), *label, "kaynak etiketi eksik: {label}");
        }
        for (_, label) in crate::mapping_rules::TRANSFORMS {
            assert_ne!(Lang::En.t(label), *label, "dönüşüm etiketi eksik: {label}");
        }
    }

    #[test]
    fn database_key_groups_resolve() {
        for (group, names) in GROUPS {
            for name in *names {
                for lang in [Lang::Tr, Lang::En] {
                    assert_ne!(
                        lang.key(group, name),
                        "?",
                        "{}: {group}.{name} eksik",
                        lang.code()
                    );
                }
            }
        }
    }

    fn sources() -> Vec<(String, String)> {
        let mut out = Vec::new();
        // Sablonlar crate'in disinda, repo kokundeki frontend/ altinda (ADR-097).
        for dir in ["../frontend/templates", "src"] {
            let entries = std::fs::read_dir(dir).expect("dizin okunamadı");
            for entry in entries.flatten() {
                let path = entry.path();
                // i18n.rs'in kendi test anahtarlari taramaya girmez
                if path.file_name().is_some_and(|n| n == "i18n.rs") {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((path.display().to_string(), text));
                }
            }
        }
        out
    }

    fn used_keys(text: &str) -> Vec<String> {
        let mut keys = Vec::new();
        for call in [".t(\"", ".t1(\"", ".tn(\""] {
            let mut rest = text;
            while let Some(at) = rest.find(call) {
                rest = &rest[at + call.len()..];
                if let Some(end) = rest.find('"') {
                    keys.push(rest[..end].to_string());
                }
            }
        }
        keys
    }

    #[test]
    fn accept_language_picks_first_supported_tag() {
        assert_eq!(Lang::from_accept_language(Some("en-US,en;q=0.9")), Lang::En);
        assert_eq!(Lang::from_accept_language(Some("tr-TR,tr;q=0.9")), Lang::Tr);
        assert_eq!(Lang::from_accept_language(Some("de,en;q=0.7")), Lang::En);
        assert_eq!(Lang::from_accept_language(Some("de-DE")), DEFAULT);
        assert_eq!(Lang::from_accept_language(None), DEFAULT);
    }

    #[test]
    fn code_round_trips_and_toggles() {
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code(" tr "), Lang::Tr);
        assert_eq!(Lang::from_code("bogus"), DEFAULT);
        assert_eq!(Lang::Tr.other_code(), "en");
        assert_eq!(Lang::En.other_code(), "tr");
        assert_eq!(Lang::Tr.other_label(), "EN");
    }

    #[test]
    fn parser_reads_quotes_escapes_and_skips_junk() {
        assert_eq!(
            parse_line("a.b = \"x \\\"y\\\" z\""),
            Some(("a.b", "x \"y\" z".to_string()))
        );
        assert_eq!(
            parse_line("a.c = 'tek \"tırnak\" içinde'"),
            Some(("a.c", "tek \"tırnak\" içinde".to_string()))
        );
        assert_eq!(parse_line("# yorum"), None);
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("bozuk satır"), None);
    }

    #[test]
    fn placeholders_are_filled_in_order() {
        assert_eq!(
            Lang::Tr.tn("diff.mismatch", &["aktif", "bekliyor"]),
            "hedefte aktif, olması gereken bekliyor"
        );
        assert_eq!(
            Lang::En.tn("diff.mismatch", &["active", "pending"]),
            "target has active, expected pending"
        );
    }

    #[test]
    fn translations_differ_between_languages() {
        assert_eq!(Lang::Tr.t("nav.identities"), "Kimlikler");
        assert_eq!(Lang::En.t("nav.identities"), "Identities");
        assert_eq!(Lang::Tr.key("state", "active"), "aktif");
        assert_eq!(Lang::En.key("state", "active"), "active");
    }
}
