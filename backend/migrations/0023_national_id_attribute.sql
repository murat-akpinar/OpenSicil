-- ADR-106 madde 5: AD semasinda ulusal kimlik numarasi alani yoktur; kurum
-- koyduysa bir extensionAttribute'tadir ve adi kurulum ayaridir. Bos (varsayilan)
-- = tarama okumaz. Doluysa mutabakat taramasi o ozniteligi okur ve bulguya
-- **AEAD ile sifreli** yazar (ADR-010: kimlik no duz metin hicbir tabloya
-- girmez); toplu sahiplenme cozup `national_id::parse`'tan gecirir, gecen deger
-- kimlige sifreli + blind index'li yazilir, gecmeyen bos kalir.
ALTER TABLE app_settings ADD COLUMN ad_national_id_attribute TEXT NOT NULL DEFAULT '';

ALTER TABLE reconcile_findings ADD COLUMN national_id_enc BYTEA;
