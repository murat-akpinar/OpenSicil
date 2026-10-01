-- Toplu sahiplenme (ADR-102): mutabakat bulgusu artik hesabin kisi
-- ozniteliklerini de tasir. Operator "yonetilmeyen" hesaplari secip toplu
-- sahiplendiginde backend kimlik satirini bu degerlerden kurar; AD'ye ikinci
-- bir okuma gitmez (ADR-004: backend AD'ye baglanmaz).
--
-- Hepsi NULL olabilir: gercek AD'de givenName/sn/employeeID/department bos
-- olabiliyor (W8). Bos olani operator toplu formda kendisi veriyor.
ALTER TABLE reconcile_findings
    ADD COLUMN given_name TEXT,
    ADD COLUMN surname TEXT,
    ADD COLUMN employee_number TEXT,
    -- AD'nin serbest metin `department` degeri; departman agaciyla ad
    -- eslesmesi backend'de yapilir, yabanci anahtar degildir.
    ADD COLUMN department_name TEXT;
