-- Silme onayi onaylanan ayrilisa aittir (ADR-024). Ayrilis geri alinir, tarihi
-- degisir ya da kayit iptale doner ve onaylanan silme henuz yapilmamissa onay
-- kalkar; yoksa sonraki ayrilista hesap yeni onay olmadan siliniyordu (guvenlik
-- denetimi OS-10). end_at'i yazan her yol (formlar, CSV) buradan gecer.
CREATE FUNCTION reset_deletion_approval() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE account_links SET deletion_approved = FALSE
     WHERE identity_id = NEW.id AND deletion_approved;
    RETURN NULL;
END $$;
CREATE TRIGGER identities_departure_changed
    AFTER UPDATE OF end_at, cancelled ON identities FOR EACH ROW
    WHEN (OLD.end_at IS DISTINCT FROM NEW.end_at OR OLD.cancelled IS DISTINCT FROM NEW.cancelled)
    EXECUTE FUNCTION reset_deletion_approval();
