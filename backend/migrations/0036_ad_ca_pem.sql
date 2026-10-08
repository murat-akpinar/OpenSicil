-- AD kok CA'si Yapilandirma ekranindan (ADR-136); sir degil, sifrelenmez.
-- Bos = henuz girilmedi; AD adresi doluyken baglanti "CA girilmemis" der.
ALTER TABLE app_settings ADD COLUMN ad_ca_pem TEXT NOT NULL DEFAULT '';
