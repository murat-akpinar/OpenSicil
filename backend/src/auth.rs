use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

// security.md: parola argon2id (RustCrypto ekosistemi, chacha20poly1305/hmac ile ayni aile).
pub fn hash_password(plain: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("parola hash'lenemedi: {e}"))
}

pub fn verify_password(plain: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_correct_password() {
        let hash = hash_password("admin").unwrap();
        assert!(verify_password("admin", &hash));
    }

    #[test]
    fn rejects_wrong_password() {
        let hash = hash_password("admin").unwrap();
        assert!(!verify_password("baska-parola", &hash));
    }

    #[test]
    fn rejects_garbage_hash_without_panicking() {
        assert!(!verify_password("admin", "bu-bir-argon2-hash-degil"));
    }

    #[test]
    fn same_password_hashes_differently_each_time() {
        assert_ne!(
            hash_password("admin").unwrap(),
            hash_password("admin").unwrap()
        );
    }
}
