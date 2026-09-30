-- Katalog ve yetki ogeleri (docs/03 Katalog; ADR-007, 014, 017, 024, 040).
-- Hedef sistem tur degil kayittir (docs/03): v1'de bir AD ve bir Zimbra satiri
-- seed edilir, baglanti bilgisi app_settings'te (ADR-068). Saklama suresi ve
-- silme onayi hedef sistem basina (ADR-024). Katalogu yalnizca worker yazar,
-- ogeyi silmez, "kayip" isaretler (ADR-015, docs/03). Izinler SERVICE_GRANTS'ta.
CREATE TABLE target_systems (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('ad', 'zimbra')),
    name TEXT NOT NULL UNIQUE,
    -- "hesap acilsin mi" varsayilani; yalnizca hesap yokken okunur (ADR-040)
    provision_account_default BOOLEAN NOT NULL DEFAULT TRUE,
    retention_days INTEGER NOT NULL DEFAULT 90 CHECK (retention_days >= 0),
    delete_requires_approval BOOLEAN NOT NULL,
    -- Ayrilista parola G gun sonra rastgelelestirilir; 0 = hemen (ADR-033)
    password_reset_delay_days INTEGER NOT NULL DEFAULT 7 CHECK (password_reset_delay_days >= 0)
);

INSERT INTO target_systems (kind, name, delete_requires_approval)
VALUES ('ad', 'Active Directory', FALSE), ('zimbra', 'Zimbra', TRUE);

-- external_id: AD objectGUID (tireli RFC 4122 yazimi, docs/05) ya da zimbraId.
-- display_name ve location yalnizca gosterim, katalog yenilendikce guncellenir.
-- is_membership / is_container uretilmis kolonlari, yetki ogesi ve konteyner
-- referanslarinin bilesik yabanci anahtarla dogru ture baglanmasi icin
-- (ADR-077 madde 4 ile ayni teknik).
CREATE TABLE catalog_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    kind TEXT NOT NULL CHECK (kind IN ('ou', 'group', 'distribution_list', 'cos')),
    external_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    location TEXT,
    sid TEXT,
    missing_since TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_membership BOOLEAN NOT NULL
        GENERATED ALWAYS AS (kind IN ('group', 'distribution_list')) STORED,
    is_container BOOLEAN NOT NULL GENERATED ALWAYS AS (kind IN ('ou', 'cos')) STORED,
    UNIQUE (target_system_id, external_id),
    UNIQUE (id, target_system_id),
    UNIQUE (id, is_membership),
    UNIQUE (id, is_container),
    CHECK (sid IS NULL OR kind = 'group')
);

-- Hedef sistem varsayilan konteyneri (AD: OU, Zimbra: COS); ayni hedefin
-- konteyner turunde bir katalog ogesi olmali.
ALTER TABLE target_systems
    ADD COLUMN default_container_item_id BIGINT,
    ADD COLUMN default_container_is_container BOOLEAN NOT NULL GENERATED ALWAYS AS (TRUE) STORED,
    ADD FOREIGN KEY (default_container_item_id, id) REFERENCES catalog_items (id, target_system_id),
    ADD FOREIGN KEY (default_container_item_id, default_container_is_container)
        REFERENCES catalog_items (id, is_container);

-- Cok degerli ayar: grup ve dagitim listesi; her rol turu ve departman tasir,
-- birlesim alinir (ADR-007).
CREATE TABLE role_entitlements (
    role_id BIGINT NOT NULL REFERENCES roles (id),
    catalog_item_id BIGINT NOT NULL,
    is_membership BOOLEAN NOT NULL GENERATED ALWAYS AS (TRUE) STORED,
    PRIMARY KEY (role_id, catalog_item_id),
    FOREIGN KEY (catalog_item_id, is_membership) REFERENCES catalog_items (id, is_membership)
);

CREATE TABLE department_entitlements (
    department_id BIGINT NOT NULL REFERENCES departments (id),
    catalog_item_id BIGINT NOT NULL,
    is_membership BOOLEAN NOT NULL GENERATED ALWAYS AS (TRUE) STORED,
    PRIMARY KEY (department_id, catalog_item_id),
    FOREIGN KEY (catalog_item_id, is_membership) REFERENCES catalog_items (id, is_membership)
);

-- Tek degerli ayarlar: yalnizca birincil rol ve departman tasir; NULL = "bu
-- kaynak soylemiyor", oncelik sirasinda siradakine bakilir (ADR-007, ADR-017).
-- email_domain ve upn_suffix serbest metin; worker yonetilen alan adlarina
-- karsi dogrular (ADR-017, ADR-077). Konteyner ayni hedefin OU/COS ogesi olmali.
CREATE TABLE role_target_settings (
    role_id BIGINT NOT NULL,
    role_kind TEXT NOT NULL GENERATED ALWAYS AS ('primary') STORED,
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    provision_account BOOLEAN,
    container_item_id BIGINT,
    container_is_container BOOLEAN NOT NULL GENERATED ALWAYS AS (TRUE) STORED,
    email_domain TEXT,
    upn_suffix TEXT,
    PRIMARY KEY (role_id, target_system_id),
    FOREIGN KEY (role_id, role_kind) REFERENCES roles (id, kind),
    FOREIGN KEY (container_item_id, target_system_id) REFERENCES catalog_items (id, target_system_id),
    FOREIGN KEY (container_item_id, container_is_container) REFERENCES catalog_items (id, is_container)
);

CREATE TABLE department_target_settings (
    department_id BIGINT NOT NULL REFERENCES departments (id),
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    provision_account BOOLEAN,
    container_item_id BIGINT,
    container_is_container BOOLEAN NOT NULL GENERATED ALWAYS AS (TRUE) STORED,
    email_domain TEXT,
    upn_suffix TEXT,
    PRIMARY KEY (department_id, target_system_id),
    FOREIGN KEY (container_item_id, target_system_id) REFERENCES catalog_items (id, target_system_id),
    FOREIGN KEY (container_item_id, container_is_container) REFERENCES catalog_items (id, is_container)
);
