-- ADR-055 madde 3: sahnelenen partide "sicil no degisimi" onerisinin yukleyen
-- tarafindan onaylandigi da saklanir (mukerrer onayi gibi).
ALTER TABLE import_batches ADD COLUMN renumber_confirmed BOOLEAN NOT NULL DEFAULT FALSE;
