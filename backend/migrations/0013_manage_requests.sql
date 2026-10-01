-- Gozlem modundaki baglantinin yonetime alinmasi (ADR-018/087). Istek backend'in
-- yazabildigi tek kolondur; modu gozlemden yonetilene yine worker cevirir
-- (ADR-015, docs/03 "mod bayragini worker cevirir").
ALTER TABLE account_links ADD COLUMN manage_requested_at TIMESTAMPTZ;
