-- OIDC girişi: admin grubu doğrulama işareti, kısa ömürlü authorize istek
-- kaydı (state/nonce/PKCE), operatör oturumu (ADR-005, ADR-065, ADR-073).

ALTER TABLE app_settings ADD COLUMN oidc_admin_verified_at TIMESTAMPTZ;

-- /oidc/login ile /oidc/callback arasinda backend stateless oldugundan
-- state/nonce/PKCE dogrulayiciyi tasimak icin kisa omurlu satir (ADR-073).
CREATE TABLE oidc_auth_requests (
    state TEXT PRIMARY KEY,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

-- OIDC ile girmis operatorun oturumu; bootstrap_sessions'tan bagimsiz
-- (bootstrap hesabi yalnizca Yapilandirma sayfasina erisir, ADR-068).
CREATE TABLE operator_sessions (
    token_hash TEXT PRIMARY KEY,
    subject TEXT NOT NULL,
    username TEXT NOT NULL,
    email TEXT NOT NULL,
    authorities TEXT[] NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
