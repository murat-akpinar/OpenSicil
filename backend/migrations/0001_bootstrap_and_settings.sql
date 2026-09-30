-- Yerel bootstrap hesabi, oturumlari ve sifreli baglanti ayarlari (ADR-068).

-- Tek satirlik tablo: id sabit TRUE, ikinci satir eklenemez (CHECK).
CREATE TABLE bootstrap_account (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    username TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    must_change_password BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE bootstrap_sessions (
    token_hash TEXT PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL
);

-- AD/Zimbra/OIDC baglanti ayarlari; *_enc alanlari chacha20poly1305 ile
-- sifrelenmis (nonce + metin), duz metin sir hicbir zaman DB'ye yazilmaz.
CREATE TABLE app_settings (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    ad_host TEXT NOT NULL DEFAULT '',
    ad_bind_dn TEXT NOT NULL DEFAULT '',
    ad_service_password_enc BYTEA,
    zimbra_url TEXT NOT NULL DEFAULT '',
    zimbra_admin_password_enc BYTEA,
    oidc_issuer TEXT NOT NULL DEFAULT '',
    oidc_client_id TEXT NOT NULL DEFAULT '',
    oidc_client_secret_enc BYTEA
);

INSERT INTO app_settings (id) VALUES (TRUE);
