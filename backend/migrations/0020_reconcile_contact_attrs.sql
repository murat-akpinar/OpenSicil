-- Sahiplenmede AD'den gelen iletisim alanlari (ADR-106). 0019 kisi
-- ozniteliklerini (ad, soyad, sicil, departman) getirmisti; kullanici
-- "telefon e-posta TC gelmemis gibi" dedi ve olcum uc ayri sebep buldu:
-- (1) sahiplenme isi kuru calistirma yuzunden uygulanmamisti, (2) tarama
-- `mail`/`mobile`/`telephoneNumber`'i hic okumuyordu, (3) TC AD semasinda yok.
-- Bu migration ikinciyi kapatiyor.
--
-- Ucu de NULL olabilir. Cep ve sabit hat ayri kolonlardir: kimlikteki alan
-- **cep**tir, sabit hat yalnizca cep bosken yedege gecer ve E.164 kontrolunden
-- gecmek zorundadir (uydurma numara yazilmaz, `identity::validate` gevsemez).
ALTER TABLE reconcile_findings
    ADD COLUMN mail TEXT,
    ADD COLUMN mobile TEXT,
    ADD COLUMN telephone_number TEXT;
