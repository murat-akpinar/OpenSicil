-- Ilk parola teslimi (ADR-009/019/036/046/056/085): operator ister, worker
-- hesabi dogrulayip parolayi yazar ve AEAD ile sifreli birakir, backend bir kez
-- gosterip bosaltir; gosterilmeyen deger 10 dk sonra zamanlayiciyla bosaltilir.
-- Duz metin parola hic saklanmaz. Izinler SERVICE_GRANTS'ta.
CREATE TABLE first_passwords (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    identity_id BIGINT NOT NULL REFERENCES identities (id),
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    requested_by TEXT NOT NULL,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- worker yazar: sifreli parola (basta anahtar surumu) ya da red nedeni
    password_enc BYTEA,
    issued_at TIMESTAMPTZ,
    error TEXT,
    -- backend yazar: gosterildi, deger bosaltildi
    shown_at TIMESTAMPTZ
);

CREATE INDEX first_passwords_pending_idx ON first_passwords (identity_id, target_system_id)
    WHERE issued_at IS NULL AND error IS NULL;

-- ADR-019 isaret kapaliyken: worker'in yazdigi parolanin pwdLastSet damgasi;
-- sonraki istekte esitse kullanici henuz degistirmemistir.
ALTER TABLE account_links ADD COLUMN first_password_pwd_last_set TEXT;
