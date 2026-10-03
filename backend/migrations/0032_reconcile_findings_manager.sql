-- ADR-129: tarama hesabin `manager` DN'ini okur ve ayni taramanin hesap
-- listesinden yoneticinin objectGUID'ine cevirir; geri dolum bu GUID'den
-- `account_links` uzerinden kimlige ulasir. Yonetici kapsam disindaysa bos.
ALTER TABLE reconcile_findings ADD COLUMN manager_external_id text;
