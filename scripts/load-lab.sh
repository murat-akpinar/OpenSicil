#!/usr/bin/env bash
# N-03 yük testi (docs/08, ADR-016/047/051): lab Samba'ya ayrı bir ağaç altında
# 20.000 hesap + 10.000 grup, veritabanına 1.000 rol / 500 departman / 20.000
# yönetilen bağlantı kurulur; ölçülenler: katalog yenileme, gece mutabakatı
# (≤ 30 dk), mutabakat sürerken tek kimlik işi, 2.000 kimliklik değişiklik seti
# (sahneleme → onay → işler, ≤ 30 dk), set sürerken tek kimlik işi (N-11 ≤ 60 sn),
# arama ve sayfa süreleri (N-12 ≤ 1 sn), süreç ve container kaynak kullanımı.
# Gerekenler e2e-lab.sh ile aynı: opensicil-test-pg (15432), lab Keycloak (8081),
# lab Samba AD (6360, seed.sh uygulanmış), samba-lab/tls. Saatlik verme freni
# ölçüm için kaldırılır (HOURLY_GRANT_LIMIT); üretimde fren belirleyicidir (docs/10 #16).
# Ölçek: N_IDENTITIES N_ROLES N_DEPARTMENTS N_GROUPS N_SET; PROFILE=release|debug.
set -euo pipefail
cd "$(dirname "$0")/.."

N_IDENTITIES=${N_IDENTITIES:-20000}; N_ROLES=${N_ROLES:-1000}; N_DEPARTMENTS=${N_DEPARTMENTS:-500}
N_GROUPS=${N_GROUPS:-10000}; N_SET=${N_SET:-2000}; PROFILE=${PROFILE:-release}
DB=opensicil_load
PG_HOST=localhost:15432
BASE=http://localhost:8000
SAMBA=${SAMBA_CONTAINER:-opensicil-samba-ad-1}
SAM_LDB=/var/lib/samba/private/sam.ldb
LOAD_OU="OU=Yuk,DC=opensicil,DC=lab"
USER_OU="OU=Personel,$LOAD_OU"
PASSIVE_OU="OU=Pasif,$USER_OU"
GROUP_OU="OU=Gruplar,$LOAD_OU"
AD_LAB_PASSWORD=${AD_LAB_PASSWORD:-lab-only-not-secret-Aa1}
AEAD_MASTER_KEY=$(head -c 32 /dev/urandom | base64)
BLIND_INDEX_KEY=$(head -c 32 /dev/urandom | base64)
# 5.000–50.000 satırı (docs/09); verme sayacı ölçüm için set büyüklüğünün üstünde
# sayac sinirlari migration'dan sonra Ayarlar tablosuna yazilir (ADR-131)
COMMON=(AEAD_MASTER_KEY="$AEAD_MASTER_KEY" BLIND_INDEX_KEY="$BLIND_INDEX_KEY")
WORK=$(mktemp -d)
BACKEND_PID=""; WORKER_PID=""; SAMPLER_PID=""
RESULTS=()

psql() { docker exec -i opensicil-test-pg psql -U testuser -v ON_ERROR_STOP=1 -qtA "$@"; }
q() { psql -d "$DB" -c "$1"; }
samba() { docker exec "$SAMBA" "$@"; }
cookie_of() { grep -i "^set-cookie: $1=" "$2" | head -1 | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r'; }
now_ms() { date +%s%3N; }
record() { RESULTS+=("$1"); echo "  → $1"; }
peak_rss_mb() { awk '/VmHWM/ {printf "%d", $2 / 1024}' "/proc/$1/status" 2>/dev/null || echo "?"; }

cleanup() {
  [ -n "$SAMPLER_PID" ] && kill "$SAMPLER_PID" 2>/dev/null || true
  [ -n "$WORKER_PID" ] && kill "$WORKER_PID" 2>/dev/null || true
  [ -n "$BACKEND_PID" ] && kill "$BACKEND_PID" 2>/dev/null || true
  samba samba-tool ou delete "$LOAD_OU" --force-subtree-delete >/dev/null 2>&1 || true
  psql -d postgres -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---- 1. binary, veritabanı, şema ----
echo "1) binary'ler ($PROFILE) ve boş veritabanı"
case "$PROFILE" in
  release) (cd backend && cargo build --release -q) && (cd worker && cargo build --release -q) ;;
  *) (cd backend && cargo build -q) && (cd worker && cargo build -q) ;;
esac
psql -d postgres -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >/dev/null
psql -d postgres -c "CREATE DATABASE $DB" >/dev/null
(cd backend && DATABASE_URL="postgres://testuser:testpass@$PG_HOST/$DB" \
  POSTGRES_BACKEND_USER=load_backend POSTGRES_BACKEND_PASSWORD=load-backend-pw \
  POSTGRES_WORKER_USER=load_worker POSTGRES_WORKER_PASSWORD=load-worker-pw \
  "target/$PROFILE/backend" migrate >"$WORK/migrate.log")
# ADR-131: worker'in kapsami (yuk agaci) ve canli mod Ayarlar tablosunda
q "UPDATE operational_settings s SET value = v.value FROM (VALUES ('DRY_RUN', 'false'), ('AD_MANAGED_USER_OUS', '$USER_OU'), ('AD_PASSIVE_OU', '$PASSIVE_OU'), ('AD_MANAGED_GROUP_OUS', '$GROUP_OU'), ('HOURLY_DESTRUCTIVE_LIMIT', '500'), ('HOURLY_GRANT_LIMIT', '$((N_SET * 2))'), ('HOURLY_FIRST_PASSWORD_LIMIT', '300'), ('EMERGENCY_QUOTA', '20')) v(key, value) WHERE s.key = v.key" >/dev/null

# ---- 2. AD ağacı: hesaplar ve gruplar (LDIF + ldbadd, samba-tool'dan yüz kat hızlı) ----
echo "2) lab AD'de yük ağacı: $N_IDENTITIES hesap, $N_GROUPS grup"
samba samba-tool ou delete "$LOAD_OU" --force-subtree-delete >/dev/null 2>&1 || true
for ou in "$LOAD_OU" "$USER_OU" "$PASSIVE_OU" "$GROUP_OU"; do samba samba-tool ou add "$ou" >/dev/null; done
awk -v n="$N_IDENTITIES" -v ou="$USER_OU" -v nd="$N_DEPARTMENTS" 'BEGIN {
  for (i = 1; i <= n; i++) {
    s = sprintf("%05d", i); d = ((i - 1) % nd) + 1
    printf "dn: CN=yuk.%s,%s\nobjectClass: user\nsAMAccountName: yuk.%s\n", s, ou, s
    printf "userPrincipalName: yuk.%s@opensicil.lab\ngivenName: Yuk\nsn: Personel %s\n", s, s
    printf "displayName: Yuk Personel %s\nemployeeID: YUK-%s\ndepartment: Yuk Departmani %d\n", s, s, d
    printf "userAccountControl: 512\n\n"
  }
}' >"$WORK/users.ldif"
awk -v n="$N_GROUPS" -v ou="$GROUP_OU" 'BEGIN {
  printf "dn: CN=GG-Yuk-VPN,%s\nobjectClass: group\nsAMAccountName: GG-Yuk-VPN\ngroupType: -2147483646\n\n", ou
  for (i = 1; i <= n; i++) {
    s = sprintf("%05d", i)
    printf "dn: CN=GG-Yuk-%s,%s\nobjectClass: group\nsAMAccountName: GG-Yuk-%s\ngroupType: -2147483646\n\n", s, ou, s
  }
}' >"$WORK/groups.ldif"
docker cp "$WORK/users.ldif" "$SAMBA:/tmp/yuk-users.ldif" >/dev/null
docker cp "$WORK/groups.ldif" "$SAMBA:/tmp/yuk-groups.ldif" >/dev/null
T0=$(now_ms); samba ldbadd -H $SAM_LDB /tmp/yuk-users.ldif >/dev/null; T_USERS=$(( $(now_ms) - T0 ))
T0=$(now_ms); samba ldbadd -H $SAM_LDB /tmp/yuk-groups.ldif >/dev/null; T_GROUPS=$(( $(now_ms) - T0 ))
samba rm -f /tmp/yuk-users.ldif /tmp/yuk-groups.ldif
record "AD seed: $N_IDENTITIES hesap $((T_USERS / 1000)) sn, $((N_GROUPS + 1)) grup $((T_GROUPS / 1000)) sn (ldbadd)"
# objectGUID'ler bağlantı için (docs/05: tireli yazım, ldbsearch zaten öyle basar)
samba ldbsearch -H $SAM_LDB -b "$USER_OU" -s one '(objectClass=user)' sAMAccountName objectGUID 2>/dev/null \
  | awk '/^dn:/ {s = ""; g = ""} /^sAMAccountName:/ {s = $2} /^objectGUID:/ {g = $2}
         /^$/ {if (s != "" && g != "") print s "," g; s = ""; g = ""}' >"$WORK/guids.csv"
[ "$(wc -l <"$WORK/guids.csv")" = "$N_IDENTITIES" ] || { echo "GUID sayısı tutmadı: $(wc -l <"$WORK/guids.csv")"; exit 1; }

# ---- 3. backend, Yapılandırma ----
echo "3) backend (servis rolüyle) ve Yapılandırma: lab AD + lab Keycloak"
env "${COMMON[@]}" DATABASE_URL="postgres://load_backend:load-backend-pw@$PG_HOST/$DB" \
  PUBLIC_URL=$BASE METRICS_TOKEN=load APPROVAL_TIMELOCK_HOURS=0 \
  "backend/target/$PROFILE/backend" >"$WORK/backend.log" 2>&1 &
BACKEND_PID=$!
for _ in $(seq 1 30); do curl -sf "$BASE/api/health" >/dev/null && break; sleep 1; done
curl -sf "$BASE/api/health" >/dev/null || { cat "$WORK/backend.log"; exit 1; }
curl -s -D "$WORK/h" -o /dev/null -d 'username=admin&password=admin' "$BASE/login"
ADMIN=$(cookie_of opensicil_operator_session "$WORK/h")
[ -n "$ADMIN" ] || { echo "yerel giriş başarısız"; exit 1; }
curl -s -o /dev/null -b "$ADMIN" -d 'new_password=load-bootstrap-parola-1&confirm_password=load-bootstrap-parola-1' "$BASE/change-password"
curl -s -o /dev/null -b "$ADMIN" \
  --data-urlencode "ad_host=localhost:6360" \
  --data-urlencode "ad_bind_dn=CN=Administrator,CN=Users,DC=opensicil,DC=lab" \
  --data-urlencode "ad_service_password=$AD_LAB_PASSWORD" \
  --data-urlencode "ad_ca_pem@samba-lab/tls/ca.pem" \
  --data-urlencode "zimbra_url=" --data-urlencode "zimbra_admin_password=" \
  --data-urlencode "oidc_issuer=http://localhost:8081/realms/opensicil" \
  --data-urlencode "oidc_client_id=opensicil-backend" \
  --data-urlencode "oidc_client_secret=lab-only-not-secret" "$BASE/config"

# ---- 4. model: roller, departmanlar, kimlikler, bağlantılar ----
echo "4) model seed: $N_ROLES rol, $N_DEPARTMENTS departman, $N_IDENTITIES kimlik ve yönetilen bağlantı"
AD=$(q "SELECT id FROM target_systems WHERE kind = 'ad'")
q "INSERT INTO roles (kind, name, slug) VALUES ('base', 'Herkes', 'herkes');
INSERT INTO roles (kind, name, slug)
  SELECT 'primary', 'Yuk Rolu ' || i, 'yuk-rolu-' || i FROM generate_series(1, $N_ROLES - 1) i;
INSERT INTO departments (name, code, slug)
  SELECT 'Yuk Departmani ' || i, 'YD-' || i, 'yuk-departmani-' || i FROM generate_series(1, $N_DEPARTMENTS) i;
-- ilk N_SET kimlik 1. rolde (değişiklik seti bu rolü düzenler), kalanlar sırayla diğer rollerde
WITH r AS (SELECT array_agg(id ORDER BY id) ids FROM roles WHERE kind = 'primary' AND NOT placeholder),
     d AS (SELECT array_agg(id ORDER BY id) ids FROM departments)
INSERT INTO identities (given_name, surname, employee_number, department_id, primary_role_id,
                        employment_type, start_date, username, upn)
  SELECT 'Yuk', 'Personel ' || to_char(i, 'FM00000'), 'YUK-' || to_char(i, 'FM00000'),
         d.ids[((i - 1) % $N_DEPARTMENTS) + 1],
         CASE WHEN i <= $N_SET THEN r.ids[1] ELSE r.ids[((i - $N_SET - 1) % ($N_ROLES - 2)) + 2] END,
         'permanent', current_date - 30, 'yuk.' || to_char(i, 'FM00000'),
         'yuk.' || to_char(i, 'FM00000') || '@opensicil.lab'
  FROM generate_series(1, $N_IDENTITIES) i, r, d;
CREATE TABLE yuk_guids (sam TEXT PRIMARY KEY, guid TEXT NOT NULL);" >/dev/null
psql -d "$DB" -c "COPY yuk_guids FROM STDIN WITH (FORMAT csv)" <"$WORK/guids.csv" >/dev/null
q "INSERT INTO account_links (identity_id, target_system_id, external_id, origin, mode, applied_state)
     SELECT i.id, $AD, g.guid, 'adopted', 'managed', 'active'
     FROM identities i JOIN yuk_guids g ON g.sam = i.username;
   DROP TABLE yuk_guids;" >/dev/null
LINKS=$(q "SELECT count(*) FROM account_links")
[ "$LINKS" = "$N_IDENTITIES" ] || { echo "bağlantı sayısı tutmadı: $LINKS"; exit 1; }
ROLE_KEY=$(q "SELECT slug FROM roles WHERE kind = 'primary' AND NOT placeholder ORDER BY id LIMIT 1")
DEPT=$(q "SELECT id FROM departments ORDER BY id LIMIT 1")
ROLE=$(q "SELECT id FROM roles WHERE slug = '$ROLE_KEY'")

# ---- 5. worker ve kaynak örnekleyici ----
echo "5) worker (servis rolüyle, canlı mod; kapsam = yük ağacı) → açılışta katalog"
env "${COMMON[@]}" DATABASE_URL="postgres://load_worker:load-worker-pw@$PG_HOST/$DB" \
  "worker/target/$PROFILE/worker" >"$WORK/worker.log" 2>&1 &
WORKER_PID=$!
( while true; do
    docker stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}' "$SAMBA" opensicil-test-pg 2>/dev/null
    ps -o pid=,pcpu=,rss= -p "$BACKEND_PID,$WORKER_PID" 2>/dev/null | sed 's/^ */proc /'
    sleep 5
  done >"$WORK/stats.log" ) &
SAMPLER_PID=$!
wait_read_job() { # $1 tür → bitince süre (sn), kuyruğa yazılmamışsa boş
  for _ in $(seq 1 720); do
    local row; row=$(q "SELECT status || ' ' || coalesce(EXTRACT(EPOCH FROM finished_at - started_at)::int::text, '') \
      FROM read_jobs WHERE kind = '$1' AND target_system_id = $AD ORDER BY id DESC LIMIT 1")
    case "$row" in succeeded*) echo "${row#succeeded }"; return 0;; failed*) echo "FAILED"; return 0;; esac
    sleep 5
  done
  echo "TIMEOUT"
}
T_CATALOG=$(wait_read_job catalog_refresh)
ITEMS=$(q "SELECT count(*) FROM catalog_items WHERE missing_since IS NULL")
record "katalog yenileme ($ITEMS öğe): $T_CATALOG sn"
q "UPDATE target_systems SET default_container_item_id = (SELECT id FROM catalog_items WHERE kind = 'ou' AND display_name = 'Personel' AND target_system_id = $AD) WHERE id = $AD" >/dev/null
# gece koşusu bu açılışta kendiliğinden açıldıysa bitmesini bekle; ölçüm ayrıca istenir
[ -z "$(q "SELECT 1 FROM read_jobs WHERE kind = 'reconcile' AND status IN ('queued', 'running')")" ] || wait_read_job reconcile >/dev/null

# ---- 6. OIDC girişleri ----
echo "6) OIDC girişleri: test-hr (kayıt) ve test-admin (onay)"
oidc_login() { # $1 kullanıcı $2 parola → oturum çerezi
  local auth action callback
  auth=$(curl -s -o /dev/null -w '%{redirect_url}' "$BASE/oidc/login")
  action=$(curl -s -c "$WORK/kc-$1" "$auth" | tr -d '\n' | grep -o 'id="kc-form-login".\{0,600\}' | grep -o 'action="[^"]*"' | head -1 | sed 's/action="//; s/"$//; s/&amp;/\&/g')
  callback=$(curl -s -o /dev/null -w '%{redirect_url}' -b "$WORK/kc-$1" -c "$WORK/kc-$1" --data-urlencode "username=$1" --data-urlencode "password=$2" "$action")
  curl -s -D "$WORK/h-$1" -o /dev/null "$callback"
  cookie_of opensicil_operator_session "$WORK/h-$1"
}
HR=$(oidc_login test-hr test-hr-pw); [ -n "$HR" ] || { echo "test-hr girişi başarısız"; exit 1; }
APPROVER=$(oidc_login test-admin test-admin-pw); [ -n "$APPROVER" ] || { echo "test-admin girişi başarısız"; exit 1; }

register() { # $1 soyad → kişi sayfası adresi; N-13 yolu, start bugün
  curl -s -o "$WORK/form.html" -w '%{redirect_url}' -b "$HR" \
    --data-urlencode given_name=Yuk --data-urlencode "surname=$1" --data-urlencode national_id_country=TR \
    --data-urlencode "employee_number=YUK-T-$1" --data-urlencode department_id="$DEPT" \
    --data-urlencode primary_role_id="$ROLE" --data-urlencode employment_type=permanent \
    --data-urlencode start_date="$(date +%F)" "$BASE/identities"
}
wait_opened() { # $1 kişi sayfası → 'açıldı' görünene kadar geçen sn
  local t0; t0=$(now_ms)
  for _ in $(seq 1 300); do
    curl -s -b "$HR" "$1" | grep -q 'açıldı' && { echo $(( ($(now_ms) - t0 + 500) / 1000 )); return 0; }
    sleep 1
  done
  echo "TIMEOUT"
}

# ---- 7. gece mutabakatı ve o sırada tek kimlik işi ----
echo "7) mutabakat taraması ($N_IDENTITIES hesap ↔ $N_IDENTITIES bağlantı) ve sürerken tek kimlik işi"
curl -s -o /dev/null -b "$ADMIN" -X POST "$BASE/targets/$AD/reconcile/scan"
for _ in $(seq 1 60); do [ "$(q "SELECT status FROM read_jobs WHERE kind = 'reconcile' ORDER BY id DESC LIMIT 1")" = running ] && break; sleep 1; done
PAGE1=$(register Tarama); [ -n "$PAGE1" ] || { echo "kayıt reddedildi:"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/form.html"; exit 1; }
T_SINGLE_SCAN=$(wait_opened "$PAGE1")
T_RECONCILE=$(wait_read_job reconcile)
FINDINGS=$(q "SELECT string_agg(kind || '=' || n, ' ') FROM (SELECT kind, count(*) n FROM reconcile_findings GROUP BY kind ORDER BY kind) f")
record "gece mutabakatı: $T_RECONCILE sn (hedef ≤ 1800) — bulgu $FINDINGS"
record "mutabakat sürerken tek kimlik işi (kayıt → 'açıldı'): $T_SINGLE_SCAN sn"

# ---- 8. değişiklik seti: rol 1'e grup → sahneleme → onay → N_SET iş; sürerken tek kimlik (N-11) ----
echo "8) değişiklik seti: '$ROLE_KEY' rolüne GG-Yuk-VPN → sahneleme → onay → $N_SET iş"
VPN=$(q "SELECT id FROM catalog_items WHERE kind = 'group' AND display_name = 'GG-Yuk-VPN'")
T0=$(now_ms)
curl -s -o "$WORK/stage.html" -b "$ADMIN" --data-urlencode "name=Yuk Rolu 1" --data-urlencode "title=" \
  --data-urlencode "entitlement=$VPN" --data-urlencode "pa.$AD=true" "$BASE/roles/$ROLE_KEY"
T_STAGE=$(( $(now_ms) - T0 ))
[ "$(q "SELECT pending_definition IS NOT NULL FROM roles WHERE id = $ROLE")" = t ] || { echo "taslak sahnelenmedi"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/stage.html" | head -5; exit 1; }
T0=$(now_ms)
curl -s -o "$WORK/approve.html" -b "$APPROVER" -X POST "$BASE/roles/$ROLE_KEY/approve"
T_APPROVE=$(( $(now_ms) - T0 ))
AFFECTED=$(q "SELECT count(*) FROM identities WHERE primary_role_id = $ROLE AND deleted_at IS NULL")
BULK=$(q "SELECT count(*) FROM jobs WHERE priority = 2 AND target_system_id = $AD")
[ "$BULK" = "$AFFECTED" ] || { echo "beklenen $AFFECTED toplu iş, açılan: $BULK"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/approve.html" | head -5; exit 1; }
record "değişiklik seti: sahneleme POST $T_STAGE ms, onay POST $T_APPROVE ms ($BULK AD işi açıldı)"
sleep 10
PAGE2=$(register Oncelik); [ -n "$PAGE2" ] || { echo "kayıt reddedildi:"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/form.html"; exit 1; }
T_N11=$(wait_opened "$PAGE2")
record "N-11 toplu set sürerken tek kimlik işi: $T_N11 sn (hedef ≤ 60)"
for _ in $(seq 1 720); do [ "$(q "SELECT count(*) FROM jobs WHERE status <> 'succeeded'")" = 0 ] && break; sleep 5; done
T_SET=$(q "SELECT EXTRACT(EPOCH FROM max(finished_at) - min(created_at))::int FROM jobs WHERE priority = 2 AND target_system_id = $AD")
INTERVENTION=$(q "SELECT count(*) FROM jobs WHERE status = 'needs_intervention'")
MEMBERS=$(samba ldbsearch -H $SAM_LDB -b "$GROUP_OU" '(sAMAccountName=GG-Yuk-VPN)' member 2>/dev/null | grep -c '^member: ' || true)
record "değişiklik seti $N_SET iş: $T_SET sn (hedef ≤ 1800; $(awk -v n="$N_SET" -v t="$T_SET" 'BEGIN {printf "%.1f", (t > 0 ? n / t : 0)}') iş/sn); müdahale $INTERVENTION; GG-Yuk-VPN üye $MEMBERS"

# ---- 9. N-12 sayfa süreleri ----
echo "9) N-12: $N_IDENTITIES kimlikte arama ve sayfalar"
ms_of() { curl -s -o /dev/null -w '%{time_total}' -b "$HR" "$1" | awk '{printf "%d", $1 * 1000}'; }
record "N-12 arama 'Personel 19999': $(ms_of "$BASE/identities?q=Personel+19999") ms; 'YUK-00042': $(ms_of "$BASE/identities?q=YUK-00042") ms (hedef ≤ 1000)"
record "sayfalar: personel listesi $(ms_of "$BASE/identities") ms, panel $(ms_of "$BASE/") ms, roller $(ms_of "$BASE/roles") ms, mutabakat sayfası $(ms_of "$BASE/targets/$AD/reconcile") ms, rol sayfası $(ms_of "$BASE/roles/$ROLE_KEY") ms"

# ---- 10. kaynaklar ----
kill "$SAMPLER_PID" 2>/dev/null || true; SAMPLER_PID=""
peak() { awk -v name="$1" '$1 == name {v = $3; if (v ~ /GiB/) {sub(/GiB.*/, "", v); v *= 1024} else sub(/MiB.*/, "", v); if (v + 0 > m) m = v + 0} END {printf "%d", m}' "$WORK/stats.log"; }
record "kaynak (tepe): backend RSS $(peak_rss_mb "$BACKEND_PID") MB, worker RSS $(peak_rss_mb "$WORKER_PID") MB, Samba $(peak "$SAMBA") MiB, Postgres $(peak opensicil-test-pg) MiB"
record "worker log: $(grep -c 'müdahale\|hata\|başarısız' "$WORK/worker.log" || true) hata satırı; backend log: $(grep -ci 'error\|hata' "$WORK/backend.log" || true)"

echo
echo "N-03 ÖLÇÜM ($N_IDENTITIES kimlik / $N_ROLES rol / $N_DEPARTMENTS departman / $N_GROUPS grup, $PROFILE)"
printf '  %s\n' "${RESULTS[@]}"
FAIL=0
[ "$T_RECONCILE" != TIMEOUT ] && [ "$T_RECONCILE" != FAILED ] && [ "$T_RECONCILE" -le 1800 ] || FAIL=1
[ "$T_SET" -le 1800 ] || FAIL=1
[ "$T_N11" != TIMEOUT ] && [ "$T_N11" -le 60 ] || FAIL=1
[ "$INTERVENTION" = 0 ] || FAIL=1
[ "$FAIL" = 0 ] && echo "N-03 TAMAM: hedefler karşılandı" || { echo "N-03 HEDEF AŞILDI"; exit 1; }
