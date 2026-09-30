-- Kimlik modeli: departman agaci, roller, kimlik, ek rol atamasi
-- (docs/03; ADR-007, 017, 020, 038, 041, 053, 077).
-- Yonetilen kapsam ve yasakli grup listesi tablo degil (ADR-077).
-- Durum kolonu yok: tarihlerden turetilir (ADR-038). Rol/departman yetki ogeleri
-- ve tek degerli ayarlar katalog tablolariyla gelir (GUID'e referans verirler).
-- Servis rollerinin izinleri migrate.rs SERVICE_GRANTS'ta.

-- Derinlik <= 8 ve dongu kontrolu uygulama katmaninda (ADR-017; departman ekrani 3b).
CREATE TABLE departments (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    parent_id BIGINT REFERENCES departments (id),
    code TEXT UNIQUE,
    name TEXT NOT NULL,
    CHECK (parent_id IS DISTINCT FROM id)
);

-- (id, kind) tekilligi, kimlik tablosunun rol turunu yabanci anahtarla
-- kisitlamasi icin (ADR-077 madde 4). title (unvan) tek degerli ayardir,
-- yalnizca birincil rol tasir (ADR-007).
CREATE TABLE roles (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('base', 'primary', 'additional')),
    name TEXT NOT NULL UNIQUE,
    title TEXT CHECK (kind = 'primary' OR title IS NULL),
    UNIQUE (id, kind)
);

-- Temel rol kurulumda tektir (ADR-007).
CREATE UNIQUE INDEX roles_single_base_idx ON roles ((TRUE)) WHERE kind = 'base';

-- end_at bitis ANIdir (planlida ertesi gun 00:00, acilde simdi); start_date ve
-- aski tarihleri gun, kurulum saat diliminde yorumlanir (ADR-038, ADR-053).
-- suspension_end iznin son gunudur (ADR-059). username/email/upn worker yazar,
-- olustuktan sonra degismez (ADR-011, ADR-015). Yonetici alani ayrilista
-- degismez, devir yoneticisi ayrilanin kaydinda durur (ADR-041).
-- Kimlik no kolonlari ADR-010 kutucugunda ALTER ile gelir.
CREATE TABLE identities (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    given_name TEXT NOT NULL,
    surname TEXT NOT NULL,
    employee_number TEXT UNIQUE,
    mobile_phone TEXT,
    existing_ad_account_hint TEXT,
    existing_zimbra_account_hint TEXT,
    department_id BIGINT NOT NULL REFERENCES departments (id),
    primary_role_id BIGINT NOT NULL,
    primary_role_kind TEXT NOT NULL GENERATED ALWAYS AS ('primary') STORED,
    manager_id BIGINT REFERENCES identities (id),
    handover_manager_id BIGINT REFERENCES identities (id),
    employment_type TEXT NOT NULL
        CHECK (employment_type IN ('permanent', 'contract', 'intern', 'outsourced')),
    start_date DATE NOT NULL,
    end_at TIMESTAMPTZ,
    suspension_start DATE,
    suspension_end DATE,
    cancelled BOOLEAN NOT NULL DEFAULT FALSE,
    -- Acil ayrilis: parola hemen sifirlanir, is oncelikli ve kotali (ADR-016, 033)
    emergency_departure BOOLEAN NOT NULL DEFAULT FALSE,
    deleted_at TIMESTAMPTZ,
    username TEXT UNIQUE,
    email TEXT UNIQUE,
    upn TEXT UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (primary_role_id, primary_role_kind) REFERENCES roles (id, kind),
    CHECK (manager_id IS DISTINCT FROM id),
    -- Kadrolu disinda bitis tarihi zorunlu (docs/03 kimlik semasi)
    CHECK (employment_type = 'permanent' OR end_at IS NOT NULL),
    CHECK (suspension_end IS NULL OR suspension_start IS NOT NULL),
    CHECK (suspension_end IS NULL OR suspension_end >= suspension_start),
    -- Kayit iptali = iptal isareti + bitis ani simdi (ADR-038); acil ayrilis da bitis ister
    CHECK (NOT cancelled OR end_at IS NOT NULL),
    CHECK (NOT emergency_departure OR end_at IS NOT NULL)
);

CREATE INDEX identities_department_id_idx ON identities (department_id);
CREATE INDEX identities_manager_id_idx ON identities (manager_id);

-- ends_on gunun sonu olarak yorumlanir; gecmis tarihli atama kaydedilemez
-- kurali uygulama katmaninda (now() icerir, ADR-020). Suresi dolan atamayi
-- zamanlayici (worker) siler (ADR-038).
CREATE TABLE identity_additional_roles (
    identity_id BIGINT NOT NULL REFERENCES identities (id),
    role_id BIGINT NOT NULL,
    role_kind TEXT NOT NULL GENERATED ALWAYS AS ('additional') STORED,
    ends_on DATE,
    PRIMARY KEY (identity_id, role_id),
    FOREIGN KEY (role_id, role_kind) REFERENCES roles (id, kind)
);
