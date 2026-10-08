-- Hogwarts test AD'si icin departman agaci + rol seti (test verisi, uretim degil).
--
-- Calistirmak icin:
--   docker compose exec -T db sh -lc 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB"' \
--     < scripts/hogwarts-seed-model.sql
--
-- Tekrar calistirilabilir. Gruplar ve OU'lar katalogdan adiyla/DN'iyle bulunur:
-- katalogda olmayan oge icin o satir yazilmaz, sessizce atlanir. Once
-- /targets -> "Kataloğu yenile" ile katalogun dolu oldugundan emin ol.
--
-- Okunur adresler (ADR-107): bu betik slug yazmaz (kural Rust'ta, SQL'de kopya
-- yok). Betikten sonra `docker compose run --rm --no-deps migrate` yeni satirlarin
-- slug'ini doldurur; o ana kadar adresler id ile calisir.
--
-- Yapi (gercek AD'deki OU ve grup duzenine birebir):
--   Hogwarts
--   ├── Teachers    OU=Teachers        GG-Teachers
--   │   └── brans adlari (Charms, Potions, Headmaster's Office …) — AD'deki
--   │       `department` degeri; OU ve grup TEACHERS'tan miras
--   ├── Staff       OU=Staff           GG-Staff
--   │   └── Facilities, Health Services, Library — ayni sekilde
--   └── Houses                                  GG-Students-All
--       ├── Gryffindor  OU=Gryffindor,OU=Houses   GG-House-Gryffindor
--       ├── Hufflepuff  …                                  GG-House-Hufflepuff
--       ├── Ravenclaw   …                                  GG-House-Ravenclaw
--       └── Slytherin   …                                  GG-House-Slytherin
-- Birincil roller unvan tasir (grup tasimaz; yapisal gruplar departmandan gelir),
-- ek roller ders ve Quidditch takimi gruplarini verir.

BEGIN;

-- --- Departman agaci ---
INSERT INTO departments (code, name, parent_id)
VALUES ('HOG', 'Hogwarts', NULL)
ON CONFLICT (code) DO NOTHING;

INSERT INTO departments (code, name, parent_id)
SELECT v.code, v.name, (SELECT id FROM departments WHERE code = 'HOG')
FROM (VALUES
  ('TEACHERS', 'Teachers'),
  ('STAFF',    'Staff'),
  ('HOUSES',   'Houses')
) AS v(code, name)
ON CONFLICT (code) DO NOTHING;

INSERT INTO departments (code, name, parent_id)
SELECT v.code, v.name, (SELECT id FROM departments WHERE code = 'HOUSES')
FROM (VALUES
  ('GRYFFINDOR', 'Gryffindor'),
  ('HUFFLEPUFF', 'Hufflepuff'),
  ('RAVENCLAW',  'Ravenclaw'),
  ('SLYTHERIN',  'Slytherin')
) AS v(code, name)
ON CONFLICT (code) DO NOTHING;

-- Ogretmen ve personel hesaplarinin AD'deki `department` degeri OU adi degil,
-- brans/birim adidir ("Charms", "Library"). Toplu sahiplenme departmani adiyla
-- esledigi icin (`bulk_adopt::candidates`) bu adlar agacta yoksa 13 hesap
-- "eslesmedi" rozeti aliyordu. Kendi gruplari ve OU'lari yok: zincir yukari
-- yurudugu icin TEACHERS/STAFF'in grubunu ve OU'sunu miras aliyorlar (ADR-017).
INSERT INTO departments (code, name, parent_id)
SELECT v.code, v.name, (SELECT id FROM departments WHERE code = 'TEACHERS')
FROM (VALUES
  ('CARE_MAGICAL', 'Care of Magical Creatures'),
  ('CHARMS',       'Charms'),
  ('DADA',         'Defence Against the Dark Arts'),
  ('DIVINATION',   'Divination'),
  ('FLYING',       'Flying'),
  ('HEADMASTER',   'Headmaster''s Office'),
  ('HERBOLOGY',    'Herbology'),
  ('POTIONS',      'Potions'),
  ('TRANSFIG',     'Transfiguration')
) AS v(code, name)
ON CONFLICT (code) DO NOTHING;

INSERT INTO departments (code, name, parent_id)
SELECT v.code, v.name, (SELECT id FROM departments WHERE code = 'STAFF')
FROM (VALUES
  ('FACILITIES', 'Facilities'),
  ('HEALTH',     'Health Services'),
  ('LIBRARY',    'Library')
) AS v(code, name)
ON CONFLICT (code) DO NOTHING;

-- --- Departman yetki ogeleri (cok degerli: birlesim alinir) ---
INSERT INTO department_entitlements (department_id, catalog_item_id)
SELECT d.id, c.id
FROM (VALUES
  ('TEACHERS',   'GG-Teachers'),
  ('STAFF',      'GG-Staff'),
  ('HOUSES',     'GG-Students-All'),
  ('GRYFFINDOR', 'GG-House-Gryffindor'),
  ('HUFFLEPUFF', 'GG-House-Hufflepuff'),
  ('RAVENCLAW',  'GG-House-Ravenclaw'),
  ('SLYTHERIN',  'GG-House-Slytherin')
) AS v(code, group_name)
JOIN departments d ON d.code = v.code
JOIN catalog_items c
  ON c.display_name = v.group_name AND c.kind = 'group' AND c.missing_since IS NULL
ON CONFLICT DO NOTHING;

-- --- Departman tek degerli ayarlari: OU, e-posta alan adi, UPN soneki ---
-- Hesaplar bugun hangi OU'daysa model de orayi soyluyor: sahiplenilen hesap
-- yerinden oynamaz. OU katalogda yoksa satir yazilmaz (kapsam AD_MANAGED_USER_OUS).
INSERT INTO department_target_settings (department_id, target_system_id, container_item_id)
SELECT d.id, c.target_system_id, c.id
FROM (VALUES
  ('TEACHERS',   'OU=Teachers,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('STAFF',      'OU=Staff,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('GRYFFINDOR', 'OU=Gryffindor,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('HUFFLEPUFF', 'OU=Hufflepuff,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('RAVENCLAW',  'OU=Ravenclaw,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('SLYTHERIN',  'OU=Slytherin,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local')
) AS v(code, ou_dn)
JOIN departments d ON d.code = v.code
JOIN catalog_items c
  ON lower(c.location) = lower(v.ou_dn) AND c.kind = 'ou' AND c.missing_since IS NULL
ON CONFLICT (department_id, target_system_id)
DO UPDATE SET container_item_id = EXCLUDED.container_item_id;

INSERT INTO department_target_settings (department_id, target_system_id, email_domain, upn_suffix)
SELECT d.id, t.id, 'hogwarts.local', 'hogwarts.local'
FROM departments d, target_systems t
WHERE d.code = 'HOG' AND t.kind = 'ad'
ON CONFLICT (department_id, target_system_id)
DO UPDATE SET email_domain = EXCLUDED.email_domain, upn_suffix = EXCLUDED.upn_suffix;

-- --- Birincil roller: unvan (AD'de `title`; 2026-10-08 olcumu: ogretmenler Teacher, hemsire Nurse) ---
INSERT INTO roles (kind, name, title)
VALUES
  ('primary', 'Headmaster',          'Headmaster'),
  ('primary', 'Deputy Headmistress', 'Deputy Headmistress'),
  ('primary', 'Professor',           'Teacher'),
  ('primary', 'Groundskeeper',       'Keeper of Keys and Grounds'),
  ('primary', 'Caretaker',           'Caretaker'),
  ('primary', 'Librarian',           'Librarian'),
  ('primary', 'Matron',              'Nurse'),
  ('primary', 'Student',             'Student')
ON CONFLICT (name) DO UPDATE SET title = EXCLUDED.title;

-- --- Ek roller: dersler (GG-Course-* basina bir rol) ---
INSERT INTO roles (kind, name)
SELECT 'additional', 'Course: ' || replace(replace(display_name, 'GG-Course-', ''), '-', ' ')
FROM catalog_items
WHERE kind = 'group' AND missing_since IS NULL AND display_name LIKE 'GG-Course-%'
ON CONFLICT (name) DO NOTHING;

INSERT INTO role_entitlements (role_id, catalog_item_id)
SELECT r.id, c.id
FROM catalog_items c
JOIN roles r
  ON r.name = 'Course: ' || replace(replace(c.display_name, 'GG-Course-', ''), '-', ' ')
WHERE c.kind = 'group' AND c.missing_since IS NULL AND c.display_name LIKE 'GG-Course-%'
ON CONFLICT DO NOTHING;

-- --- Ek roller: Quidditch takimlari (ev takimi + GG-Team-Quidditch-All) ---
INSERT INTO roles (kind, name)
SELECT 'additional', 'Quidditch: ' || replace(display_name, 'GG-Team-Quidditch-', '')
FROM catalog_items
WHERE kind = 'group' AND missing_since IS NULL
  AND display_name LIKE 'GG-Team-Quidditch-%'
  AND display_name <> 'GG-Team-Quidditch-All'
ON CONFLICT (name) DO NOTHING;

INSERT INTO role_entitlements (role_id, catalog_item_id)
SELECT r.id, c.id
FROM catalog_items c
JOIN roles r ON r.name = 'Quidditch: ' || replace(c.display_name, 'GG-Team-Quidditch-', '')
WHERE c.kind = 'group' AND c.missing_since IS NULL
  AND c.display_name LIKE 'GG-Team-Quidditch-%'
  AND c.display_name <> 'GG-Team-Quidditch-All'
ON CONFLICT DO NOTHING;

INSERT INTO role_entitlements (role_id, catalog_item_id)
SELECT r.id, c.id
FROM roles r, catalog_items c
WHERE r.kind = 'additional' AND r.name LIKE 'Quidditch: %'
  AND c.kind = 'group' AND c.missing_since IS NULL
  AND c.display_name = 'GG-Team-Quidditch-All'
ON CONFLICT DO NOTHING;

COMMIT;

-- --- Sonuc ---
SELECT d.code, d.name, p.code AS parent,
       (SELECT count(*) FROM department_entitlements e WHERE e.department_id = d.id) AS gruplar,
       (SELECT c.display_name FROM department_target_settings s
          JOIN catalog_items c ON c.id = s.container_item_id
         WHERE s.department_id = d.id) AS ou
FROM departments d LEFT JOIN departments p ON p.id = d.parent_id
ORDER BY p.code NULLS FIRST, d.code;

SELECT r.kind, r.name, r.title,
       (SELECT count(*) FROM role_entitlements e WHERE e.role_id = r.id) AS gruplar
FROM roles r ORDER BY r.kind, r.name;

-- Toplu sahiplenme ekraninin "eslesmedi" rozetiyle ayni kosul
-- (`bulk_adopt::candidates`): bos cikmasi gerekir.
SELECT f.account_name, f.department_name AS eslesmeyen_departman
FROM reconcile_findings f
LEFT JOIN departments d ON lower(d.name) = lower(f.department_name)
WHERE f.kind = 'unmanaged' AND d.id IS NULL
ORDER BY f.department_name, f.account_name;
