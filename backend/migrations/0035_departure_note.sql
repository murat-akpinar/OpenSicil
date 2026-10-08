-- Ayrilis nedeni serbest metin (ADR-111 madde 3): `{departure_note}` token'i, AD
-- `description`ina eslenebilir. Kisisel veri: `silindi` temizliginde bosalir.
ALTER TABLE identities ADD COLUMN departure_note TEXT
    CHECK (char_length(departure_note) <= 200);

-- ADR-111 madde 1: AD'de saklama sonu silme degil, onay bekler. Mevcut kurulumda
-- da cevrilir: yon guvenli (hesap silinmez, "Silinmeyi bekleyenler"e duser).
UPDATE target_systems SET delete_requires_approval = TRUE WHERE kind = 'ad';
