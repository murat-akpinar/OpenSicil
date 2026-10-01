-- ADR-095: tek oturum mekanizmasi. Yerel hesap da artik operator_sessions
-- satiri uretir (Sistem yoneticisi yetkisiyle), bootstrap_sessions kalkar.
-- Oturumun hangi kapidan acildigi denetimde ve ekranda gorunur.

ALTER TABLE operator_sessions
    ADD COLUMN auth_source TEXT NOT NULL DEFAULT 'oidc'
        CHECK (auth_source IN ('ad', 'local', 'oidc'));

-- Varsayilan yalnizca mevcut satirlari tasimak icindi (hepsi OIDC'den gelmisti);
-- bundan sonra her oturum kapisini acikca yazar.
ALTER TABLE operator_sessions ALTER COLUMN auth_source DROP DEFAULT;

-- Yerel kapinin kaba kuvvet korumasi (ADR-095 madde 3): AD kapisinda kilitleme
-- AD'nin kendi politikasi, bizde sayac tutulmaz.
ALTER TABLE bootstrap_account
    ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN locked_until TIMESTAMPTZ;

DROP TABLE bootstrap_sessions;
