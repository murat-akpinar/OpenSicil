// --- START FEATURE: oidc-login ---
// OIDC yonetim girisi (ADR-005, ADR-065, ADR-073). Client, discovery sonucuyla
// her istekte yeniden kurulur (AppState'te saklanmaz): 4.0'daki typestate
// generic'lerini `Client` type alias'iyla bir kez adlandirmak, her cagri
// sitesinde tekrarlamaktan daha az kod (ADR-073).

use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreIdTokenClaims, CoreProviderMetadata,
};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
};
use sqlx::PgPool;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use base64::Engine;

pub const AUTH_REQUEST_LIFETIME_MINUTES: i64 = 10;

// Alti sabit yonetim grubu -> yetki eslemesi (ADR-005, altincisi ADR-019).
// Grup adlari "OpenSicil-" onekiyle (ADR-063 urun adini degistirdi).
const GROUP_AUTHORITIES: &[(&str, &str)] = &[
    ("OpenSicil-Auditors", "auditor"),
    ("OpenSicil-HR", "hr"),
    ("OpenSicil-RoleAdmins", "role_admin"),
    ("OpenSicil-PII", "pii_reader"),
    ("OpenSicil-Admins", "admin"),
    ("OpenSicil-Helpdesk", "helpdesk"),
];

pub const ADMIN_AUTHORITY: &str = "admin";

// Bilinmeyen grup adi yok sayilir; grup adlari kurulum ayari yapmak (ADR-005'in
// dedigi gibi) bu kutucugun kapsami disinda, sabit liste yeterli (kapsam disi
// onerisi ozette).
pub fn authorities_for_groups(groups: &[String]) -> Vec<String> {
    GROUP_AUTHORITIES
        .iter()
        .filter(|(group, _)| groups.iter().any(|g| g == group))
        .map(|(_, authority)| (*authority).to_string())
        .collect()
}

#[derive(Debug)]
pub enum OidcError {
    NotConfigured,
    Configuration(String),
    Discovery(String),
    InvalidState,
    TokenExchange(String),
    MissingIdToken,
    ClaimsVerification(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for OidcError {
    fn from(e: sqlx::Error) -> Self {
        OidcError::Db(e)
    }
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::NotConfigured => write!(f, "OIDC henüz yapılandırılmadı"),
            OidcError::Configuration(e) => write!(f, "yapılandırma hatası: {e}"),
            OidcError::Discovery(e) => write!(f, "discovery başarısız: {e}"),
            OidcError::InvalidState => write!(f, "state geçersiz ya da süresi geçmiş"),
            OidcError::TokenExchange(e) => write!(f, "token değişimi başarısız: {e}"),
            OidcError::MissingIdToken => write!(f, "sunucu id_token döndürmedi"),
            OidcError::ClaimsVerification(e) => write!(f, "id_token doğrulanamadı: {e}"),
            OidcError::Db(e) => write!(f, "veritabanı hatası: {e}"),
        }
    }
}

pub struct LoginResult {
    pub subject: String,
    pub username: String,
    pub email: String,
    pub authorities: Vec<String>,
}

// from_provider_metadata()'nin urettigi somut typestate: auth endpoint her
// zaman set (discovery zorunlu kilar), token/userinfo discovery'de var olabilir
// de olmayabilir de (EndpointMaybeSet) — ADR-073.
type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

fn http_client() -> openidconnect::reqwest::Client {
    // Yonlendirme takibi kapali: openidconnect'in kendi SSRF uyarisi (ADR-073).
    openidconnect::reqwest::Client::builder()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .expect("http istemcisi kurulamadı")
}

async fn load_credentials(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
) -> Result<crate::settings::OidcCredentials, OidcError> {
    crate::settings::load_oidc_credentials(pool, aead_key)
        .await
        .map_err(OidcError::Configuration)?
        .ok_or(OidcError::NotConfigured)
}

async fn build_client(
    creds: &crate::settings::OidcCredentials,
    redirect_uri: &str,
) -> Result<Client, OidcError> {
    let issuer = IssuerUrl::new(creds.issuer.clone())
        .map_err(|e| OidcError::Configuration(e.to_string()))?;
    let redirect = RedirectUrl::new(redirect_uri.to_string())
        .map_err(|e| OidcError::Configuration(e.to_string()))?;
    let http = http_client();
    let metadata = CoreProviderMetadata::discover_async(issuer, &http)
        .await
        .map_err(|e| OidcError::Discovery(e.to_string()))?;
    Ok(CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(creds.client_id.clone()),
        Some(ClientSecret::new(creds.client_secret.clone())),
    )
    .set_redirect_uri(redirect))
}

pub async fn login_redirect(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    redirect_uri: &str,
) -> Result<String, OidcError> {
    let creds = load_credentials(pool, aead_key).await?;
    let client = build_client(&creds, redirect_uri).await?;

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_token, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(pkce_challenge)
        .url();

    store_auth_request(
        pool,
        csrf_token.secret(),
        nonce.secret(),
        pkce_verifier.secret(),
    )
    .await?;

    Ok(auth_url.to_string())
}

async fn store_auth_request(
    pool: &PgPool,
    state: &str,
    nonce: &str,
    pkce_verifier: &str,
) -> Result<(), OidcError> {
    sqlx::query(
        "INSERT INTO oidc_auth_requests (state, nonce, pkce_verifier, expires_at) \
         VALUES ($1, $2, $3, now() + make_interval(mins => $4))",
    )
    .bind(state)
    .bind(nonce)
    .bind(pkce_verifier)
    .bind(AUTH_REQUEST_LIFETIME_MINUTES as i32)
    .execute(pool)
    .await?;
    Ok(())
}

// Tek kullanimlik: state satiri burada silinir, tekrar oynatma (replay) ayni
// state ile ikinci kez calismaz. Suresi gecmis satir de eslenmez.
async fn consume_auth_request(pool: &PgPool, state: &str) -> Result<(String, String), OidcError> {
    let row: Option<(String, String)> = sqlx::query_as(
        "DELETE FROM oidc_auth_requests WHERE state = $1 AND expires_at > now() \
         RETURNING nonce, pkce_verifier",
    )
    .bind(state)
    .fetch_optional(pool)
    .await?;
    row.ok_or(OidcError::InvalidState)
}

pub async fn handle_callback(
    pool: &PgPool,
    aead_key: &[u8; crate::crypto::KEY_LEN],
    redirect_uri: &str,
    code: String,
    state: String,
) -> Result<LoginResult, OidcError> {
    let (nonce_secret, pkce_verifier_secret) = consume_auth_request(pool, &state).await?;

    let creds = load_credentials(pool, aead_key).await?;
    let client = build_client(&creds, redirect_uri).await?;
    let http = http_client();

    let token_response = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| OidcError::TokenExchange(e.to_string()))?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier_secret))
        .request_async(&http)
        .await
        .map_err(|e| OidcError::TokenExchange(e.to_string()))?;

    let id_token = token_response
        .extra_fields()
        .id_token()
        .ok_or(OidcError::MissingIdToken)?;
    let verifier = client.id_token_verifier();
    let nonce = Nonce::new(nonce_secret);
    let claims = id_token
        .claims(&verifier, &nonce)
        .map_err(|e| OidcError::ClaimsVerification(e.to_string()))?;

    // id_token imzasi yukarida dogrulandi; "groups" EmptyAdditionalClaims'in
    // disinda oldugu icin ayni (zaten dogrulanmis) token'in govdesini bir kez
    // daha, yalnizca bu ozel claim'i okumak icin ayristiriyoruz (ADR-073).
    Ok(login_result_from_claims(claims, &id_token.to_string()))
}

fn login_result_from_claims(claims: &CoreIdTokenClaims, compact_jwt: &str) -> LoginResult {
    let subject = claims.subject().as_str().to_string();
    let username = claims
        .preferred_username()
        .map(|u| u.as_str().to_string())
        .unwrap_or_else(|| subject.clone());
    let email = claims
        .email()
        .map(|e| e.as_str().to_string())
        .unwrap_or_default();
    let authorities = authorities_for_groups(&extract_groups(compact_jwt));

    LoginResult {
        subject,
        username,
        email,
        authorities,
    }
}

fn extract_groups(compact_jwt: &str) -> Vec<String> {
    let Some(payload_b64) = compact_jwt.split('.').nth(1) else {
        return Vec::new();
    };
    let Ok(payload_bytes) = BASE64_URL.decode(payload_b64) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload_bytes) else {
        return Vec::new();
    };
    value
        .get("groups")
        .and_then(|g| g.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn is_configured(settings: &crate::settings::AppSettings) -> bool {
    !settings.oidc_issuer.is_empty()
        && !settings.oidc_client_id.is_empty()
        && settings.oidc_client_secret_set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_groups_to_authorities() {
        let groups = vec!["OpenSicil-Admins".to_string(), "OpenSicil-HR".to_string()];
        let mut authorities = authorities_for_groups(&groups);
        authorities.sort();
        assert_eq!(authorities, vec!["admin".to_string(), "hr".to_string()]);
    }

    #[test]
    fn ignores_unknown_groups() {
        let groups = vec!["Domain Users".to_string(), "Random-Group".to_string()];
        assert!(authorities_for_groups(&groups).is_empty());
    }

    #[test]
    fn empty_groups_yield_no_authorities() {
        assert!(authorities_for_groups(&[]).is_empty());
    }

    #[test]
    fn extract_groups_reads_groups_claim_from_jwt_payload() {
        let payload = serde_json::json!({"groups": ["OpenSicil-Admins", "OpenSicil-HR"]});
        let payload_b64 = BASE64_URL.encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("header.{payload_b64}.signature");
        assert_eq!(
            extract_groups(&jwt),
            vec!["OpenSicil-Admins".to_string(), "OpenSicil-HR".to_string()]
        );
    }

    #[test]
    fn extract_groups_returns_empty_without_groups_claim() {
        let payload = serde_json::json!({"sub": "abc"});
        let payload_b64 = BASE64_URL.encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("header.{payload_b64}.signature");
        assert!(extract_groups(&jwt).is_empty());
    }

    #[test]
    fn extract_groups_returns_empty_on_malformed_jwt() {
        assert!(extract_groups("not-a-jwt").is_empty());
        assert!(extract_groups("a.b").is_empty());
    }
}
// --- END FEATURE: oidc-login ---
