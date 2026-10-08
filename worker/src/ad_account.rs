// --- START FEATURE: ad-provisioning ---
// AD hesap islemleri (docs/05 Hesap acma, CN kurali, Etkinlestirme/pasiflestirme,
// Grup uyeligi; ADR-009, 012, 032, 057): hesap TEK `add` ile acilir (oznitelikler,
// unicodePwd, UAC 514, pwdLastSet 0, biliniyorsa accountExpires) — AD parolayi
// reddederse hesap hic olusmaz; objectGUID hemen okunur. Nesnelere GUID ile
// erisilir, yazma GUID'den cozulen gercek DN'e yapilir. Uyelik grubun `member`
// ozniteligiyle degistirilir. Parola kimseye gosterilmez (ADR-009).

use std::collections::{HashMap, HashSet};

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::OsRng;
use ldap3::{dn_escape, ldap_escape, Ldap, Mod, Scope};

use crate::ad;
use crate::writes::{TargetWriter, WriteError, WriteOp};

pub const UAC_ACCOUNT_DISABLE: u32 = 0x0002;
pub const UAC_NORMAL_ACCOUNT: u32 = 0x0200;
const CN_MAX_LEN: usize = 64;
const PASSWORD_LEN: usize = 20;
// 1601-01-01 ile 1970-01-01 arasi saniye; accountExpires 100 ns birimli FILETIME
const FILETIME_EPOCH_OFFSET: i64 = 11_644_473_600;
const FILETIME_PER_SECOND: i64 = 10_000_000;
const READABLE_PASSWORD_LEN: usize = 16;
const READABLE_GROUP_LEN: usize = 4;
const READABLE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
const PASSWORD_CLASSES: [&[u8]; 4] = [
    b"ABCDEFGHJKLMNPQRSTUVWXYZ",
    b"abcdefghjkmnpqrstuvwxyz",
    b"23456789",
    b"!#%+-=?@",
];

pub fn unix_to_filetime(unix_seconds: i64) -> i64 {
    (unix_seconds + FILETIME_EPOCH_OFFSET) * FILETIME_PER_SECOND
}

// docs/05: deger tirnak icindeki parolanin UTF-16LE kodlamasidir
pub fn unicode_pwd(password: &str) -> Vec<u8> {
    format!("\"{password}\"")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect()
}

// Rastgele bayttan harf secerken dogrudan mod almak, alfabe 256'yi tam bolmedigi
// surece bastaki harfleri digerlerinden daha sik verir (23 harflik sinifta bes
// harf %9 fazla cikar). 256'nin alfabe katina dusmeyen baytlar atilir — kalanin
// dagilimi duzgun olur, beklenen deneme sayisi 1,2'yi gecmez.
fn pick(alphabet: &[u8]) -> char {
    let cutoff = (256 / alphabet.len() * alphabet.len()) as u32;
    let mut byte = [0u8; 1];
    loop {
        OsRng.fill_bytes(&mut byte);
        if u32::from(byte[0]) < cutoff {
            return alphabet[usize::from(byte[0]) % alphabet.len()] as char;
        }
    }
}

// Kimseye gosterilmeyen rastgele parola (ADR-009); her siniftan en az bir
// karakter, AD karmasiklik kurali icin. Ilk parola (3d) ayri ve okunabilir.
pub fn random_password() -> String {
    let all: Vec<u8> = PASSWORD_CLASSES.concat();
    (0..PASSWORD_LEN)
        .map(|i| pick(PASSWORD_CLASSES.get(i).copied().unwrap_or(&all)))
        .collect()
}

/// ADR-055 madde 5: AD uretilen parolayi politika ihlaliyle reddederse yeni parola
/// uretilip en cok bu kadar denenir; sonra ADR-009'un anlasilir hatasiyla durulur.
pub const PASSWORD_ATTEMPTS: usize = 3;

/// AD'nin parola politikasi reddi: hata kodu `0000052D` (ERROR_PASSWORD_RESTRICTION)
/// ile LDAP sonuc kodu 19 (constraintViolation, parola degisimi) ya da 53
/// (unwillingToPerform, `add` — lab Samba'da olculdu 2026-10-08). Ayni sonuc
/// kodlarinin diger ihlalleri (tekil oznitelik vb.) parola sorunu degildir.
pub fn is_password_rejection(message: &str) -> bool {
    (message.contains("rc=19") || message.contains("rc=53"))
        && message.to_ascii_uppercase().contains("0000052D")
}

/// Uretec ve yazici enjekte edilir: govde `$password` ile yazar (gerekirse klonlar),
/// AD politika reddinde (`$message` hatanin metni) yeni parola uretilip en cok
/// `PASSWORD_ATTEMPTS` kez denenir. Doner: `Ok((kabul edilen parola, sonuc))` ya da
/// son hata. Reddedilen parola hicbir yerde kalmaz. Fonksiyon degil makro: jenerik
/// async closure'in gelecegi `Send` kanitlanamiyor, yazici gelecegi ise `Send`.
#[macro_export]
macro_rules! with_fresh_password {
    ($generate:expr, |$password:ident| $write:expr, $message:expr) => {{
        let mut attempt = 1;
        loop {
            let $password = $generate();
            match $write {
                Ok(done) => break Ok(($password, done)),
                Err(e)
                    if attempt < $crate::ad_account::PASSWORD_ATTEMPTS
                        && $crate::ad_account::is_password_rejection(&$message(&e)) =>
                {
                    attempt += 1
                }
                Err(e) => break Err(e),
            }
        }
    }};
}

/// Uc ret sonrasi operatorun okuyacagi metin (ADR-009).
pub fn exhausted(message: &str) -> String {
    format!(
        "AD parola politikası üretilen parolayı {PASSWORD_ATTEMPTS} kez reddetti; uzunluk, \
         karmaşıklık ve ad parçası kuralını kontrol edin (ADR-009/055): {message}"
    )
}

// --- START FEATURE: first-password ---
// ADR-056/085: okunabilir ilk parola `Kf7m-Rq2x-Wn8d-Tz4p` — 16 karakter, dort dortlu;
// 0/O/o/1/l/I yok; her siniftan (buyuk, kucuk, rakam) en az bir karakter.
pub fn readable_password() -> String {
    loop {
        let chars: Vec<char> = (0..READABLE_PASSWORD_LEN)
            .map(|_| pick(READABLE_ALPHABET))
            .collect();
        let has = |f: fn(&char) -> bool| chars.iter().any(f);
        if has(char::is_ascii_uppercase)
            && has(char::is_ascii_lowercase)
            && has(char::is_ascii_digit)
        {
            return chars
                .chunks(READABLE_GROUP_LEN)
                .map(|g| g.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join("-");
        }
    }
}
// --- END FEATURE: first-password ---

// docs/05 CN kurali: "{given} {surname}", OU'da cakisirsa "{given} {surname} ({username})";
// 64'u asarsa ad-soyad kesilir, parantez korunur.
pub fn cn_for(given: &str, surname: &str, username: Option<&str>) -> String {
    let base = format!("{given} {surname}").trim().to_string();
    match username {
        None => base.chars().take(CN_MAX_LEN).collect(),
        Some(u) => {
            let suffix = format!(" ({u})");
            let keep = CN_MAX_LEN.saturating_sub(suffix.chars().count());
            let head: String = base.chars().take(keep).collect();
            format!("{}{suffix}", head.trim_end())
        }
    }
}

pub fn account_dn(cn: &str, ou_dn: &str) -> String {
    format!("CN={},{ou_dn}", dn_escape(cn))
}

// UAC bit degisimi (docs/05 Etkinlestirme/pasiflestirme); kilit (lockoutTime) ayri, dokunulmaz
pub fn with_enabled(uac: u32, enabled: bool) -> u32 {
    if enabled {
        uac & !UAC_ACCOUNT_DISABLE
    } else {
        uac | UAC_ACCOUNT_DISABLE
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryAccount {
    pub dn: String,
    pub guid: String,
    pub enabled: bool,
    pub member_of: Vec<String>,
    pub last_logon_timestamp: Option<String>,
    pub pwd_last_set: Option<String>,
}

// GUID ile arama tabani (yalnizca tireli yazim, ADR-057 madde 4)
pub async fn find_by_guid(
    ldap: &mut Ldap,
    guid: &str,
) -> Result<Option<DirectoryAccount>, WriteError> {
    let base = format!("<GUID={guid}>");
    let attrs = [
        "objectGUID",
        "userAccountControl",
        "memberOf",
        "lastLogonTimestamp",
        "pwdLastSet",
    ];
    let found = match ad::search(ldap, &base, Scope::Base, "(objectClass=*)", &attrs).await {
        Ok(found) => found,
        // noSuchObject (32): hesap yok (kayip); diger kodlar hata
        Err(WriteError::Failed(msg)) if msg.contains("rc=32") || msg.contains("noSuchObject") => {
            return Ok(None)
        }
        Err(e) => return Err(e),
    };
    Ok(found.into_iter().next().map(|entry| {
        let text = |name: &str| entry.attrs.get(name).and_then(|v| v.first()).cloned();
        let uac: u32 = text("userAccountControl")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        DirectoryAccount {
            dn: entry.dn.clone(),
            guid: guid.to_string(),
            enabled: uac & UAC_ACCOUNT_DISABLE == 0,
            member_of: entry.attrs.get("memberOf").cloned().unwrap_or_default(),
            last_logon_timestamp: text("lastLogonTimestamp"),
            pwd_last_set: text("pwdLastSet"),
        }
    }))
}

pub async fn dn_by_guid(ldap: &mut Ldap, guid: &str) -> Result<Option<String>, WriteError> {
    Ok(find_by_guid(ldap, guid).await?.map(|a| a.dn))
}

// Yeni hesabin objectGUID'i DN'den okunur (docs/05 adim: "objectGUID hemen okunup kaydedilir")
pub async fn guid_by_dn(ldap: &mut Ldap, dn: &str) -> Result<String, WriteError> {
    let found = ad::search(ldap, dn, Scope::Base, "(objectClass=*)", &["objectGUID"]).await?;
    found
        .first()
        .and_then(|e| {
            e.bin_attrs
                .get("objectGUID")
                .and_then(|v| v.first())
                .and_then(|b| ad::guid_to_string(b))
                .or_else(|| {
                    e.attrs
                        .get("objectGUID")
                        .and_then(|v| v.first())
                        .and_then(|s| ad::guid_to_string(s.as_bytes()))
                })
        })
        .ok_or_else(|| WriteError::Failed(format!("objectGUID okunamadı: {dn}")))
}

pub async fn cn_exists(ldap: &mut Ldap, ou_dn: &str, cn: &str) -> Result<bool, WriteError> {
    let filter = format!("(cn={})", ldap_escape(cn));
    let found = ad::search(ldap, ou_dn, Scope::OneLevel, &filter, &["cn"]).await?;
    Ok(!found.is_empty())
}

/// Bagli `Ldap` uzerinden yazan connector; `writes::apply` niyet/sonucu sarar.
pub struct AdWriter<'a> {
    pub ldap: &'a mut Ldap,
}

impl TargetWriter for AdWriter<'_> {
    async fn write(&mut self, op: &WriteOp) -> Result<(), WriteError> {
        match op {
            WriteOp::CreateAccount {
                dn,
                attributes,
                password,
                account_expires,
            } => create_account(self.ldap, dn, attributes, password, *account_expires).await,
            WriteOp::SetEnabled { dn, enabled } => set_enabled(self.ldap, dn, *enabled).await,
            WriteOp::AddMember {
                group_dn,
                member_dn,
            } => change_member(self.ldap, group_dn, member_dn, true).await,
            WriteOp::RemoveMember {
                group_dn,
                member_dn,
            } => change_member(self.ldap, group_dn, member_dn, false).await,
            WriteOp::SetAttributes { dn, changes } => set_attributes(self.ldap, dn, changes).await,
            WriteOp::MoveAccount {
                dn,
                new_rdn,
                new_parent,
            } => self
                .ldap
                .modifydn(dn, new_rdn, true, Some(new_parent))
                .await
                .map_err(ad::classify)?
                .success()
                .map(|_| ())
                .map_err(ad::classify),
            WriteOp::DeleteAccount { dn } => self
                .ldap
                .delete(dn)
                .await
                .map_err(ad::classify)?
                .success()
                .map(|_| ())
                .map_err(ad::classify),
            // ADR-033: kimsenin bilmedigi rastgele parola + pwdLastSet 0; parola hic
            // saklanmaz. AD reddederse yenisi uretilir (ADR-055 madde 5).
            WriteOp::ResetPassword { dn } => reset_password(self.ldap, dn).await,
            WriteOp::SetFirstPassword {
                dn,
                password,
                change_required,
            } => set_password(self.ldap, dn, password, *change_required).await,
        }
    }
}

// ADR-033: kimsenin bilmedigi rastgele parola + pwdLastSet 0; parola hic saklanmaz.
// AD reddederse yenisi uretilir (ADR-055 madde 5).
async fn reset_password(ldap: &mut Ldap, dn: &str) -> Result<(), WriteError> {
    let message = |e: &WriteError| e.to_string();
    let written: Result<(String, ()), WriteError> = crate::with_fresh_password!(
        random_password,
        |password| set_password(ldap, dn, &password, true).await,
        message
    );
    written
        .map(|_| ())
        .map_err(|e| match is_password_rejection(&e.to_string()) {
            true => WriteError::Failed(exhausted(&e.to_string())),
            false => e,
        })
}

// Parola yazilir; `change_required` ise pwdLastSet 0 (ilk giriste degistir, ADR-009/019),
// degilse AD'nin verdigi zaman damgasi kalir.
async fn set_password(
    ldap: &mut Ldap,
    dn: &str,
    password: &str,
    change_required: bool,
) -> Result<(), WriteError> {
    let mut mods = vec![Mod::Replace(
        b"unicodePwd".to_vec(),
        HashSet::from([unicode_pwd(password)]),
    )];
    if change_required {
        mods.push(Mod::Replace(
            b"pwdLastSet".to_vec(),
            HashSet::from([b"0".to_vec()]),
        ));
    }
    ldap.modify(dn, mods)
        .await
        .map_err(ad::classify)?
        .success()
        .map(|_| ())
        .map_err(ad::classify)
}

/// DN'i (RDN, ust) olarak ayirir; kacisli virgul (`\,`) ayirici sayilmaz.
pub fn split_dn(dn: &str) -> Option<(&str, &str)> {
    let bytes = dn.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b',' => return Some((&dn[..i], &dn[i + 1..])),
            _ => i += 1,
        }
    }
    None
}

/// Hesabin uyesi oldugu gruplarin objectGUID'leri (uyelik farki icin; docs/05).
pub async fn member_group_guids(
    ldap: &mut Ldap,
    base_dn: &str,
    member_dn: &str,
) -> Result<Vec<String>, WriteError> {
    let filter = format!("(&(objectClass=group)(member={}))", ldap_escape(member_dn));
    let found = ad::search(ldap, base_dn, Scope::Subtree, &filter, &["objectGUID"]).await?;
    Ok(found
        .iter()
        .filter_map(|e| {
            e.bin_attrs
                .get("objectGUID")
                .and_then(|v| v.first())
                .and_then(|b| ad::guid_to_string(b))
        })
        .collect())
}

// docs/05 Oznitelik guncelleme: replace; kaynak bossa sil (ADR-012).
async fn set_attributes(
    ldap: &mut Ldap,
    dn: &str,
    changes: &[(String, Option<String>)],
) -> Result<(), WriteError> {
    let mods: Vec<Mod<&str>> = changes
        .iter()
        .map(|(attr, value)| match value {
            Some(v) => Mod::Replace(attr.as_str(), HashSet::from([v.as_str()])),
            None => Mod::Delete(attr.as_str(), HashSet::new()),
        })
        .collect();
    ldap.modify(dn, mods)
        .await
        .map_err(ad::classify)?
        .success()
        .map(|_| ())
        .map_err(ad::classify)
}

/// Eslenen ozniteliklerin hedefteki mevcut degerleri (fark icin).
pub async fn read_attributes(
    ldap: &mut Ldap,
    dn: &str,
    attrs: &[&str],
) -> Result<HashMap<String, Vec<String>>, WriteError> {
    let found = ad::search(ldap, dn, Scope::Base, "(objectClass=*)", attrs).await?;
    Ok(found
        .into_iter()
        .next()
        .map(|entry| entry.attrs)
        .unwrap_or_default())
}

// ADR-057 madde 3: tek `add`. AD parolayi reddederse hesap hic olusmaz.
async fn create_account(
    ldap: &mut Ldap,
    dn: &str,
    attributes: &[(String, String)],
    password: &str,
    account_expires: Option<i64>,
) -> Result<(), WriteError> {
    let uac = (UAC_NORMAL_ACCOUNT | UAC_ACCOUNT_DISABLE).to_string();
    let expires = account_expires
        .map(unix_to_filetime)
        .unwrap_or(0)
        .to_string();
    let mut attrs: Vec<(Vec<u8>, HashSet<Vec<u8>>)> = vec![
        (b"objectClass".to_vec(), HashSet::from([b"user".to_vec()])),
        (
            b"userAccountControl".to_vec(),
            HashSet::from([uac.into_bytes()]),
        ),
        (b"pwdLastSet".to_vec(), HashSet::from([b"0".to_vec()])),
        (
            b"accountExpires".to_vec(),
            HashSet::from([expires.into_bytes()]),
        ),
        (
            b"unicodePwd".to_vec(),
            HashSet::from([unicode_pwd(password)]),
        ),
    ];
    for (name, value) in attributes {
        if !value.is_empty() {
            attrs.push((
                name.clone().into_bytes(),
                HashSet::from([value.clone().into_bytes()]),
            ));
        }
    }
    ldap.add(dn, attrs)
        .await
        .map_err(ad::classify)?
        .success()
        .map(|_| ())
        .map_err(ad::classify)
}

async fn set_enabled(ldap: &mut Ldap, dn: &str, enabled: bool) -> Result<(), WriteError> {
    let current = ad::search(
        ldap,
        dn,
        Scope::Base,
        "(objectClass=*)",
        &["userAccountControl"],
    )
    .await?;
    let uac: u32 = current
        .first()
        .and_then(|e| e.attrs.get("userAccountControl"))
        .and_then(|v| v.first())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| WriteError::Failed(format!("userAccountControl okunamadı: {dn}")))?;
    let next = with_enabled(uac, enabled).to_string();
    ldap.modify(
        dn,
        vec![Mod::Replace(
            "userAccountControl",
            HashSet::from([next.as_str()]),
        )],
    )
    .await
    .map_err(ad::classify)?
    .success()
    .map(|_| ())
    .map_err(ad::classify)
}

// docs/05 Grup uyeligi: memberOf yazilamaz, grubun member'i degistirilir
async fn change_member(
    ldap: &mut Ldap,
    group_dn: &str,
    member_dn: &str,
    add: bool,
) -> Result<(), WriteError> {
    let values = HashSet::from([member_dn]);
    let change = if add {
        Mod::Add("member", values)
    } else {
        Mod::Delete("member", values)
    };
    ldap.modify(group_dn, vec![change])
        .await
        .map_err(ad::classify)?
        .success()
        .map(|_| ())
        .map_err(ad::classify)
}
// --- END FEATURE: ad-provisioning ---

#[cfg(test)]
mod tests {
    use super::*;

    const REJECTED: &str = "LDAP operation result: rc=19 (constraintViolation), dn: \"\", \
        text: \"0000052D: Constraint violation - check_password_restrictions\"";
    const UNIQUE: &str = "LDAP operation result: rc=19 (constraintViolation), dn: \"\", \
        text: \"00002082: unique attribute\"";
    // lab Samba'nin `add` reddi, birebir (2026-10-08)
    const SAMBA_ADD: &str = "LDAP operation result: rc=53 (unwillingToPerform), dn: \"\", \
        text: \"0000052D: Unwilling to perform - check_password_restrictions: the password is too short.\"";

    // ADR-055 madde 5: uretec ve yazici enjekte; ret sayisi ve son hata.
    async fn run(answers: &[&'static str]) -> (Result<(String, ()), WriteError>, usize) {
        let mut calls = 0;
        let mut n = 0;
        let message = |e: &WriteError| e.to_string();
        let result: Result<(String, ()), WriteError> = crate::with_fresh_password!(
            || {
                n += 1;
                format!("parola-{n}")
            },
            |password| {
                let answer = answers[calls];
                calls += 1;
                let _ = &password;
                async {
                    if answer.is_empty() {
                        Ok(())
                    } else {
                        Err(WriteError::Failed(answer.into()))
                    }
                }
                .await
            },
            message
        );
        (result, calls)
    }

    #[tokio::test]
    async fn rejected_password_is_regenerated_at_most_three_times() {
        let (ok, calls) = run(&[REJECTED, REJECTED, ""]).await;
        assert_eq!(
            ok.unwrap().0,
            "parola-3",
            "yalnızca kabul edilen parola döner"
        );
        assert_eq!(calls, 3);
        let (err, calls) = run(&[REJECTED, REJECTED, REJECTED, ""]).await;
        assert_eq!(calls, 3, "dördüncü deneme yok");
        let message = err.unwrap_err().to_string();
        assert!(is_password_rejection(&message));
        assert!(exhausted(&message).contains("3 kez reddetti"));
        let (ok, calls) = run(&[SAMBA_ADD, ""]).await;
        assert!(
            ok.is_ok() && calls == 2,
            "add reddi (53) de yeniden üretilir"
        );
        assert!(!is_password_rejection(
            "rc=53 (unwillingToPerform), text: \"00002077\""
        ));
        let (other, calls) = run(&[UNIQUE, ""]).await;
        assert_eq!(calls, 1, "başka kısıt ihlalinde yeni parola üretilmez");
        assert!(!is_password_rejection(&other.unwrap_err().to_string()));
    }

    #[test]
    fn readable_password_is_four_groups_without_confusable_chars() {
        for _ in 0..50 {
            let p = readable_password();
            let groups: Vec<&str> = p.split('-').collect();
            assert_eq!(groups.len(), 4, "{p}");
            assert!(groups.iter().all(|g| g.len() == 4), "{p}");
            assert!(!p.chars().any(|c| "0Oo1lI".contains(c)), "{p}");
            assert!(p.chars().any(|c| c.is_ascii_uppercase()), "{p}");
            assert!(p.chars().any(|c| c.is_ascii_lowercase()), "{p}");
            assert!(p.chars().any(|c| c.is_ascii_digit()), "{p}");
        }
    }

    #[test]
    fn split_dn_respects_escaped_commas() {
        assert_eq!(
            split_dn("CN=Yilmaz\\, Ayse,OU=Personel,DC=x"),
            Some(("CN=Yilmaz\\, Ayse", "OU=Personel,DC=x"))
        );
        assert_eq!(split_dn("CN=a,DC=x"), Some(("CN=a", "DC=x")));
        assert_eq!(split_dn("DC=x"), None);
    }

    #[test]
    fn filetime_epoch_and_unicode_pwd_encoding() {
        assert_eq!(unix_to_filetime(0), 116_444_736_000_000_000);
        assert_eq!(unicode_pwd("a"), vec![b'"', 0, b'a', 0, b'"', 0]);
    }

    // Red ornekleme: harf hep alfabeden gelir ve alfabenin tamami cikabilir
    // (bir harf hic cikmiyorsa cutoff yanlis hesaplanmistir).
    #[test]
    fn pick_stays_in_the_alphabet_and_reaches_all_of_it() {
        for alphabet in [READABLE_ALPHABET, PASSWORD_CLASSES[0], PASSWORD_CLASSES[2]] {
            let seen: std::collections::HashSet<char> =
                (0..alphabet.len() * 200).map(|_| pick(alphabet)).collect();
            assert!(
                seen.iter().all(|c| alphabet.contains(&(*c as u8))),
                "alfabe dışı harf: {seen:?}"
            );
            assert_eq!(seen.len(), alphabet.len(), "erişilemeyen harf var");
        }
    }

    #[test]
    fn password_has_every_class_and_length() {
        let pw = random_password();
        assert_eq!(pw.chars().count(), PASSWORD_LEN);
        for class in PASSWORD_CLASSES {
            assert!(pw.bytes().any(|b| class.contains(&b)), "sınıf eksik: {pw}");
        }
        assert_ne!(random_password(), pw);
    }

    #[test]
    fn cn_rule_and_uac_bits() {
        assert_eq!(cn_for("Ayşe", "Yılmaz", None), "Ayşe Yılmaz");
        assert_eq!(
            cn_for("Ayşe", "Yılmaz", Some("ayse.yilmaz")),
            "Ayşe Yılmaz (ayse.yilmaz)"
        );
        let long = cn_for(&"A".repeat(70), "B", Some("ab"));
        assert_eq!(long.chars().count(), CN_MAX_LEN);
        assert!(long.ends_with(" (ab)"));
        assert_eq!(
            account_dn("Yılmaz, Ayşe", "OU=Personel,DC=x"),
            "CN=Yılmaz\\2c Ayşe,OU=Personel,DC=x"
        );
        assert_eq!(with_enabled(0x202, true), 0x200);
        assert_eq!(with_enabled(0x200, false), 0x202);
        assert_eq!(with_enabled(0x202, false), 0x202);
    }

    // Lab Samba AD: tek add ile hesap acilir (pasif), etkinlestirilir, gruba
    // eklenir/cikarilir, GUID ile bulunur, silinir; reddedilen parola hesap olusturmaz.
    // ADR-055 madde 5, lab: Samba politika ihlalini (zayif parola) 19 + 0000052D ile
    // reddeder; yeniden uretim hesabi acar ve kabul edilen deger doner. Not (olcum
    // 2026-10-08): Samba 4.24 sAMAccountName'i iceren parolayi REDDETMIYOR — ad parcasi
    // kurali Windows'ta; o ret `windows_ad_answers_open_questions`ta olculur.
    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn lab_rejects_a_weak_password_and_regeneration_opens_the_account() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let cfg = ad::AdConfig {
            urls: ad::parse_urls(&var("AD_LAB_URL")),
            bind_dn: var("AD_LAB_BIND_DN"),
            password: var("AD_LAB_PASSWORD"),
            ca_file: var("AD_CA_FILE"),
        };
        let mut ldap = ad::connect(&cfg).await.expect("lab AD");
        let username = format!("yeniden{}", std::process::id() % 100_000);
        let dn = account_dn(
            &cn_for("Yeniden", "Uretim", None),
            "OU=Personel,DC=opensicil,DC=lab",
        );
        let _ = ldap.delete(&dn).await; // onceki calismadan kalan
        let attributes = vec![
            ("sAMAccountName".to_string(), username.clone()),
            ("givenName".to_string(), "Yeniden".to_string()),
            ("sn".to_string(), "Uretim".to_string()),
        ];
        let mut writer = AdWriter { ldap: &mut ldap };
        let mut first = true;
        let mut generate = || match std::mem::take(&mut first) {
            true => "zayif".to_string(),
            false => random_password(),
        };
        let mut rejected = Vec::new();
        let message = |e: &WriteError| e.to_string();
        let created: Result<(String, ()), WriteError> = crate::with_fresh_password!(
            generate,
            |password| {
                let op = WriteOp::CreateAccount {
                    dn: dn.clone(),
                    attributes: attributes.clone(),
                    password: password.clone(),
                    account_expires: None,
                };
                let result = writer.write(&op).await;
                if let Err(e) = &result {
                    rejected.push(e.to_string());
                }
                result
            },
            message
        );
        let (password, ()) = created.expect("ikinci denemede hesap açılır");
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(is_password_rejection(&rejected[0]), "{}", rejected[0]);
        assert_ne!(password, "zayif");
        assert!(guid_by_dn(writer.ldap, &dn).await.is_ok());
        writer.ldap.delete(&dn).await.unwrap().success().unwrap();
    }

    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn creates_enables_and_deletes_account_in_lab() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let cfg = ad::AdConfig {
            urls: ad::parse_urls(&var("AD_LAB_URL")),
            bind_dn: var("AD_LAB_BIND_DN"),
            password: var("AD_LAB_PASSWORD"),
            ca_file: var("AD_CA_FILE"),
        };
        let mut ldap = ad::connect(&cfg).await.expect("lab AD");
        let username = format!("test{}", std::process::id() % 100_000);
        let ou = "OU=Personel,DC=opensicil,DC=lab";
        let dn = account_dn(&cn_for("Test", "Kişi", None), ou);
        let _ = ldap.delete(&dn).await; // onceki calismadan kalan
        let attributes = vec![
            ("cn".to_string(), "Test Kişi".to_string()),
            ("sAMAccountName".to_string(), username.clone()),
            (
                "userPrincipalName".to_string(),
                format!("{username}@opensicil.lab"),
            ),
            ("givenName".to_string(), "Test".to_string()),
            ("sn".to_string(), "Kişi".to_string()),
            ("displayName".to_string(), "Test Kişi".to_string()),
        ];
        let mut writer = AdWriter { ldap: &mut ldap };

        let weak = WriteOp::CreateAccount {
            dn: dn.clone(),
            attributes: attributes.clone(),
            password: "a".into(),
            account_expires: None,
        };
        assert!(
            matches!(writer.write(&weak).await, Err(WriteError::Failed(_))),
            "zayıf parola reddedilir"
        );
        assert!(
            guid_by_dn(writer.ldap, &dn).await.is_err(),
            "reddedilen parolada hesap oluşmaz"
        );

        let create = WriteOp::CreateAccount {
            dn: dn.clone(),
            attributes,
            password: random_password(),
            account_expires: Some(4_102_444_800),
        };
        writer
            .write(&create)
            .await
            .expect("tek add ile hesap açılmalı");
        let guid = guid_by_dn(writer.ldap, &dn).await.unwrap();
        let account = find_by_guid(writer.ldap, &guid)
            .await
            .unwrap()
            .expect("GUID ile bulunmalı");
        assert!(!account.enabled, "pasif açılır (UAC 514)");
        assert_eq!(account.pwd_last_set.as_deref(), Some("0"));
        assert!(
            account.last_logon_timestamp.is_none(),
            "hiç giriş yapılmamış"
        );

        writer
            .write(&WriteOp::SetEnabled {
                dn: dn.clone(),
                enabled: true,
            })
            .await
            .unwrap();
        assert!(
            find_by_guid(writer.ldap, &guid)
                .await
                .unwrap()
                .unwrap()
                .enabled
        );
        let group_dn = "CN=GG-VPN,OU=Gruplar,DC=opensicil,DC=lab".to_string();
        writer
            .write(&WriteOp::AddMember {
                group_dn: group_dn.clone(),
                member_dn: dn.clone(),
            })
            .await
            .unwrap();
        assert!(find_by_guid(writer.ldap, &guid)
            .await
            .unwrap()
            .unwrap()
            .member_of
            .iter()
            .any(|g| g.eq_ignore_ascii_case(&group_dn)));
        writer
            .write(&WriteOp::RemoveMember {
                group_dn,
                member_dn: dn.clone(),
            })
            .await
            .unwrap();
        writer
            .write(&WriteOp::SetEnabled {
                dn: dn.clone(),
                enabled: false,
            })
            .await
            .unwrap();
        assert!(
            !find_by_guid(writer.ldap, &guid)
                .await
                .unwrap()
                .unwrap()
                .enabled
        );

        ldap.delete(&dn).await.unwrap().success().unwrap();
        assert!(
            find_by_guid(&mut ldap, &guid).await.unwrap().is_none(),
            "silinen hesap kayıp görünür"
        );
        ldap.unbind().await.ok();
    }

    // Gercek Windows Server AD (Samba degil): docs/08'in ⊞ isaretli sorularindan
    // ucu olculur — tek `add` kabulu ve pwdLastSet'in 0 kalmasi, ad parcasi iceren
    // parolanin reddi (ADR-055), <GUID=…> DN'inin modify/modifyDN/delete hedefi
    // olarak kabulu (yalnizca bilgi: worker gercek DN ile yazar). Sonuclar docs/11.
    #[tokio::test]
    #[ignore = "gerçek Windows AD gerektirir: AD_WIN_URL, AD_WIN_BIND_DN, AD_WIN_PASSWORD, AD_WIN_CA_FILE, AD_WIN_OU ile çalıştır"]
    async fn windows_ad_answers_open_questions() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let cfg = ad::AdConfig {
            urls: ad::parse_urls(&var("AD_WIN_URL")),
            bind_dn: var("AD_WIN_BIND_DN"),
            password: var("AD_WIN_PASSWORD"),
            ca_file: var("AD_WIN_CA_FILE"),
        };
        let ou = var("AD_WIN_OU");
        let mut ldap = ad::connect(&cfg).await.expect("Windows AD");
        let base = ad::base_dn(&mut ldap).await.unwrap();
        let domain = base
            .split(',')
            .filter_map(|part| {
                let part = part.trim();
                part.strip_prefix("DC=")
                    .or_else(|| part.strip_prefix("dc="))
            })
            .collect::<Vec<_>>()
            .join(".");
        let username = format!("wintest{}", std::process::id() % 100_000);
        let (given, surname) = ("Wintest", "Hogwartsdeneme");
        let dn = account_dn(&cn_for(given, surname, None), &ou);
        let _ = ldap.delete(&dn).await; // onceki calismadan kalan

        // ⊞ 1: tek `add` (unicodePwd + UAC 514 + pwdLastSet 0), LDAPS + simple bind
        let mut writer = AdWriter { ldap: &mut ldap };
        writer
            .write(&WriteOp::CreateAccount {
                dn: dn.clone(),
                attributes: vec![
                    ("sAMAccountName".to_string(), username.clone()),
                    (
                        "userPrincipalName".to_string(),
                        format!("{username}@{domain}"),
                    ),
                    ("givenName".to_string(), given.to_string()),
                    ("sn".to_string(), surname.to_string()),
                    ("displayName".to_string(), format!("{given} {surname}")),
                ],
                password: random_password(),
                account_expires: None,
            })
            .await
            .expect("tek add ile hesap açılmalı");
        let guid = guid_by_dn(writer.ldap, &dn).await.unwrap();
        let account = find_by_guid(writer.ldap, &guid)
            .await
            .unwrap()
            .expect("GUID ile bulunmalı");
        assert!(!account.enabled, "pasif açılır (UAC 514)");
        assert_eq!(
            account.pwd_last_set.as_deref(),
            Some("0"),
            "pwdLastSet 0 kalır"
        );

        // ⊞ 2: ad parcasi iceren parola reddedilir, rastgele parola kabul edilir
        let named = writer
            .write(&WriteOp::SetFirstPassword {
                dn: dn.clone(),
                password: format!("{surname}.2026!"),
                change_required: true,
            })
            .await;
        log_error!("⊞ ad parçası içeren parola: {named:?}");
        assert!(
            matches!(named, Err(WriteError::Failed(_))),
            "ad parçası içeren parola reddedilir (ADR-055)"
        );
        writer
            .write(&WriteOp::SetFirstPassword {
                dn: dn.clone(),
                password: readable_password(),
                change_required: true,
            })
            .await
            .expect("rastgele parola kabul edilir");

        // ⊞ 3: <GUID=…> yazma hedefi olabiliyor mu (bilgi; assert yok)
        let guid_dn = format!("<GUID={guid}>");
        let modify = writer
            .write(&WriteOp::SetAttributes {
                dn: guid_dn.clone(),
                changes: vec![("description".to_string(), Some("guid-dn".to_string()))],
            })
            .await;
        let rename = writer
            .write(&WriteOp::MoveAccount {
                dn: guid_dn.clone(),
                new_rdn: format!("CN={}", dn_escape(format!("{given} {surname} 2"))),
                new_parent: ou.clone(),
            })
            .await;
        let delete = writer.write(&WriteOp::DeleteAccount { dn: guid_dn }).await;
        log_error!("⊞ <GUID=…> modify: {modify:?}");
        log_error!("⊞ <GUID=…> modifyDN: {rename:?}");
        log_error!("⊞ <GUID=…> delete: {delete:?}");

        if let Some(left) = dn_by_guid(&mut ldap, &guid).await.unwrap() {
            ldap.delete(&left).await.unwrap().success().unwrap();
        }
        assert!(
            find_by_guid(&mut ldap, &guid).await.unwrap().is_none(),
            "test hesabı temizlendi"
        );
        ldap.unbind().await.ok();
    }
}
