#!/usr/bin/env bash
# Uçtan uca lab doğrulaması (Faz 3a, ADR-079): kayıt → AD'de pasif hesap → ekranda "açıldı";
# Faz 3d (ADR-056): "kaydet ve ilk parolayı ver" → parola ekranda, N-13 süresi ölçülür.
# Gerekenler: opensicil-test-pg (15432), lab Keycloak (8081), lab Samba AD (6360,
# seed.sh uygulanmış), samba-lab/tls (gen-tls.sh). Backend ve worker gerçek binary
# olarak, servis rolleriyle çalışır; sonunda hesap ve veritabanı temizlenir.
set -euo pipefail
cd "$(dirname "$0")/.."

DB=opensicil_e2e
PG_HOST=localhost:15432
BASE=http://localhost:8000
AD_LAB_PASSWORD=${AD_LAB_PASSWORD:-lab-only-not-secret-Aa1}
AEAD_MASTER_KEY=$(head -c 32 /dev/urandom | base64)
BLIND_INDEX_KEY=$(head -c 32 /dev/urandom | base64)
# ortak ayarlar tablonun seed'inden gelir (ADR-131); env'de yalnizca anahtarlar
COMMON=(AEAD_MASTER_KEY="$AEAD_MASTER_KEY" BLIND_INDEX_KEY="$BLIND_INDEX_KEY")
WORK=$(mktemp -d)
BACKEND_PID=""; WORKER_PID=""; USERNAME=""; USERNAME2=""

psql() { docker exec -i opensicil-test-pg psql -U testuser -v ON_ERROR_STOP=1 -qtA "$@"; }
cookie_of() { grep -i "^set-cookie: $1=" "$2" | head -1 | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r'; }
cleanup() {
  [ -n "$WORKER_PID" ] && kill "$WORKER_PID" 2>/dev/null || true
  [ -n "$BACKEND_PID" ] && kill "$BACKEND_PID" 2>/dev/null || true
  [ -n "$USERNAME" ] && docker exec opensicil-samba-ad-1 samba-tool user delete "$USERNAME" >/dev/null 2>&1 || true
  [ -n "$USERNAME2" ] && docker exec opensicil-samba-ad-1 samba-tool user delete "$USERNAME2" >/dev/null 2>&1 || true
  psql -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "1) binary'ler ve boş veritabanı"
(cd backend && cargo build -q) && (cd worker && cargo build -q)
# yarıda kalan önceki çalışmanın hesapları (ad şablonu sabit: uctan.uca, parola.teslim)
for stale in uctan.uca parola.teslim; do
  docker exec opensicil-samba-ad-1 samba-tool user delete "$stale" >/dev/null 2>&1 || true
done
psql -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null
psql -d postgres -c "CREATE DATABASE $DB" >/dev/null
# migrate ./migrations'ı çalışma dizininden okur (imajda WORKDIR backend'dir)
(cd backend && DATABASE_URL="postgres://testuser:testpass@$PG_HOST/$DB" \
  POSTGRES_BACKEND_USER=e2e_backend POSTGRES_BACKEND_PASSWORD=e2e-backend-pw \
  POSTGRES_WORKER_USER=e2e_worker POSTGRES_WORKER_PASSWORD=e2e-worker-pw \
  target/debug/backend migrate >"$WORK/migrate.log")
# ADR-131: worker'in kapsami ve kuru calistirmasi Ayarlar tablosunda
psql -d $DB -c "UPDATE operational_settings s SET value = v.value FROM (VALUES ('DRY_RUN', 'false'), ('AD_MANAGED_USER_OUS', 'OU=Personel,DC=opensicil,DC=lab'), ('AD_PASSIVE_OU', 'OU=Pasif,OU=Personel,DC=opensicil,DC=lab'), ('AD_MANAGED_GROUP_OUS', 'OU=Gruplar,DC=opensicil,DC=lab')) v(key, value) WHERE s.key = v.key" >/dev/null

echo "2) backend (servis rolüyle)"
env "${COMMON[@]}" DATABASE_URL="postgres://e2e_backend:e2e-backend-pw@$PG_HOST/$DB" \
  PUBLIC_URL=$BASE METRICS_TOKEN=e2e APPROVAL_TIMELOCK_HOURS=0 \
  backend/target/debug/backend >"$WORK/backend.log" 2>&1 &
BACKEND_PID=$!
for _ in $(seq 1 30); do curl -sf "$BASE/api/health" >/dev/null && break; sleep 1; done
curl -sf "$BASE/api/health" >/dev/null || { cat "$WORK/backend.log"; exit 1; }

echo "3) yerel admin ile Yapılandırma: lab AD + lab Keycloak"
# ADR-095: yerel giris de operator oturumu uretir, tek cerez var.
curl -s -D "$WORK/h" -o /dev/null -d 'username=admin&password=admin' "$BASE/login"
BOOT=$(cookie_of opensicil_operator_session "$WORK/h")
[ -n "$BOOT" ] || { echo "yerel giriş başarısız"; cat "$WORK/h"; exit 1; }
curl -s -o /dev/null -b "$BOOT" -d 'new_password=e2e-bootstrap-parola-1&confirm_password=e2e-bootstrap-parola-1' "$BASE/change-password"
curl -s -o /dev/null -b "$BOOT" \
  --data-urlencode "ad_host=localhost:6360" \
  --data-urlencode "ad_bind_dn=CN=Administrator,CN=Users,DC=opensicil,DC=lab" \
  --data-urlencode "ad_service_password=$AD_LAB_PASSWORD" \
  --data-urlencode "ad_ca_pem@samba-lab/tls/ca.pem" \
  --data-urlencode "zimbra_url=" --data-urlencode "zimbra_admin_password=" \
  --data-urlencode "oidc_issuer=http://localhost:8081/realms/opensicil" \
  --data-urlencode "oidc_client_id=opensicil-backend" \
  --data-urlencode "oidc_client_secret=lab-only-not-secret" "$BASE/config"

echo "4) worker (servis rolüyle, canlı mod) → açılışta katalog"
env "${COMMON[@]}" DATABASE_URL="postgres://e2e_worker:e2e-worker-pw@$PG_HOST/$DB" \
  worker/target/debug/worker >"$WORK/worker.log" 2>&1 &
WORKER_PID=$!
for _ in $(seq 1 30); do
  [ "$(psql -d $DB -c "SELECT count(*) FROM catalog_items WHERE kind = 'ou' AND display_name = 'Personel'")" = "1" ] && break
  sleep 1
done

echo "5) model seed (rol ekranı 3b'de): temel rol, birincil rol, departman, hedef varsayılan OU"
psql -d $DB -c "INSERT INTO roles (kind, name) VALUES ('base', 'Herkes'), ('primary', 'Sistem Uzmanı');
INSERT INTO departments (name) VALUES ('Bilgi İşlem');
UPDATE target_systems SET default_container_item_id = (SELECT id FROM catalog_items WHERE kind = 'ou' AND display_name = 'Personel' LIMIT 1) WHERE kind = 'ad';" >/dev/null
DEPT=$(psql -d $DB -c "SELECT id FROM departments LIMIT 1")
ROLE=$(psql -d $DB -c "SELECT id FROM roles WHERE kind = 'primary' AND NOT placeholder LIMIT 1")

echo "6) OIDC girişi: test-hr (OpenSicil-HR)"
AUTH_URL=$(curl -s -o /dev/null -w '%{redirect_url}' "$BASE/oidc/login")
ACTION=$(curl -s -c "$WORK/kc" "$AUTH_URL" | tr -d '\n' | grep -o 'id="kc-form-login".\{0,600\}' | grep -o 'action="[^"]*"' | head -1 | sed 's/action="//; s/"$//; s/&amp;/\&/g')
CALLBACK=$(curl -s -o /dev/null -w '%{redirect_url}' -b "$WORK/kc" -c "$WORK/kc" \
  --data-urlencode username=test-hr --data-urlencode password=test-hr-pw "$ACTION")
curl -s -D "$WORK/h" -o /dev/null "$CALLBACK"
OP=$(cookie_of opensicil_operator_session "$WORK/h")
[ -n "$OP" ] || { echo "OIDC girişi başarısız"; cat "$WORK/backend.log"; exit 1; }

echo "7) kayıt formu: başlangıç yarın → bekliyor"
TOMORROW=$(date -d tomorrow +%F)
LOCATION=$(curl -s -o "$WORK/form.html" -w '%{redirect_url}' -b "$OP" \
  --data-urlencode given_name=Uçtan --data-urlencode surname=Uca --data-urlencode national_id_country=TR \
  --data-urlencode employee_number=E2E-1 --data-urlencode department_id="$DEPT" \
  --data-urlencode primary_role_id="$ROLE" --data-urlencode employment_type=permanent \
  --data-urlencode start_date="$TOMORROW" "$BASE/identities")
[ -n "$LOCATION" ] || { echo "kayıt reddedildi:"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/form.html"; exit 1; }
ID=${LOCATION##*/}

echo "8) worker işi uygular → kişi sayfasında 'açıldı'"
for _ in $(seq 1 30); do
  curl -s -b "$OP" "$LOCATION" >"$WORK/page.html"
  grep -q 'açıldı' "$WORK/page.html" && break
  sleep 2
done
grep -q 'açıldı' "$WORK/page.html" || { echo "sayfada 'açıldı' görünmedi"; cat "$WORK/worker.log"; exit 1; }
USERNAME=$(psql -d $DB -c "SELECT username FROM identities WHERE id = $ID")

echo "9) Samba'da hesap pasif (userAccountControl 514)"
UAC=$(docker exec opensicil-samba-ad-1 samba-tool user show "$USERNAME" | grep '^userAccountControl:' | awk '{print $2}')
[ "$UAC" = "514" ] || { echo "beklenen 514, gelen: $UAC"; exit 1; }

echo "10) 'Kaydet ve ilk parolayı ver' (ADR-056, N-13 ≤ 60 sn): bugün başlayan kayıt → parola ekranda"
T0=$(date +%s)
FP_URL=$(curl -s -o "$WORK/form2.html" -w '%{redirect_url}' -b "$OP" \
  --data-urlencode given_name=Parola --data-urlencode surname=Teslim --data-urlencode national_id_country=TR \
  --data-urlencode employee_number=E2E-2 --data-urlencode department_id="$DEPT" \
  --data-urlencode primary_role_id="$ROLE" --data-urlencode employment_type=permanent \
  --data-urlencode start_date="$(date +%F)" --data-urlencode issue_first_password=1 "$BASE/identities")
case "$FP_URL" in */first-password/*) ;; *) echo "teslim sayfasına yönlenmedi: '$FP_URL'"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/form2.html"; exit 1;; esac
ID2=$(echo "$FP_URL" | sed 's#.*/identities/\([0-9]*\)/.*#\1#')
USERNAME2=$(psql -d $DB -c "SELECT username FROM identities WHERE id = $ID2" || true)
for _ in $(seq 1 30); do
  curl -s -b "$OP" "$FP_URL" >"$WORK/fp.html"
  grep -qE '[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}' "$WORK/fp.html" && break
  grep -q 'reddedildi\|sıfırlayın\|yanıt vermedi' "$WORK/fp.html" && { echo "ilk parola reddedildi:"; grep -o '<p[^>]*>[^<]*</p>' "$WORK/fp.html"; exit 1; }
  sleep 2
done
PASSWORD=$(grep -oE '[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}' "$WORK/fp.html" | head -1 || true)
ELAPSED=$(( $(date +%s) - T0 ))
[ -n "$PASSWORD" ] || { echo "parola ${ELAPSED} sn içinde görünmedi; sayfa:"; grep -o '<t[dr][^>]*>[^<]*' "$WORK/fp.html" | tr -d '\n'; echo; cat "$WORK/worker.log"; exit 1; }
[ "$ELAPSED" -le 60 ] || { echo "N-13 aşıldı: ${ELAPSED} sn"; exit 1; }
USERNAME2=$(psql -d $DB -c "SELECT username FROM identities WHERE id = $ID2")
curl -s -b "$OP" "$FP_URL" >"$WORK/fp2.html"
grep -q "$PASSWORD" "$WORK/fp2.html" && { echo "parola ikinci açılışta hâlâ görünüyor"; exit 1; }
UAC2=$(docker exec opensicil-samba-ad-1 samba-tool user show "$USERNAME2" | grep '^userAccountControl:' | awk '{print $2}')
[ "$UAC2" = "512" ] || { echo "beklenen 512 (bugün başladı), gelen: $UAC2"; exit 1; }
echo "  N-13: kayıttan parolaya ${ELAPSED} sn; parola bir kez gösterildi; AD'de etkin hesap (UAC $UAC2)"

echo
echo "E2E TAMAM: kimlik #$ID ($USERNAME) → AD'de pasif hesap (UAC $UAC) → ekranda 'açıldı'; kimlik #$ID2 ($USERNAME2) → tek adımda ilk parola (${ELAPSED} sn)"
grep -o 'olması gereken[^<]*\|uyumlu\|bekliyor' "$WORK/page.html" | sort -u | sed 's/^/  sayfa: /'
grep 'zamanlayıcı\|iş [0-9]* ' "$WORK/worker.log" | sed 's/^/  worker: /' || true
