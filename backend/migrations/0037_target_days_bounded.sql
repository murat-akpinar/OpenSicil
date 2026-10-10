-- Hedefin gun ayarlari 100 yille sinirli (org::MAX_TARGET_DAYS). Ust sinir yokken
-- worker'da `end_at + make_interval(days => ...)` tasiyor, zamanlayici tiki geri
-- aliniyor ve ayrilanlarin hesaplari kapanmiyordu (guvenlik denetimi OS-06).
UPDATE target_systems SET retention_days = LEAST(retention_days, 36500),
    password_reset_delay_days = LEAST(password_reset_delay_days, 36500);
ALTER TABLE target_systems
    ADD CONSTRAINT target_systems_days_bounded
    CHECK (retention_days <= 36500 AND password_reset_delay_days <= 36500);
