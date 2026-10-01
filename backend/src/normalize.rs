// ADR-011 normallestirme: kullanici adi (worker, `username.rs`) ve okunur adres
// (backend, ADR-107) ayni kurali okur. Ikiz dosya: backend/src/normalize.rs ile
// birebir ayni, biri degisince digeri de (ADR-070, `main.rs` testi karsilastirir).

use unicode_normalization::UnicodeNormalization;

/// Turkce kucuk harf (`I`/`İ`/`ı` → `i`, `çğöşü`), NFD ile aksan atma,
/// `a-z0-9` disindaki her sey atilir. Bos donebilir.
pub fn normalize_component(text: &str) -> String {
    let turkish: String = text
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
        .collect();
    turkish
        .nfd()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turkish_letters_accents_and_symbols_are_normalised() {
        assert_eq!(normalize_component("Işıl"), "isil");
        assert_eq!(normalize_component("İSMAİL"), "ismail");
        assert_eq!(normalize_component("Émile-Zoë"), "emilezoe");
        assert_eq!(normalize_component("Çağrı Öz-Şüt 7"), "cagriozsut7");
        assert_eq!(normalize_component("***"), "");
    }
}
