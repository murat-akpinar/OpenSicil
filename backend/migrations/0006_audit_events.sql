-- Denetim kaydi alanlari ve worker islem turleri sabitlenir
-- (docs/07 "Denetim kaydi"; ADR-014, 016, 050, 062).
-- Operator satiri: actor_subject (OIDC sub) + actor_username; bootstrap
-- hesabinda subject yok. Worker satiri: actor yok; hedefe her yazmadan once
-- NIYET satiri (operation_class dolu), sonra ona baglanan SONUC satiri
-- (intent_id + outcome). Sonucu olmayan niyet "sonucu bilinmiyor"dur.
-- operation_class sayac sinifidir (ADR-050): yikici, verme, ilk parola,
-- yalnizca oznitelik (sayilmaz). Acil ayrilis yikici + emergency (ADR-016 kota).
-- Yalnizca worker operation_class yazabilir (kolon bazli INSERT, migrate.rs),
-- backend sahte niyetle sayaci dolduramaz.
ALTER TABLE audit_log
    ADD COLUMN actor_subject TEXT,
    ADD COLUMN actor_username TEXT,
    ADD COLUMN identity_id BIGINT REFERENCES identities (id),
    ADD COLUMN target_system_id BIGINT REFERENCES target_systems (id),
    ADD COLUMN operation_class TEXT
        CHECK (operation_class IN ('destructive', 'grant', 'first_password', 'attribute')),
    ADD COLUMN emergency BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN intent_id BIGINT REFERENCES audit_log (id),
    ADD COLUMN outcome TEXT CHECK (outcome IN ('succeeded', 'failed')),
    ADD CHECK (operation_class IS NOT NULL OR NOT emergency),
    ADD CHECK ((intent_id IS NULL) = (outcome IS NULL));

-- Sayac artik role degil niyet sinifina bakar; eski indeks gereksiz.
DROP INDEX audit_log_performed_by_occurred_at_idx;
CREATE INDEX audit_log_intent_window_idx ON audit_log (operation_class, occurred_at)
    WHERE intent_id IS NULL AND operation_class IS NOT NULL;
CREATE INDEX audit_log_identity_id_idx ON audit_log (identity_id);

-- Saatlik sayac dolulugu: son bir saatteki niyet satirlari, birim kimlik
-- (ayni kimligin yeniden denemesi sayiyi buyutmez). Worker freni buradan
-- uygular (3f), backend "neden bekliyor"u buradan gosterir (ADR-039).
CREATE VIEW hourly_counter_usage AS
SELECT operation_class,
       COUNT(DISTINCT identity_id) AS identities,
       COUNT(DISTINCT identity_id) FILTER (WHERE emergency) AS emergency_identities
FROM audit_log
WHERE intent_id IS NULL
  AND operation_class IS NOT NULL
  AND occurred_at >= now() - interval '1 hour'
GROUP BY operation_class;
