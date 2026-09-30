-- Hesap baglantisi ve is kuyrugu (docs/02 kuyruk, docs/03 hesap baglantisi;
-- ADR-015, 016, 018, 028, 032, 033, 040, 046, 047, 048, 052, 062).

-- Kimligin bir hedef sistemdeki hesabi; yalnizca worker yazar (ADR-015):
-- motor sadece bagli ve yonetilen hesaba dokunur. applied_state = o hedefe en
-- son uygulanan kimlik durumu (ADR-032; zamanlayicinin karsilastirma tabani).
-- Zimbra'ya ozgu saklanan posta ayarlari Zimbra bolumunde eklenir.
CREATE TABLE account_links (
    identity_id BIGINT NOT NULL REFERENCES identities (id),
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    external_id TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('provisioned', 'adopted')),
    mode TEXT NOT NULL CHECK (mode IN ('managed', 'observed')),
    applied_state TEXT
        CHECK (applied_state IN ('pending', 'active', 'suspended', 'departed', 'deleted')),
    first_password_issued BOOLEAN NOT NULL DEFAULT FALSE,
    -- ADR-033/046: ayrilista parola OpenSicil tarafindan sifirlandi
    password_reset_at_departure BOOLEAN NOT NULL DEFAULT FALSE,
    -- ADR-048: iptal icin hedefte "hic kullanilmamis" dogrulamasi; NULL = bakilmadi
    verified_unused BOOLEAN,
    -- ADR-024: onay gerektiren hedefte silme onaylandi
    deletion_approved BOOLEAN NOT NULL DEFAULT FALSE,
    -- ADR-042: sahiplenmede hedefteki ad-soyad kimlikle uyusmadi (uyari)
    name_mismatch BOOLEAN NOT NULL DEFAULT FALSE,
    -- ADR-040: hesabi OpenSicil sildi (kayip hesaptan ayirt edilir)
    deleted_by_us_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (identity_id, target_system_id),
    UNIQUE (target_system_id, external_id)
);

-- Is: "bu kimligi su hedefte olmasi gereken duruma getir" (kimlik bazli, idempotent).
-- priority: 0 acil ayrilis, 1 tek kimlik islemi ve tarihli gecis, 2 toplu
-- degisiklik seti, 3 mutabakat (ADR-016). Kira 5 dk (ADR-062); kirasi dolan is
-- deneme tuketmeden yeniden alinir. Deneme hakki biten is 'needs_intervention'
-- olur ve ACIK sayilir (ADR-052); operator retry_requested ile kuyruga dondurur
-- (backend status yazamaz: ele gecirilmis backend ayrilisi "basarili" isaretleyemez).
CREATE TABLE jobs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    identity_id BIGINT NOT NULL REFERENCES identities (id),
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    priority SMALLINT NOT NULL CHECK (priority BETWEEN 0 AND 3),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'succeeded', 'needs_intervention')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_by TEXT,
    locked_until TIMESTAMPTZ,
    retry_requested BOOLEAN NOT NULL DEFAULT FALSE,
    last_error TEXT,
    result TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    CHECK ((status = 'running') = (locked_by IS NOT NULL))
);

-- Tekillestirme (ADR-016): ayni kimlik ve hedef icin en fazla bir ACIK is.
CREATE UNIQUE INDEX jobs_one_open_per_identity_target_idx
    ON jobs (identity_id, target_system_id) WHERE status <> 'succeeded';
CREATE INDEX jobs_claim_idx ON jobs (priority, created_at) WHERE status <> 'succeeded';
