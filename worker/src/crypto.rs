use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;

// ADR-069: chacha20poly1305, kimlik no + ilk parola + AD/Zimbra/OIDC sirlari icin tek AEAD.
// AEAD ana anahtari ve blind index anahtari ayni bicimdedir: base64, 32 bayt (ADR-010).
// Ham [u8; 32] dondurulur; GenericArray tipi bu modulun disina sizmaz.
pub fn parse_key(var_name: &str, base64_value: &str) -> Result<[u8; KEY_LEN], String> {
    let bytes = BASE64
        .decode(base64_value)
        .map_err(|e| format!("{var_name} base64 çözülemedi: {e}"))?;
    bytes
        .try_into()
        .map_err(|b: Vec<u8>| format!("{var_name} {KEY_LEN} bayt olmalı, {} bayt geldi", b.len()))
}

// Cikti: nonce (12 bayt) + sifreli metin, tek BYTEA sutununda saklanir.
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("chacha20poly1305 şifreleme başarısız olamaz");
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

// OIDC client secret'ı (settings::load_oidc_credentials) ve Faz 3'teki AD/Zimbra
// connector'ı bunu çözüp kullanır.
pub fn decrypt(key: &[u8; KEY_LEN], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < NONCE_LEN {
        return Err("şifreli veri çok kısa".to_string());
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce_array: [u8; NONCE_LEN] = nonce_bytes
        .try_into()
        .expect("split_at(NONCE_LEN) tam NONCE_LEN uzunlukta dilim üretir");
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .decrypt(&Nonce::from(nonce_array), ciphertext)
        .map_err(|_| "şifre çözme başarısız (yanlış anahtar ya da bozulmuş veri)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_from_byte(fill: u8) -> [u8; KEY_LEN] {
        parse_key("AEAD_MASTER_KEY", &BASE64.encode([fill; KEY_LEN])).unwrap()
    }

    fn test_key() -> [u8; KEY_LEN] {
        key_from_byte(0x42)
    }

    #[test]
    fn round_trips_plaintext() {
        let key = test_key();
        let ciphertext = encrypt(&key, b"gizli-servis-hesabi-parolasi");
        let plaintext = decrypt(&key, &ciphertext).unwrap();
        assert_eq!(plaintext, b"gizli-servis-hesabi-parolasi");
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        let key = test_key();
        let a = encrypt(&key, b"ayni-metin");
        let b = encrypt(&key, b"ayni-metin");
        assert_ne!(a, b);
    }

    #[test]
    fn decrypt_fails_with_wrong_key() {
        let ciphertext = encrypt(&test_key(), b"veri");
        let other_key = key_from_byte(0x99);
        assert!(decrypt(&other_key, &ciphertext).is_err());
    }

    #[test]
    fn decrypt_fails_on_truncated_data() {
        assert!(decrypt(&test_key(), b"kisa").is_err());
    }

    #[test]
    fn parse_key_rejects_wrong_length_and_names_the_variable() {
        let err = parse_key("BLIND_INDEX_KEY", &BASE64.encode(b"cok-kisa")).unwrap_err();
        assert!(err.starts_with("BLIND_INDEX_KEY"), "{err}");
    }

    #[test]
    fn parse_key_rejects_invalid_base64() {
        assert!(parse_key("AEAD_MASTER_KEY", "!!!not-base64!!!").is_err());
    }
}
