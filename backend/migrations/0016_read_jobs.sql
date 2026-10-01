-- Okuma seridi (ADR-051): mutabakat, katalog yenileme ve toplu yonetime almanin
-- fark hesabi yazma seridinden AYRI bir gorevde calisir. Bu isler hedefe hicbir
-- sey yazmaz ve fren sayaclarina dokunmaz (ADR-050); veritabanina yalnizca
-- katalog, rapor ve iş kaydi yazarlar. Ayni anda en fazla bir okuma isi calisir,
-- boylece acil ayrilis (N-02) uzun bir mutabakatin arkasinda beklemez.
CREATE TABLE read_jobs (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('catalog_refresh', 'reconcile')),
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    status TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'running', 'succeeded', 'failed')),
    -- Operator istegiyse kullanici adi, zamanlayici/acilis istegiyse NULL.
    requested_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    result TEXT,
    CHECK ((started_at IS NULL) OR status <> 'queued')
);

-- Tekillestirme (ADR-016 kuyruk kurali): ayni tur ve hedef icin acik is varken
-- yenisi acilmaz; "yenile" dugmesine iki kez basmak iki tarama yapmaz.
CREATE UNIQUE INDEX read_jobs_open_idx ON read_jobs (kind, target_system_id)
    WHERE status IN ('queued', 'running');

CREATE INDEX read_jobs_recent_idx ON read_jobs (target_system_id, created_at DESC);
