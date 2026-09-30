-- Denetim kaydi (ADR-015, ADR-016): performed_by varsayilani current_user.
-- Servis rolleri INSERT'te yalnizca event_type ve detail verebilir (kolon bazli
-- GRANT), UPDATE/DELETE yapamaz; boylece backend worker adina satir uyduramaz.
-- GRANT'ler migrate.rs'te (SERVICE_GRANTS) verilir: rol adlari ortam
-- degiskeninden gelir, statik migration dosyasinda bilinmez.
-- id BIGSERIAL degil IDENTITY: identity kolonu sequence icin ayrica USAGE izni
-- istemez, GENERATED ALWAYS de servis rolunun id vermesini engeller.
CREATE TABLE audit_log (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    performed_by NAME NOT NULL DEFAULT current_user,
    event_type TEXT NOT NULL,
    detail JSONB NOT NULL DEFAULT '{}'::jsonb
);

-- Saatlik sayaclar "worker rolunun son bir saatte yazdigi satirlar"i sayar (ADR-016).
CREATE INDEX audit_log_performed_by_occurred_at_idx ON audit_log (performed_by, occurred_at);
