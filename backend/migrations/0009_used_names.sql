-- Kullanilmis ad kaydi (ADR-011, ADR-035, ADR-042): kimlik silinince kullanici
-- adi ve e-posta duz metin buraya girer ve bir daha uretilmez; Sistem yoneticisi
-- gerekceyle serbest birakir (released_at), ad yeniden uretilebilir. Kayit
-- iptali ad yakmaz. Kisiye ait baska veri tutulmaz. Worker ekler (silmede),
-- backend serbest birakir (SERVICE_GRANTS).
CREATE TABLE used_names (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('username', 'email')),
    former_identity_id BIGINT REFERENCES identities (id),
    burned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at TIMESTAMPTZ,
    release_reason TEXT,
    CHECK ((released_at IS NULL) = (release_reason IS NULL))
);

-- Ayni ad ayni anda yalnizca bir kez yanik olabilir; cakisma kontrolu buradan
CREATE UNIQUE INDEX used_names_active_idx ON used_names (kind, lower(name)) WHERE released_at IS NULL;
