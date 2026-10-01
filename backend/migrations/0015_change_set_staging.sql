-- Degisiklik seti sahneleme (ADR-031): esigi asan rol ya da departman
-- duzenlemesi modele yazilmaz, taslak olarak bekler. Motor yalnizca YAYIMLANMIS
-- tanimi okur (worker'in model yukleyicisi bu kolonlara bakmaz), bu yuzden
-- onaylanmamis bir duzenleme hedefe sizamaz. Bir tanimin en fazla bir taslagi
-- olur: tek kolon, yeni duzenleme ustune yazar.
--
-- pending_by OIDC `sub`; onayi BASKA bir Sistem yoneticisi verir ([ADR-026](...)),
-- ya da onay zaman kilidi aciksa pending_at + N saat gecince baslatan da verebilir.
ALTER TABLE roles
    ADD COLUMN pending_definition JSONB,
    ADD COLUMN pending_by TEXT,
    ADD COLUMN pending_by_username TEXT,
    ADD COLUMN pending_at TIMESTAMPTZ,
    ADD CONSTRAINT roles_pending_complete
        CHECK ((pending_definition IS NULL) = (pending_at IS NULL));

ALTER TABLE departments
    ADD COLUMN pending_definition JSONB,
    ADD COLUMN pending_by TEXT,
    ADD COLUMN pending_by_username TEXT,
    ADD COLUMN pending_at TIMESTAMPTZ,
    ADD CONSTRAINT departments_pending_complete
        CHECK ((pending_definition IS NULL) = (pending_at IS NULL));
