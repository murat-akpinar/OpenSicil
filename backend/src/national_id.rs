// --- START FEATURE: national-id ---
// Ulusal kimlik numarasi (ADR-010): dogrulama (TR kontrol haneleri), AEAD ile
// sifreleme (basinda anahtar surumu), HMAC-SHA256 blind index (tekillik ve
// arama), maskeli gosterim. Duz metin DB'ye, log'a, denetim kaydina girmez.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;

use crate::crypto::{self, KEY_LEN};

// ADR-010 "sifreli degerin basinda anahtar surumu": rotasyonda yeni surum
// eklenir, eski satirlar surum baytindan taninir. Bugun tek anahtar var.
const KEY_VERSION: u8 = 1;
const TR_LENGTH: usize = 11;
const OTHER_MAX_LENGTH: usize = 32;
const MASK_VISIBLE_EDGE: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NationalId {
    pub country: String,
    pub value: String,
}

// Girdi normalize edilir (bosluk atilir, buyuk harf); TR icin 11 hane, ilk hane
// 0 degil, 10. ve 11. haneler kontrol hanesi; digerleri en fazla 32 karakter
// A-Z 0-9 '-' ve kontrol hanesi dogrulanmaz (ADR-010).
pub fn parse(country: &str, raw: &str) -> Result<NationalId, String> {
    let country = country.trim().to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err("ülke kodu iki harf olmalı".to_string());
    }
    let value: String = raw
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match country.as_str() {
        "TR" => validate_tr(&value)?,
        _ => validate_generic(&value)?,
    }
    Ok(NationalId { country, value })
}

fn validate_tr(value: &str) -> Result<(), String> {
    let digits: Vec<u32> = value.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != TR_LENGTH || value.len() != TR_LENGTH {
        return Err("T.C. Kimlik No 11 haneli olmalı".to_string());
    }
    if digits[0] == 0 {
        return Err("T.C. Kimlik No 0 ile başlayamaz".to_string());
    }
    let odd_sum: u32 = [0, 2, 4, 6, 8].iter().map(|&i| digits[i]).sum();
    let even_sum: u32 = [1, 3, 5, 7].iter().map(|&i| digits[i]).sum();
    let tenth = (odd_sum * 7 + 10 * 10 - even_sum) % 10;
    let eleventh = (digits[..10].iter().sum::<u32>()) % 10;
    if digits[9] != tenth || digits[10] != eleventh {
        return Err("T.C. Kimlik No kontrol hanesi tutmuyor".to_string());
    }
    Ok(())
}

fn validate_generic(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > OTHER_MAX_LENGTH {
        return Err(format!(
            "kimlik numarası 1–{OTHER_MAX_LENGTH} karakter olmalı"
        ));
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("kimlik numarası yalnızca A-Z, 0-9 ve - içerebilir".to_string());
    }
    Ok(())
}

// Ulke + deger birlikte ozetlenir: farkli ulkelerin ayni numarasi carpismaz.
pub fn blind_index(key: &[u8; KEY_LEN], id: &NationalId) -> Vec<u8> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 her uzunlukta anahtar kabul eder");
    mac.update(id.country.as_bytes());
    mac.update(b"\0");
    mac.update(id.value.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub fn encrypt(aead_key: &[u8; KEY_LEN], id: &NationalId) -> Vec<u8> {
    let mut out = vec![KEY_VERSION];
    out.extend(crypto::encrypt(aead_key, id.value.as_bytes()));
    out
}

pub fn decrypt(aead_key: &[u8; KEY_LEN], data: &[u8]) -> Result<String, String> {
    match data.split_first() {
        Some((&KEY_VERSION, rest)) => {
            let bytes = crypto::decrypt(aead_key, rest)?;
            String::from_utf8(bytes).map_err(|_| "kimlik numarası UTF-8 değil".to_string())
        }
        Some((version, _)) => Err(format!("bilinmeyen anahtar sürümü: {version}")),
        None => Err("şifreli kimlik numarası boş".to_string()),
    }
}

// ADR-010: ekranda maskeli, ilk ve son iki hane acik (12*******34).
pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= MASK_VISIBLE_EDGE * 2 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..MASK_VISIBLE_EDGE].iter().collect();
    let tail: String = chars[chars.len() - MASK_VISIBLE_EDGE..].iter().collect();
    format!(
        "{head}{}{tail}",
        "*".repeat(chars.len() - MASK_VISIBLE_EDGE * 2)
    )
}

pub struct Keys<'a> {
    pub aead: &'a [u8; KEY_LEN],
    pub blind_index: &'a [u8; KEY_LEN],
}

// Tekillik DB'deki benzersiz indekstendir (national_id_bidx UNIQUE); mukerrer
// kayit sqlx::Error::Database olarak doner, cagiran operatore "zaten kayitli" der.
// Executor generic: kayit formu kimlik satiriyla ayni transaction'da yazar.
pub async fn store<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    keys: &Keys<'_>,
    identity_id: i64,
    id: &NationalId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE identities SET national_id_enc = $1, national_id_bidx = $2, national_id_country = $3 \
         WHERE id = $4",
    )
    .bind(encrypt(keys.aead, id))
    .bind(blind_index(keys.blind_index, id))
    .bind(&id.country)
    .bind(identity_id)
    .execute(pool)
    .await?;
    Ok(())
}

// Mukerrer kisi kontrolu ve "kimlik no ile ara": cozmeden, blind index ile.
pub async fn find_identity(
    pool: &PgPool,
    blind_index_key: &[u8; KEY_LEN],
    id: &NationalId,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM identities WHERE national_id_bidx = $1")
        .bind(blind_index(blind_index_key, id))
        .fetch_optional(pool)
        .await
}
// --- END FEATURE: national-id ---

#[cfg(test)]
mod tests {
    use super::*;

    // Bilinen gecerli test numarasi (NVI algoritmasiyla uretilmis).
    const VALID_TR: &str = "10000000146";

    fn key(fill: u8) -> [u8; KEY_LEN] {
        [fill; KEY_LEN]
    }

    #[test]
    fn tr_accepts_valid_and_normalizes_whitespace() {
        let id = parse("tr", " 100 000 001 46 ").unwrap();
        assert_eq!(id.country, "TR");
        assert_eq!(id.value, VALID_TR);
    }

    #[test]
    fn tr_rejects_bad_checksum_leading_zero_and_length() {
        assert!(parse("TR", "10000000147").is_err(), "kontrol hanesi");
        assert!(parse("TR", "00000000146").is_err(), "sıfırla başlama");
        assert!(parse("TR", "1000000014").is_err(), "uzunluk");
        assert!(parse("TR", "1000000014a").is_err(), "harf");
    }

    #[test]
    fn other_country_uppercases_and_limits_charset() {
        assert_eq!(parse("de", "ab-12").unwrap().value, "AB-12");
        assert!(parse("DE", "ab_12").is_err(), "alt çizgi");
        assert!(parse("DE", &"A".repeat(33)).is_err(), "32'den uzun");
        assert!(parse("DE", "").is_err(), "boş");
        assert!(parse("DEU", "1").is_err(), "üç harfli ülke");
    }

    #[test]
    fn blind_index_is_deterministic_and_key_and_country_dependent() {
        let tr = parse("TR", VALID_TR).unwrap();
        let same = parse("TR", VALID_TR).unwrap();
        let other_country = NationalId {
            country: "DE".to_string(),
            value: VALID_TR.to_string(),
        };
        assert_eq!(blind_index(&key(1), &tr), blind_index(&key(1), &same));
        assert_ne!(blind_index(&key(1), &tr), blind_index(&key(2), &tr));
        assert_ne!(
            blind_index(&key(1), &tr),
            blind_index(&key(1), &other_country)
        );
    }

    #[test]
    fn encrypt_round_trips_with_version_byte() {
        let id = parse("TR", VALID_TR).unwrap();
        let data = encrypt(&key(7), &id);
        assert_eq!(data[0], KEY_VERSION);
        assert_eq!(decrypt(&key(7), &data).unwrap(), VALID_TR);
        assert!(decrypt(&key(8), &data).is_err(), "yanlış anahtar");
        assert!(decrypt(&key(7), &[9, 0, 0]).is_err(), "bilinmeyen sürüm");
        assert!(decrypt(&key(7), &[]).is_err(), "boş");
    }

    #[test]
    fn mask_shows_only_edges() {
        assert_eq!(mask(VALID_TR), "10*******46");
        assert_eq!(mask("1234"), "****");
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn store_enforces_uniqueness_and_find_uses_blind_index() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        let keys = Keys {
            aead: &key(1),
            blind_index: &key(2),
        };
        let id = parse("TR", VALID_TR).unwrap();

        store(&pool, &keys, ids[0], &id)
            .await
            .expect("kaydedilemedi");
        assert_eq!(
            find_identity(&pool, keys.blind_index, &id).await.unwrap(),
            Some(ids[0])
        );
        assert!(
            store(&pool, &keys, ids[1], &id).await.is_err(),
            "aynı numara ikinci kimliğe yazılamamalı"
        );

        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT national_id_enc FROM identities WHERE id = $1")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            !stored
                .windows(VALID_TR.len())
                .any(|w| w == VALID_TR.as_bytes()),
            "düz metin DB'de olmamalı"
        );
        assert_eq!(decrypt(keys.aead, &stored).unwrap(), VALID_TR);

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
