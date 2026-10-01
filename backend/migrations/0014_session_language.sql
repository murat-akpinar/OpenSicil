-- Operatorun arayuz dili tercihi oturum satirinda (ADR-067 madde 1, ADR-089).
-- Oturum yokken (giris, parola degistirme, Yapilandirma) dil Accept-Language'dan
-- gelir; orada saklanacak bir tercih yoktur.
ALTER TABLE operator_sessions
  ADD COLUMN lang TEXT NOT NULL DEFAULT 'tr' CHECK (lang IN ('tr', 'en'));
