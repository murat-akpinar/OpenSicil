-- Ulusal kimlik numarasi (ADR-010): uygulama katmaninda AEAD ile sifreli deger
-- (basinda anahtar surumu), tekillik ve arama icin ayri anahtarla HMAC-SHA256
-- blind index, ulke kodu (ISO 3166-1 alpha-2). Duz metin hicbir kolonda yok.
-- Uc kolon birlikte dolu ya da birlikte bos; silindi temizligi ucunu birden
-- bosaltir (ADR-038). Backend yazar, worker yalnizca temizler (SERVICE_GRANTS).
ALTER TABLE identities
    ADD COLUMN national_id_enc BYTEA,
    ADD COLUMN national_id_bidx BYTEA UNIQUE,
    ADD COLUMN national_id_country TEXT CHECK (national_id_country ~ '^[A-Z]{2}$'),
    ADD CHECK (
        (national_id_enc IS NULL) = (national_id_bidx IS NULL)
        AND (national_id_enc IS NULL) = (national_id_country IS NULL)
    );
