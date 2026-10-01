-- ADR-107: rol ve departman adresleri okunur ada doner. Slug'i backend uretir
-- (ADR-011 normallestirmesi, SQL'de ikinci kopya yok); bu yuzden kolon burada
-- bos acilir ve `migrate` alt komutu gecmis satirlari Rust'taki tek kuralla
-- doldurur. Tekillik indekste: ayni slug'a dusen ikinci ad sonek alir. NULL
-- serbest: yalnizca sembolden olusan adin slug'i yoktur, adres id ile calisir.
ALTER TABLE roles ADD COLUMN slug TEXT;
CREATE UNIQUE INDEX roles_slug_idx ON roles (slug);

ALTER TABLE departments ADD COLUMN slug TEXT;
CREATE UNIQUE INDEX departments_slug_idx ON departments (slug);
