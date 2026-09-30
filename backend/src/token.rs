// Oturum token yardimcilari; bootstrap_sessions ve operator_sessions ikisi de kullanir.

use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use base64::Engine;

// DB'de token'in kendisi degil hash'i tutulur: veritabani sizarsa cerezler tek basina ise yaramaz.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    BASE64_URL.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_is_deterministic_and_distinct() {
        assert_eq!(hash_token("ayni"), hash_token("ayni"));
        assert_ne!(hash_token("bir"), hash_token("iki"));
    }

    #[test]
    fn generate_token_is_not_repeated() {
        assert_ne!(generate_token(), generate_token());
    }
}
