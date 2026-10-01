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
-- Yapi (gercek AD'deki OU ve grup duzenine birebir):
--   Hogwarts
--   ├── Teachers    OU=Users,OU=Teachers        GG-Teachers
--   ├── Staff       OU=Users,OU=Staff           GG-Staff
--   └── Houses                                  GG-Students-All
--       ├── Gryffindor  OU=Users,OU=Gryffindor,OU=Houses   GG-House-Gryffindor
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
  ('TEACHERS',   'OU=Users,OU=Teachers,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('STAFF',      'OU=Users,OU=Staff,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('GRYFFINDOR', 'OU=Users,OU=Gryffindor,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('HUFFLEPUFF', 'OU=Users,OU=Hufflepuff,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('RAVENCLAW',  'OU=Users,OU=Ravenclaw,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local'),
  ('SLYTHERIN',  'OU=Users,OU=Slytherin,OU=Houses,OU=Hogwarts,DC=hogwarts,DC=local')
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

-- --- Birincil roller: unvan (AD'de `title`) ---
INSERT INTO roles (kind, name, title)
VALUES
  ('primary', 'Headmaster',          'Headmaster'),
  ('primary', 'Deputy Headmistress', 'Deputy Headmistress'),
  ('primary', 'Professor',           'Professor'),
  ('primary', 'Groundskeeper',       'Keeper of Keys and Grounds'),
  ('primary', 'Caretaker',           'Caretaker'),
  ('primary', 'Librarian',           'Librarian'),
  ('primary', 'Matron',              'Matron'),
  ('primary', 'Student',             'Student')
ON CONFLICT (name) DO NOTHING;

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
