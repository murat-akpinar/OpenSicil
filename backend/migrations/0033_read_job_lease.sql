-- Okuma seridinde is kirasi (ADR-062'nin okuma seridi karsiligi): `claim` satira
-- kira yazar, kirasini uzatmayan is (worker is ortasinda olduyse) geri alinir.
-- Kira olmadan satir sonsuza dek `running` kalir ve read_jobs_open_idx o tur +
-- hedef icin yeni is actirmaz: gece mutabakati her gece sessizce duser.
ALTER TABLE read_jobs ADD COLUMN locked_until TIMESTAMPTZ;
