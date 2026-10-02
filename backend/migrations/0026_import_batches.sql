-- Onay bekleyen CSV ice aktarma partisi (ADR-018: dosya tek degisiklik setidir,
-- esik ve onay kurallari aynen; ADR-055 madde 1: onay aninda yeniden onizleme).
-- Dosyanin kendisi saklanmaz: dogrulanmis satirlar JSON olarak AEAD ile sifreli
-- durur (kimlik no tasiyabilir, ADR-010). Uygulanan ya da reddedilen parti silinir.
CREATE TABLE import_batches (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    rows_enc BYTEA NOT NULL,
    row_count INTEGER NOT NULL,
    -- sahnelenirken etkilenen kimlik sayisi; onay aninda yeniden hesaplanip
    -- farkliysa soylenir (ADR-055 madde 1)
    affected INTEGER NOT NULL,
    -- ADR-042 madde 2: "olasi mukerrer kisi" uyarisi yukleyen tarafindan onaylandi
    duplicates_confirmed BOOLEAN NOT NULL DEFAULT FALSE,
    by_subject TEXT NOT NULL,
    by_username TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
