-- F-19 / ADR-054: worker'in modu (kuru calistirma) ve son gorulme ani backend'in
-- metrik ucuna ve panele buradan gelir — backend worker'in ortamini goremez,
-- `worker-health` dosya tabanlidir ve container'in icinde kalir. Tek satir;
-- worker zamanlayici tikinde (dakikada bir) yazar, backend yalnizca okur.
CREATE TABLE worker_status (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    worker_id TEXT NOT NULL,
    dry_run BOOLEAN NOT NULL,
    seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
