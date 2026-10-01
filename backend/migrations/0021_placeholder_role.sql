-- ADR-103 madde 4/5: AD'den sahiplenilen kisinin rolu yoktur. Zorunlu kolon
-- gevsetilmez; yetki ogesi olmayan bir "Tanimsiz" birincil rol seed'lenir ve
-- toplu sahiplenmenin varsayilani olur. Bayrak ad yerine kolondadir: operator
-- rolu yeniden adlandirsa da sayac ("rolu atanmamis N kisi") ve yonetime alma
-- kapisi calismaya devam eder. Tek yer tutucu olur (kismi tekil indeks).
ALTER TABLE roles ADD COLUMN placeholder BOOLEAN NOT NULL DEFAULT false;

CREATE UNIQUE INDEX roles_single_placeholder_idx ON roles ((TRUE)) WHERE placeholder;

INSERT INTO roles (kind, name, placeholder)
VALUES ('primary', 'Tanımsız', true)
ON CONFLICT (name) DO UPDATE SET placeholder = true;
