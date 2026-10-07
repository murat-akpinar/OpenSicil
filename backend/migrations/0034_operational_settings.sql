-- Isletme ayarlari ekrandan verilir (ADR-131): anahtar adlari eski ortam degiskeni
-- adlarinin birebir aynisi. Sir girmez (sirlar app_settings'te, AEAD). Kapsam
-- dortlusu bos baslar: migration .env'i okumaz, kurulum OU'lari ekrandan girer.
CREATE TABLE operational_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ,
    updated_by TEXT
);

INSERT INTO operational_settings (key, value) VALUES
    ('AD_MANAGED_USER_OUS', ''),
    ('AD_MANAGED_GROUP_OUS', ''),
    ('AD_PASSIVE_OU', ''),
    ('ZIMBRA_MANAGED_DOMAINS', ''),
    ('USERNAME_TEMPLATE', '{given_first}.{surname}'),
    ('EMAIL_LOCAL_TEMPLATE', '{given_first}.{surname}'),
    ('DRY_RUN', 'true'),
    ('FIRST_LOGIN_CHANGE_REQUIRED', 'true'),
    ('RECONCILE_SCAN_AT', '02:00'),
    ('CHANGE_SET_THRESHOLD', '10'),
    ('OWNERSHIP_MODE_ENABLED', 'false'),
    ('SENSITIVE_MAPPING_ENABLED', 'false'),
    ('HOURLY_DESTRUCTIVE_LIMIT', '50'),
    ('HOURLY_GRANT_LIMIT', '50'),
    ('HOURLY_FIRST_PASSWORD_LIMIT', '50'),
    ('EMERGENCY_QUOTA', '5'),
    ('TZ', 'Europe/Istanbul');
