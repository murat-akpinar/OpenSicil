// --- START FEATURE: ad-login ---
// ADR-095 madde 1, 2 ve 6: asil giris kapisi, operatorun kendi AD parolasiyla
// LDAPS uzerinden simple bind'dir. Parola hicbir yere yazilmaz, hash'lenmez,
// loglanmaz; yalnizca o bind cagrisi boyunca bellekte durur. Kullanici arama ve
// grup okuma servis hesabiyla yapilir (yetki karari operatorun okuma hakkina
// birakilmaz). worker/src/ad.rs'in tamami kopyalanmaz (ADR-095 Sonuclari):
// burada yalnizca kimlik dogrulama dilimi var.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use ldap3::{ldap_escape, Ldap, LdapConnAsync, LdapConnSettings, LdapError, Scope, SearchEntry};
use sqlx::PgPool;

const LDAPS_PORT: u16 = 636;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CA_FILE_VAR: &str = "AD_CA_FILE";
// LDAP sonuc kodu 49: gecersiz kimlik bilgisi (yanlis parola, pasif/kilitli hesap)
const INVALID_CREDENTIALS_RC: u32 = 49;
const USER_ATTRS: [&str; 4] = ["sAMAccountName", "objectGUID", "mail", "memberOf"];
const GROUP_ATTRS: [&str; 2] = ["cn", "memberOf"];

#[derive(Debug)]
pub enum AuthError {
    /// Yapilandirma sayfasinda AD bos: bu kapi hic yok
    NotConfigured,
    /// DC'ye ulasilamiyor, CA/servis hesabi bozuk ya da dizin beklenmeyen cevap
    /// verdi. Ekranda "AD'ye ulasilamiyor" yazar, yerel kapi calismaya devam eder
    /// (ADR-095 Sonuclari: giris sessizce basarisiz olmaz).
    Unavailable(String),
    /// Kullanici bulunamadi ya da parola yanlis; ikisi ayirt edilmez
    BadCredentials,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::NotConfigured => write!(f, "AD yapılandırılmadı"),
            AuthError::Unavailable(e) => write!(f, "AD'ye ulaşılamıyor: {e}"),
            AuthError::BadCredentials => write!(f, "kullanıcı adı ya da parola yanlış"),
        }
    }
}

/// Dogrulanmis operator; `username` sAMAccountName'dir ve kimlik eslesmesi
/// bununla yapilir (ADR-095 madde 6: tahmin yok).
pub struct AdOperator {
    pub username: String,
    pub guid: String,
    pub email: String,
    pub authorities: Vec<String>,
}

pub async fn authenticate(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    username: &str,
    password: &str,
) -> Result<AdOperator, AuthError> {
    // Bos parolayla simple bind LDAP'ta "unauthenticated bind"dir ve sunucu
    // basarili donebilir (RFC 4513 madde 5.1.2); dizine hic gidilmez.
    if username.trim().is_empty() || password.is_empty() {
        return Err(AuthError::BadCredentials);
    }
    let cfg = load_config(pool, aead_key).await?;
    let mut ldap = service_connect(&cfg).await?;
    let base = base_dn(&mut ldap).await?;
    let Some(user) = find_user(&mut ldap, &base, username).await? else {
        return Err(AuthError::BadCredentials);
    };
    // Asil dogrulama: operatorun DN'i + operatorun parolasi
    drop(connect(&cfg, &user.dn, password).await?);
    let groups = group_names(&mut ldap, &user.member_of).await?;
    Ok(AdOperator {
        username: user.sam,
        guid: user.guid,
        email: user.email,
        authorities: crate::oidc::authorities_for_groups(&groups),
    })
}

struct AdConfig {
    urls: Vec<String>,
    bind_dn: String,
    password: String,
    ca_file: String,
}

// Baglanti bilgisi Yapilandirma sayfasindan (ADR-068), parola AEAD ile cozulur.
async fn load_config(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
) -> Result<AdConfig, AuthError> {
    let row: (String, String, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT ad_host, ad_bind_dn, ad_service_password_enc FROM app_settings WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| AuthError::Unavailable(format!("AD ayarları okunamadı: {e}")))?;
    let (ad_host, bind_dn, password_enc) = row;
    if ad_host.trim().is_empty() {
        return Err(AuthError::NotConfigured);
    }
    let password_enc = password_enc.ok_or_else(|| {
        AuthError::Unavailable("AD servis hesabı parolası girilmemiş".to_string())
    })?;
    let password = String::from_utf8(
        crate::crypto::decrypt(aead_key, &password_enc).map_err(AuthError::Unavailable)?,
    )
    .map_err(|_| AuthError::Unavailable("AD parolası UTF-8 değil".to_string()))?;
    let ca_file = std::env::var(CA_FILE_VAR)
        .map_err(|_| AuthError::Unavailable(format!("{CA_FILE_VAR} ortam değişkeni eksik")))?;
    Ok(AdConfig {
        urls: parse_urls(&ad_host),
        bind_dn,
        password,
        ca_file,
    })
}

// ADR-016: DC adresi sirali listedir; ilk adres tercihli. "dc1;dc2:636;ldaps://dc3".
pub fn parse_urls(ad_host: &str) -> Vec<String> {
    ad_host
        .split([';', ','])
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(|h| {
            if h.contains("://") {
                h.to_string()
            } else if h.contains(':') {
                format!("ldaps://{h}")
            } else {
                format!("ldaps://{h}:{LDAPS_PORT}")
            }
        })
        .collect()
}

// PEM'deki her CERTIFICATE blogu DER'e cevrilir; ek crate yok (base64 zaten var).
fn read_pem_certs(path: &str) -> Result<Vec<Vec<u8>>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("CA dosyası okunamadı ({path}): {e}"))?;
    let mut certs = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines().map(str::trim) {
        match (line, current.as_mut()) {
            ("-----BEGIN CERTIFICATE-----", _) => current = Some(String::new()),
            ("-----END CERTIFICATE-----", Some(body)) => {
                let der = base64::engine::general_purpose::STANDARD
                    .decode(body.as_bytes())
                    .map_err(|e| format!("CA dosyası base64 çözülemedi: {e}"))?;
                certs.push(der);
                current = None;
            }
            (_, Some(body)) => body.push_str(line),
            _ => {}
        }
    }
    if certs.is_empty() {
        return Err(format!("CA dosyasında sertifika yok: {path}"));
    }
    Ok(certs)
}

// Sertifika dogrulamasi kapatilamaz (docs/05): kok CA dosyadan gelir.
fn tls_settings(ca_file: &str) -> Result<LdapConnSettings, AuthError> {
    let mut roots = rustls::RootCertStore::empty();
    for der in read_pem_certs(ca_file).map_err(AuthError::Unavailable)? {
        roots
            .add(der.into())
            .map_err(|e| AuthError::Unavailable(format!("CA sertifikası geçersiz: {e}")))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| AuthError::Unavailable(format!("TLS yapılandırması kurulamadı: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(LdapConnSettings::new()
        .set_config(Arc::new(config))
        .set_conn_timeout(CONNECT_TIMEOUT))
}

// Bind reddi (rc 49) kimlik bilgisi hatasidir; TCP/TLS/zaman asimi hedef
// arizasidir ve siradaki DC denenir (ADR-016).
fn classify(error: LdapError) -> AuthError {
    match error {
        LdapError::LdapResult { ref result } if result.rc == INVALID_CREDENTIALS_RC => {
            AuthError::BadCredentials
        }
        other => AuthError::Unavailable(other.to_string()),
    }
}

async fn connect(cfg: &AdConfig, bind_dn: &str, password: &str) -> Result<Ldap, AuthError> {
    let mut last = AuthError::Unavailable("DC adresi verilmedi".to_string());
    for url in &cfg.urls {
        let settings = tls_settings(&cfg.ca_file)?;
        match LdapConnAsync::with_settings(settings, url).await {
            Ok((conn, mut ldap)) => {
                ldap3::drive!(conn);
                ldap.simple_bind(bind_dn, password)
                    .await
                    .map_err(classify)?
                    .success()
                    .map_err(classify)?;
                return Ok(ldap);
            }
            Err(e) => {
                log_error!("ad_auth: {url} bağlanamadı: {e}");
                last = classify(e);
            }
        }
    }
    Err(last)
}

// Servis hesabinin bind'i reddedilirse bu operatorun parolasiyla ilgili degil:
// kurulum hatasidir, operatore "parolan yanlis" denmez.
async fn service_connect(cfg: &AdConfig) -> Result<Ldap, AuthError> {
    connect(cfg, &cfg.bind_dn, &cfg.password)
        .await
        .map_err(|e| match e {
            AuthError::BadCredentials => {
                AuthError::Unavailable("AD servis hesabının bind'ı reddedildi".to_string())
            }
            other => other,
        })
}

// Her arama EntriesOnly (yonlendirmeler ayiklanir, ldap3 #156) + sayfali (docs/05).
async fn search(
    ldap: &mut Ldap,
    base: &str,
    scope: Scope,
    filter: &str,
    attrs: &[&str],
) -> Result<Vec<SearchEntry>, AuthError> {
    use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
    let adapters: Vec<Box<dyn Adapter<&str, &[&str]>>> = vec![
        Box::new(EntriesOnly::new()),
        Box::new(PagedResults::new(500)),
    ];
    let mut stream = ldap
        .streaming_search_with(adapters, base, scope, filter, attrs)
        .await
        .map_err(classify)?;
    let mut entries = Vec::new();
    while let Some(entry) = stream.next().await.map_err(classify)? {
        entries.push(SearchEntry::construct(entry));
    }
    stream.finish().await.success().map_err(classify)?;
    Ok(entries)
}

async fn base_dn(ldap: &mut Ldap) -> Result<String, AuthError> {
    let root = search(
        ldap,
        "",
        Scope::Base,
        "(objectClass=*)",
        &["defaultNamingContext"],
    )
    .await?;
    root.first()
        .and_then(|e| text_attr(e, "defaultNamingContext"))
        .ok_or_else(|| AuthError::Unavailable("defaultNamingContext okunamadı".to_string()))
}

struct FoundUser {
    dn: String,
    sam: String,
    guid: String,
    email: String,
    member_of: Vec<String>,
}

/// Operator kendi AD kullanici adini ya da UPN'ini yazabilir (ADR-095 madde 1).
/// Deger `ldap_escape` ile kacirilir: `*` ya da `)` iceren girdi filtreyi bozmaz.
pub fn user_filter(username: &str) -> String {
    let escaped = ldap_escape(username);
    format!(
        "(&(objectClass=user)(objectCategory=person)\
         (|(sAMAccountName={escaped})(userPrincipalName={escaped})))"
    )
}

// Yonetilen kapsamla sinirlanmaz: operator (BT personeli) yonetilen personel
// OU'larinin disinda durabilir; kapsam yazma kurali, okuma degil (ADR-014).
async fn find_user(
    ldap: &mut Ldap,
    base: &str,
    username: &str,
) -> Result<Option<FoundUser>, AuthError> {
    let entries = search(
        ldap,
        base,
        Scope::Subtree,
        &user_filter(username),
        &USER_ATTRS,
    )
    .await?;
    if entries.len() > 1 {
        return Err(AuthError::Unavailable(format!(
            "aynı kullanıcı adına {} hesap eşleşti: dizinde tekilleştirilmeli",
            entries.len()
        )));
    }
    let Some(entry) = entries.first() else {
        return Ok(None);
    };
    let (Some(sam), Some(guid)) = (
        text_attr(entry, "sAMAccountName"),
        binary_attr(entry, "objectGUID").and_then(|b| guid_to_string(&b)),
    ) else {
        return Err(AuthError::Unavailable(
            "hesabın sAMAccountName/objectGUID değeri okunamadı".to_string(),
        ));
    };
    Ok(Some(FoundUser {
        dn: entry.dn.clone(),
        sam,
        guid,
        email: text_attr(entry, "mail").unwrap_or_default(),
        member_of: entry.attrs.get("memberOf").cloned().unwrap_or_default(),
    }))
}

// ADR-095 madde 2: yetki AD grup uyeliginden okunur, ic ice uyelik dahil.
// memberOf zinciri yukari yurunur; LDAP_MATCHING_RULE_IN_CHAIN'li tek arama
// yerine bu yol secildi, cunku ayni yurume (worker `privileged_group`) hem lab
// Samba'sinda hem gercek Windows AD'sinde dogrulanmis durumda.
async fn group_names(ldap: &mut Ldap, member_of: &[String]) -> Result<Vec<String>, AuthError> {
    let mut queue: Vec<String> = member_of.to_vec();
    let mut seen: HashSet<String> = HashSet::new();
    let mut names = Vec::new();
    while let Some(dn) = queue.pop() {
        if !seen.insert(dn.to_ascii_lowercase()) {
            continue;
        }
        let entries = search(ldap, &dn, Scope::Base, "(objectClass=group)", &GROUP_ATTRS).await?;
        let Some(entry) = entries.first() else {
            continue;
        };
        if let Some(cn) = text_attr(entry, "cn") {
            names.push(cn);
        }
        queue.extend(entry.attrs.get("memberOf").cloned().unwrap_or_default());
    }
    Ok(names)
}

fn text_attr(entry: &SearchEntry, name: &str) -> Option<String> {
    entry.attrs.get(name).and_then(|v| v.first()).cloned()
}

fn binary_attr(entry: &SearchEntry, name: &str) -> Option<Vec<u8>> {
    if let Some(value) = entry.bin_attrs.get(name).and_then(|v| v.first()) {
        return Some(value.clone());
    }
    entry
        .attrs
        .get(name)
        .and_then(|v| v.first())
        .map(|v| v.as_bytes().to_vec())
}

// objectGUID ham 16 bayt: ilk uc alan little-endian (docs/05 "tireli yazim").
fn guid_to_string(bytes: &[u8]) -> Option<String> {
    let b: &[u8; 16] = bytes.try_into().ok()?;
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lazy_unreachable_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(500))
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı")
    }

    // Bos parola dizine hic gitmez: veritabani erisilemez olsa da reddedilir.
    #[tokio::test]
    async fn an_empty_password_is_rejected_before_the_directory_is_touched() {
        let pool = lazy_unreachable_pool();
        let key = [1u8; crate::crypto::KEY_LEN];
        assert!(matches!(
            authenticate(&pool, &key, "operator", "").await,
            Err(AuthError::BadCredentials)
        ));
        assert!(matches!(
            authenticate(&pool, &key, "   ", "parola").await,
            Err(AuthError::BadCredentials)
        ));
        // Dolu girdi veritabanina gider ve orada takilir: kapi gercekten acilmis
        assert!(matches!(
            authenticate(&pool, &key, "operator", "parola").await,
            Err(AuthError::Unavailable(_))
        ));
    }

    #[test]
    fn the_user_filter_escapes_the_typed_name() {
        let filter = user_filter("a)(objectClass=*");
        assert!(
            filter.contains(r"a\29\28objectClass=\2a"),
            "kaçırılmadı: {filter}"
        );
        assert_eq!(filter.matches("sAMAccountName=").count(), 1);
        assert!(user_filter("ayse.yilmaz").contains("(sAMAccountName=ayse.yilmaz)"));
        assert!(user_filter("ayse.yilmaz@ornek.org")
            .contains("(userPrincipalName=ayse.yilmaz@ornek.org)"));
    }

    #[test]
    fn a_bare_host_becomes_an_ldaps_url_with_the_default_port() {
        assert_eq!(parse_urls("dc1.ornek.org"), ["ldaps://dc1.ornek.org:636"]);
        assert_eq!(
            parse_urls("dc1:3269; ldaps://dc2.ornek.org"),
            ["ldaps://dc1:3269", "ldaps://dc2.ornek.org"]
        );
        assert!(parse_urls("  ").is_empty());
    }

    #[test]
    fn a_bind_rejection_is_bad_credentials_and_a_dropped_connection_is_not() {
        let rejected = LdapError::LdapResult {
            result: ldap3::result::LdapResult {
                rc: INVALID_CREDENTIALS_RC,
                matched: String::new(),
                text: "80090308: LdapErr: DSID-0C09044E, data 52e".to_string(),
                refs: Vec::new(),
                ctrls: Vec::new(),
            },
        };
        assert!(matches!(classify(rejected), AuthError::BadCredentials));
        assert!(matches!(
            classify(LdapError::EndOfStream),
            AuthError::Unavailable(_)
        ));
        let no_such_object = LdapError::LdapResult {
            result: ldap3::result::LdapResult {
                rc: 32,
                matched: String::new(),
                text: "no such object".to_string(),
                refs: Vec::new(),
                ctrls: Vec::new(),
            },
        };
        assert!(matches!(
            classify(no_such_object),
            AuthError::Unavailable(_)
        ));
    }

    #[test]
    fn a_pem_without_a_certificate_block_is_refused() {
        assert!(read_pem_certs("/dev/null").is_err());
        assert!(read_pem_certs("/olmayan/ca.pem").is_err());
    }
}
// --- END FEATURE: ad-login ---
