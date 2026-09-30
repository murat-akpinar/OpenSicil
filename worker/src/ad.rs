// --- START FEATURE: ad-connector ---
// AD connector okuma yolu (docs/05 Baglanti, Katalog, Yasakli gruplar; ADR-014,
// 016, 057, 077): yalnizca LDAPS, sertifika dogrulamasi kapatilamaz (CA dosyayla),
// bir is tek DC'ye tek baglantiyla konusur (siradaki DC yalnizca baglanamayinca),
// her arama EntriesOnly + sayfali, objectGUID tireli RFC 4122 yazimla tutulur.
// Katalog: yonetilen OU'lar ve yonetilen grup OU'larindaki gruplar; yasakli
// gruplar (yerlesik SID/RID, adminCount, OpenSicil yonetim gruplari ve bunlarin
// ic ice uyeleri) kataloga hic girmez.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
use ldap3::{ldap_escape, Ldap, LdapConnAsync, LdapConnSettings, LdapError, Scope, SearchEntry};
use sqlx::PgPool;

use crate::writes::WriteError;

const LDAPS_PORT: u16 = 636;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PAGE_SIZE: i32 = 500;
const NESTED_MEMBER_RULE: &str = "1.2.840.113556.1.4.1941";
// docs/05 Yasakli gruplar: yerlesik gruplar sabit SID, domain gruplari RID ile
const BUILTIN_FORBIDDEN_SIDS: [&str; 6] = [
    "S-1-5-32-544",
    "S-1-5-32-548",
    "S-1-5-32-549",
    "S-1-5-32-550",
    "S-1-5-32-551",
    "S-1-5-32-552",
];
const DOMAIN_FORBIDDEN_RIDS: [u32; 9] = [512, 516, 517, 518, 519, 520, 521, 526, 527];
// ADR-005/077: backend oidc.rs'teki listeyle ayni
const MANAGEMENT_GROUPS: [&str; 6] = [
    "OpenSicil-Admins",
    "OpenSicil-HR",
    "OpenSicil-RoleAdmins",
    "OpenSicil-PII",
    "OpenSicil-Auditors",
    "OpenSicil-Helpdesk",
];
const FORBIDDEN_CONTAINERS: [&str; 3] = ["cn=users,", "cn=builtin,", "ou=domain controllers,"];

pub struct AdConfig {
    pub urls: Vec<String>,
    pub bind_dn: String,
    pub password: String,
    pub ca_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedScope {
    pub user_ous: Vec<String>,
    pub passive_ou: Option<String>,
    pub group_ous: Vec<String>,
}

fn split_dns(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

// ADR-077 kapsam degiskenleri; CN=Users, CN=Builtin, OU=Domain Controllers kapsam
// olamaz, verilirse connector baslamaz (docs/05).
pub fn parse_scope(lookup: impl Fn(&str) -> Option<String>) -> Result<ManagedScope, String> {
    let get = |name: &str| lookup(name).map(|v| split_dns(&v)).unwrap_or_default();
    let scope = ManagedScope {
        user_ous: get("AD_MANAGED_USER_OUS"),
        passive_ou: get("AD_PASSIVE_OU").into_iter().next(),
        group_ous: get("AD_MANAGED_GROUP_OUS"),
    };
    if scope.user_ous.is_empty() || scope.group_ous.is_empty() {
        return Err("AD_MANAGED_USER_OUS ve AD_MANAGED_GROUP_OUS boş olamaz".to_string());
    }
    for dn in scope
        .user_ous
        .iter()
        .chain(&scope.group_ous)
        .chain(&scope.passive_ou)
    {
        let lower = dn.to_ascii_lowercase();
        if FORBIDDEN_CONTAINERS.iter().any(|c| lower.starts_with(c)) {
            return Err(format!("kapsam olamaz: {dn} (docs/05)"));
        }
    }
    Ok(scope)
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

// Baglanti bilgisi Yapilandirma sayfasindan (ADR-068), parola AEAD ile cozulur.
pub async fn load_config(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    ca_file: Option<&str>,
) -> Result<Option<AdConfig>, String> {
    let row: (String, String, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT ad_host, ad_bind_dn, ad_service_password_enc FROM app_settings WHERE id = TRUE",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("AD ayarları okunamadı: {e}"))?;
    let (ad_host, bind_dn, password_enc) = row;
    if ad_host.trim().is_empty() {
        return Ok(None);
    }
    let Some(password_enc) = password_enc else {
        return Err("AD servis hesabı parolası girilmemiş".to_string());
    };
    let password = String::from_utf8(crate::crypto::decrypt(aead_key, &password_enc)?)
        .map_err(|_| "AD parolası UTF-8 değil".to_string())?;
    let ca_file = ca_file
        .ok_or("AD_CA_FILE ortam değişkeni eksik")?
        .to_string();
    Ok(Some(AdConfig {
        urls: parse_urls(&ad_host),
        bind_dn,
        password,
        ca_file,
    }))
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

fn tls_settings(ca_file: &str) -> Result<LdapConnSettings, WriteError> {
    let mut roots = rustls::RootCertStore::empty();
    for der in read_pem_certs(ca_file).map_err(WriteError::Failed)? {
        roots
            .add(der.into())
            .map_err(|e| WriteError::Failed(format!("CA sertifikası geçersiz: {e}")))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| WriteError::Failed(format!("TLS yapılandırması kurulamadı: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(LdapConnSettings::new()
        .set_config(Arc::new(config))
        .set_conn_timeout(CONNECT_TIMEOUT))
}

// TCP/TLS/zaman asimi/baglanti kopmasi hedef arizasidir (deneme tuketmez);
// LDAP sonuc kodu nesne duzeyi hatadir (ADR-052).
pub fn classify(error: LdapError) -> WriteError {
    match error {
        LdapError::Io { .. } | LdapError::Timeout { .. } | LdapError::EndOfStream => {
            WriteError::Unreachable(error.to_string())
        }
        other => WriteError::Failed(other.to_string()),
    }
}

// Sirayla DC dener; ilk basarili bind'i doner. Yalnizca baglanti hatasi sonraki
// DC'ye gecirir; bind reddi (yanlis parola) hemen Failed'dir.
pub async fn connect(cfg: &AdConfig) -> Result<Ldap, WriteError> {
    let mut last = WriteError::Unreachable("DC adresi verilmedi".to_string());
    for url in &cfg.urls {
        let settings = tls_settings(&cfg.ca_file)?;
        match LdapConnAsync::with_settings(settings, url).await {
            Ok((conn, mut ldap)) => {
                ldap3::drive!(conn);
                let bound = ldap
                    .simple_bind(&cfg.bind_dn, &cfg.password)
                    .await
                    .map_err(classify)?
                    .success()
                    .map_err(classify);
                match bound {
                    Ok(_) => return Ok(ldap),
                    Err(e) => return Err(e),
                }
            }
            Err(e) => {
                eprintln!("ad: {url} bağlanamadı: {e}");
                last = classify(e);
            }
        }
    }
    Err(last)
}

// Her arama EntriesOnly (yonlendirmeler ayiklanir, ldap3 #156) + sayfali (docs/05).
pub async fn search(
    ldap: &mut Ldap,
    base: &str,
    scope: Scope,
    filter: &str,
    attrs: &[&str],
) -> Result<Vec<SearchEntry>, WriteError> {
    let adapters: Vec<Box<dyn Adapter<&str, &[&str]>>> = vec![
        Box::new(EntriesOnly::new()),
        Box::new(PagedResults::new(PAGE_SIZE)),
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

// objectGUID ham 16 bayt: ilk uc alan little-endian, kalan sekiz bayt sirayla
// (docs/05 "tireli yazim"). Yalnizca bu yazim kullanilir.
pub fn guid_to_string(bytes: &[u8]) -> Option<String> {
    let b: &[u8; 16] = bytes.try_into().ok()?;
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

// objectSid: revizyon(1) + alt otorite sayisi(1) + otorite(6, big-endian) + alt otoriteler(4'er, little-endian)
pub fn sid_to_string(bytes: &[u8]) -> Option<String> {
    let (&revision, rest) = bytes.split_first()?;
    let (&count, rest) = rest.split_first()?;
    if rest.len() < 6 + 4 * count as usize {
        return None;
    }
    let authority = rest[..6]
        .iter()
        .fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
    let mut sid = format!("S-{revision}-{authority}");
    for chunk in rest[6..6 + 4 * count as usize].chunks(4) {
        let sub = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        sid.push_str(&format!("-{sub}"));
    }
    Some(sid)
}

fn binary_attr(entry: &SearchEntry, name: &str) -> Option<Vec<u8>> {
    // ldap3 gecerli UTF-8 olan degerleri attrs'e koyar; GUID/SID rastgele bayt
    // oldugu icin iki haritaya da bakilir (docs/05).
    entry
        .bin_attrs
        .get(name)
        .and_then(|v| v.first().cloned())
        .or_else(|| {
            entry
                .attrs
                .get(name)
                .and_then(|v| v.first())
                .map(|s| s.as_bytes().to_vec())
        })
}

fn text_attr(entry: &SearchEntry, name: &str) -> Option<String> {
    entry.attrs.get(name).and_then(|v| v.first()).cloned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryOu {
    pub dn: String,
    pub name: String,
    pub guid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryGroup {
    pub dn: String,
    pub name: String,
    pub guid: String,
    pub sid: String,
    pub admin_count: bool,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub ous: Vec<DirectoryOu>,
    /// Yonetilen grup OU'larindaki, yasakli olmayan gruplar (katalog)
    pub groups: Vec<DirectoryGroup>,
    /// Yasakli gruplar (bilgi ve test icin; kataloga girmez)
    pub forbidden: Vec<DirectoryGroup>,
}

pub fn is_well_known_forbidden(sid: &str) -> bool {
    if BUILTIN_FORBIDDEN_SIDS.contains(&sid) {
        return true;
    }
    sid.starts_with("S-1-5-21-")
        && sid
            .rsplit('-')
            .next()
            .and_then(|rid| rid.parse::<u32>().ok())
            .is_some_and(|rid| DOMAIN_FORBIDDEN_RIDS.contains(&rid))
}

fn is_forbidden_seed(group: &DirectoryGroup) -> bool {
    group.admin_count
        || is_well_known_forbidden(&group.sid)
        || MANAGEMENT_GROUPS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&group.name))
}

// DN kapsam OU'larindan birinin altinda mi (buyuk/kucuk harf duyarsiz).
pub fn under_any(dn: &str, ous: &[String]) -> bool {
    let lower = dn.to_ascii_lowercase();
    ous.iter()
        .any(|ou| lower.ends_with(&format!(",{}", ou.to_ascii_lowercase())))
}

fn to_group(entry: &SearchEntry) -> Option<DirectoryGroup> {
    Some(DirectoryGroup {
        dn: entry.dn.clone(),
        name: text_attr(entry, "cn")?,
        guid: guid_to_string(&binary_attr(entry, "objectGUID")?)?,
        sid: sid_to_string(&binary_attr(entry, "objectSid")?)?,
        admin_count: text_attr(entry, "adminCount").is_some_and(|v| v.trim() != "0"),
    })
}

async fn base_dn(ldap: &mut Ldap) -> Result<String, WriteError> {
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
        .ok_or_else(|| WriteError::Failed("defaultNamingContext okunamadı".to_string()))
}

async fn resolve_ous(
    ldap: &mut Ldap,
    scope: &ManagedScope,
) -> Result<Vec<DirectoryOu>, WriteError> {
    let mut ous = Vec::new();
    for dn in scope
        .user_ous
        .iter()
        .chain(&scope.passive_ou)
        .chain(&scope.group_ous)
    {
        let found = search(
            ldap,
            dn,
            Scope::Base,
            "(objectClass=organizationalUnit)",
            &["ou", "objectGUID"],
        )
        .await
        .map_err(|e| match e {
            WriteError::Failed(msg) => {
                WriteError::Failed(format!("kapsam OU'su bulunamadı ({dn}): {msg}"))
            }
            other => other,
        })?;
        let entry = found
            .first()
            .ok_or_else(|| WriteError::Failed(format!("kapsam OU'su bulunamadı: {dn}")))?;
        ous.push(DirectoryOu {
            dn: entry.dn.clone(),
            name: text_attr(entry, "ou").unwrap_or_default(),
            guid: guid_to_string(&binary_attr(entry, "objectGUID").unwrap_or_default())
                .ok_or_else(|| WriteError::Failed(format!("OU objectGUID okunamadı: {dn}")))?,
        });
    }
    Ok(ous)
}

// Yasakli tohumlarin ic ice uyesi olan gruplar tek aramada (LDAP_MATCHING_RULE_IN_CHAIN).
async fn nested_members_of(
    ldap: &mut Ldap,
    base: &str,
    seed_dn: &str,
) -> Result<Vec<DirectoryGroup>, WriteError> {
    let filter = format!(
        "(&(objectClass=group)(memberOf:{NESTED_MEMBER_RULE}:={}))",
        ldap_escape(seed_dn)
    );
    let entries = search(ldap, base, Scope::Subtree, &filter, &GROUP_ATTRS).await?;
    Ok(entries.iter().filter_map(to_group).collect())
}

const GROUP_ATTRS: [&str; 4] = ["cn", "objectGUID", "objectSid", "adminCount"];

pub async fn read_catalog(ldap: &mut Ldap, scope: &ManagedScope) -> Result<Snapshot, WriteError> {
    let base = base_dn(ldap).await?;
    let ous = resolve_ous(ldap, scope).await?;
    let all_groups: Vec<DirectoryGroup> = search(
        ldap,
        &base,
        Scope::Subtree,
        "(objectClass=group)",
        &GROUP_ATTRS,
    )
    .await?
    .iter()
    .filter_map(to_group)
    .collect();
    let mut forbidden: Vec<DirectoryGroup> = all_groups
        .iter()
        .filter(|g| is_forbidden_seed(g))
        .cloned()
        .collect();
    let seeds: Vec<String> = forbidden.iter().map(|g| g.dn.clone()).collect();
    for seed in seeds {
        for nested in nested_members_of(ldap, &base, &seed).await? {
            if !forbidden.iter().any(|f| f.guid == nested.guid) {
                forbidden.push(nested);
            }
        }
    }
    let groups = all_groups
        .into_iter()
        .filter(|g| under_any(&g.dn, &scope.group_ous))
        .filter(|g| !forbidden.iter().any(|f| f.guid == g.guid))
        .collect();
    Ok(Snapshot {
        ous,
        groups,
        forbidden,
    })
}
// --- END FEATURE: ad-connector ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_uses_mixed_endian_dashed_form() {
        let raw = [
            0x04, 0x03, 0x02, 0x01, 0x06, 0x05, 0x08, 0x07, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ];
        assert_eq!(
            guid_to_string(&raw).as_deref(),
            Some("01020304-0506-0708-090a-0b0c0d0e0f10")
        );
        assert_eq!(guid_to_string(&raw[..15]), None);
    }

    #[test]
    fn sid_decodes_domain_admins() {
        // S-1-5-21-1-2-3-512
        let mut raw = vec![1, 5, 0, 0, 0, 0, 0, 5];
        for sub in [21u32, 1, 2, 3, 512] {
            raw.extend_from_slice(&sub.to_le_bytes());
        }
        let sid = sid_to_string(&raw).unwrap();
        assert_eq!(sid, "S-1-5-21-1-2-3-512");
        assert!(is_well_known_forbidden(&sid));
        assert!(is_well_known_forbidden("S-1-5-32-544"));
        assert!(!is_well_known_forbidden("S-1-5-21-1-2-3-1105"));
        assert!(!is_well_known_forbidden("S-1-5-21-1-2-3-51200"));
        assert_eq!(sid_to_string(&raw[..10]), None);
    }

    #[test]
    fn urls_default_to_ldaps_636_in_order() {
        assert_eq!(
            parse_urls("dc1; dc2:6360,ldaps://dc3"),
            vec!["ldaps://dc1:636", "ldaps://dc2:6360", "ldaps://dc3"]
        );
    }

    #[test]
    fn scope_rejects_builtin_containers_and_requires_both_lists() {
        let env = |user: &'static str, group: &'static str| {
            move |name: &str| match name {
                "AD_MANAGED_USER_OUS" => Some(user.to_string()),
                "AD_MANAGED_GROUP_OUS" => Some(group.to_string()),
                "AD_PASSIVE_OU" => Some("OU=Pasif,OU=Personel,DC=x".to_string()),
                _ => None,
            }
        };
        let ok = parse_scope(env("OU=Personel,DC=x", "OU=Gruplar,DC=x;OU=Gruplar2,DC=x")).unwrap();
        assert_eq!(ok.group_ous.len(), 2);
        assert_eq!(ok.passive_ou.as_deref(), Some("OU=Pasif,OU=Personel,DC=x"));
        assert!(parse_scope(env("OU=Personel,DC=x", "CN=Users,DC=x")).is_err());
        assert!(parse_scope(env("", "OU=Gruplar,DC=x")).is_err());
    }

    #[test]
    fn under_any_is_case_insensitive_suffix_match() {
        let ous = vec!["OU=Gruplar,DC=opensicil,DC=lab".to_string()];
        assert!(under_any("CN=GG-VPN,ou=gruplar,dc=opensicil,dc=lab", &ous));
        assert!(!under_any(
            "CN=GG-VPN,OU=Disarida,DC=opensicil,DC=lab",
            &ous
        ));
        assert!(
            !under_any("OU=Gruplar,DC=opensicil,DC=lab", &ous),
            "OU'nun kendisi altında değil"
        );
    }

    #[test]
    fn pem_reader_extracts_der_blocks() {
        let dir = std::env::temp_dir().join(format!("opensicil-ca-{}", std::process::id()));
        std::fs::write(
            &dir,
            "junk\n-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let certs = read_pem_certs(dir.to_str().unwrap()).unwrap();
        assert_eq!(certs, vec![vec![1, 2, 3]]);
        std::fs::remove_file(&dir).unwrap();
        assert!(read_pem_certs("/yok/boyle/dosya").is_err());
    }

    // Lab Samba AD gerektirir: docs/09 lab bolumu (gen-tls.sh + seed.sh) ve
    // AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE.
    #[tokio::test]
    #[ignore = "lab Samba AD gerektirir: AD_LAB_URL, AD_LAB_BIND_DN, AD_LAB_PASSWORD, AD_CA_FILE ile çalıştır"]
    async fn reads_lab_catalog_and_excludes_forbidden_groups() {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} ayarlanmalı"));
        let cfg = AdConfig {
            urls: parse_urls(&var("AD_LAB_URL")),
            bind_dn: var("AD_LAB_BIND_DN"),
            password: var("AD_LAB_PASSWORD"),
            ca_file: var("AD_CA_FILE"),
        };
        let scope = ManagedScope {
            user_ous: vec!["OU=Personel,DC=opensicil,DC=lab".to_string()],
            passive_ou: Some("OU=Pasif,OU=Personel,DC=opensicil,DC=lab".to_string()),
            group_ous: vec!["OU=Gruplar,DC=opensicil,DC=lab".to_string()],
        };
        let mut ldap = connect(&cfg).await.expect("lab AD'ye bağlanılamadı");
        let snapshot = read_catalog(&mut ldap, &scope)
            .await
            .expect("katalog okunamadı");
        let names = |v: &[DirectoryGroup]| v.iter().map(|g| g.name.clone()).collect::<Vec<_>>();
        let groups = names(&snapshot.groups);
        let forbidden = names(&snapshot.forbidden);
        assert_eq!(snapshot.ous.len(), 3, "üç kapsam OU'su çözülmeli");
        for expected in ["GG-Internet", "GG-VPN", "GG-Nobet"] {
            assert!(
                groups.contains(&expected.to_string()),
                "{expected} katalogda olmalı: {groups:?}"
            );
        }
        assert!(
            !groups.contains(&"GG-Disarida".to_string()),
            "kapsam dışı grup katalogda olmamalı"
        );
        for bad in [
            "Domain Admins",
            "GG-Nested-Admin",
            "GG-AdminCount",
            "OpenSicil-Admins",
        ] {
            assert!(
                forbidden.contains(&bad.to_string()),
                "{bad} yasaklı olmalı: {forbidden:?}"
            );
            assert!(
                !groups.contains(&bad.to_string()),
                "{bad} katalogda olmamalı"
            );
        }
        assert!(snapshot
            .groups
            .iter()
            .all(|g| g.guid.len() == 36 && g.sid.starts_with("S-1-5-21-")));

        let mut wrong = AdConfig {
            password: "yanlis".to_string(),
            ..cfg
        };
        assert!(
            matches!(connect(&wrong).await, Err(WriteError::Failed(_))),
            "bind reddi Failed"
        );
        wrong.urls = vec!["ldaps://127.0.0.1:1".to_string()];
        assert!(
            matches!(connect(&wrong).await, Err(WriteError::Unreachable(_))),
            "bağlanamama Unreachable"
        );
        ldap.unbind().await.ok();
    }
}
