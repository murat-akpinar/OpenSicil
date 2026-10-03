-- ADR-120 madde 5: fark listesine rol girer. AD'nin `title` ozniteligi bugune
-- kadar hic okunmuyordu (`worker/src/ad.rs` ACCOUNT_ATTRS) ve bulguda kolonu
-- yoktu; rolun AD karsiligi `roles.title` kolonunda duruyor (0004), eslestirme
-- rol adi uzerinden degil o kolon uzerinden yapilir.
--
-- NULL olabilir: gercek AD'de `title` siklikla bos.
ALTER TABLE reconcile_findings ADD COLUMN title TEXT;
