-- Oznitelik eslemesi (ADR-012, 029, 034, 082): hedef sistem basina satir;
-- hedef oznitelik kodda sabit izinli listeden (mapping_rules.rs), kaynak +
-- donusum, "sadece bossa yaz" (ADR-034). Backend yazar; worker okur ve her iste
-- dogrular: listede olmayan hedef ya da ayar kapaliyken hassas kaynak mudahaledir.
CREATE TABLE attribute_mappings (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    target_system_id BIGINT NOT NULL REFERENCES target_systems (id),
    target_attribute TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    -- sabit metin ya da sablon; diger kaynaklarda NULL
    source_text TEXT,
    transform TEXT NOT NULL DEFAULT 'none',
    write_if_empty BOOLEAN NOT NULL DEFAULT FALSE,
    -- ADR-012: hassas kaynak icin Sistem yoneticisinin acik onayi (denetimde)
    sensitive_acknowledged BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (target_system_id, target_attribute)
);

-- ADR-012 varsayilan AD eslemeleri; sAMAccountName ve UPN eslenemez (ADR-034).
INSERT INTO attribute_mappings (target_system_id, target_attribute, source_kind, source_text)
SELECT t.id, m.attr, m.src, m.txt
FROM target_systems t,
     (VALUES ('givenName', 'given_name', NULL),
             ('sn', 'surname', NULL),
             ('displayName', 'template', '{given} {surname}'),
             ('mail', 'email', NULL),
             ('department', 'department_name', NULL),
             ('title', 'title', NULL),
             ('manager', 'manager_account', NULL),
             ('employeeID', 'employee_number', NULL)) AS m (attr, src, txt)
WHERE t.kind = 'ad';
