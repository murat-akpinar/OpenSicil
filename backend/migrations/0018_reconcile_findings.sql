-- Mutabakat bulgulari (ADR-099): yonetilen kullanici OU'larindaki hesaplarin
-- OpenSicil'deki karsiligiyla karsilastirilmis hali. ANLIK GORUNTUDUR: her
-- tarama hedef bazinda oncekini siler, tarih serisi tutulmaz (ADR-099 madde 3).
-- Degisimin tarihcesi denetim kaydindadir.
--
-- Yalnizca worker yazar (ADR-015); backend okur ve ekrana basar.
CREATE TABLE reconcile_findings (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    -- Bulgunun hangi taramadan geldigi: ekran "en son ne zaman tarandi" der.
    read_job_id BIGINT NOT NULL REFERENCES read_jobs (id),
    kind TEXT NOT NULL CHECK (kind IN ('managed', 'observed', 'unmanaged', 'missing')),
    -- objectGUID. 'missing' bulgusunda hesap AD'de yok, deger account_links'ten gelir.
    external_id TEXT NOT NULL,
    account_name TEXT NOT NULL,
    display_name TEXT,
    -- Hesabin bulundugu konteyner (DN'in CN= kismi atilmis hali).
    container TEXT,
    -- AD'de etkin mi (userAccountControl ACCOUNTDISABLE biti). 'missing'te bilinmez.
    enabled BOOLEAN,
    -- Bagli hesapta kimlige baglanti; 'unmanaged'te NULL.
    identity_id BIGINT REFERENCES identities (id),
    found_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Ekran hedefe gore ve sinifa gore okur; tarama ise hedefin tamamini siler.
CREATE INDEX reconcile_findings_target_idx
    ON reconcile_findings (target_system_id, kind, account_name);

-- Ayni taramada ayni hesap iki kez bulunmaz.
CREATE UNIQUE INDEX reconcile_findings_account_idx
    ON reconcile_findings (target_system_id, external_id);
