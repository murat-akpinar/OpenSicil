-- Toplu yonetime alma (ADR-018, 043, 051, 087): okuma seridinin hesapladigi
-- "yonetime alinirsa ne degisir" farki gozlem baglantisinda saklanir; worker yazar,
-- backend okur. applies: yetki ya da hesap durumu farki var (esige giren, ADR-037/043);
-- NULL = hic hesaplanmadi.
ALTER TABLE account_links
    ADD COLUMN observed_diff TEXT,
    ADD COLUMN observed_diff_applies BOOLEAN,
    ADD COLUMN observed_diff_at TIMESTAMPTZ;

-- Okuma seridinin ucuncu turu (ADR-094 sonuclari)
ALTER TABLE read_jobs DROP CONSTRAINT read_jobs_kind_check;
ALTER TABLE read_jobs
    ADD CONSTRAINT read_jobs_kind_check CHECK (kind IN ('catalog_refresh', 'reconcile', 'manage_diff'));

-- Esigi asan toplu yonetime alma secimi: onaya kadar bekler, uygulanan/reddedilen silinir.
CREATE TABLE manage_batches (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    identity_ids BIGINT[] NOT NULL,
    -- sahnelenirken esige giren kimlik sayisi; onayda yeniden hesaplanir (ADR-055 madde 1)
    affected INTEGER NOT NULL,
    by_subject TEXT NOT NULL,
    by_username TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
