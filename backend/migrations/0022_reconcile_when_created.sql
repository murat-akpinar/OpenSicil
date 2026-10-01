-- ADR-103 madde 6: baslangic tarihi uydurulmaz. AD'deki `whenCreated` hesabin
-- acilis gunudur ve ise giris tarihinin ulasilabilir en iyi yaklasimidir; tarama
-- bunu bulguya yazar, toplu sahiplenme `start_date` olarak kullanir, bossa
-- formdaki tarihe duser. Aksi halde "binlerce kisi bugun ise girdi" olur ve
-- panelin 30 gunluk sayaclari anlamini yitirir. Gun olarak tutulur (UTC gunu;
-- gece yarisina yakin acilan hesapta bir gun kayma kabul edilir).
ALTER TABLE reconcile_findings ADD COLUMN when_created DATE;
