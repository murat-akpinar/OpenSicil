#!/usr/bin/env bash
# Mutabakat taramasinin GERCEK Windows AD'ye karsi uctan uca dogrulanmasi
# (ADR-099). Salt okuma: AD'ye hicbir sey yazmaz, hicbir sey degistirmez.
#
# Gerekenler: opensicil-test-pg (15432), gercek AD'ye ag erisimi, CA dosyasi
# (tmp/hogwarts-ca.pem) ve baglanti bilgisi (tmp/lab-ad-notlari.md).
# Backend ve worker gercek binary olarak, servis rolleriyle; sonunda
# veritabani silinir. Calisan compose yiginina ve .env'e DOKUNMAZ.
set -euo pipefail
cd "$(dirname "$0")/.."

DB=opensicil_hogwarts
PG_HOST=localhost:15432
BASE=http://localhost:8000
AD_HOST=DUMBLEDORE-DC01.hogwarts.local:636
AD_BIND_DN=open.sicil@hogwarts.local
AD_CA=$PWD/tmp/hogwarts-ca.pem
SCOPE_USERS="OU=Hogwarts,DC=hogwarts,DC=local"
SCOPE_GROUPS="OU=Groups,OU=Hogwarts,DC=hogwarts,DC=local"

AD_PASSWORD=$(sed -n 's/^- Parola: //p' tmp/lab-ad-notlari.md | head -1)
[ -n "$AD_PASSWORD" ] || { echo "parola tmp/lab-ad-notlari.md'de yok" >&2; exit 1; }

AEAD_MASTER_KEY=$(head -c 32 /dev/urandom | base64)
BLIND_INDEX_KEY=$(head -c 32 /dev/urandom | base64)
COMMON=(OWNERSHIP_MODE_ENABLED=false HOURLY_DESTRUCTIVE_LIMIT=50 HOURLY_GRANT_LIMIT=50
        HOURLY_FIRST_PASSWORD_LIMIT=50 EMERGENCY_QUOTA=5 SENSITIVE_MAPPING_ENABLED=false
        TZ=Europe/Istanbul AEAD_MASTER_KEY="$AEAD_MASTER_KEY" BLIND_INDEX_KEY="$BLIND_INDEX_KEY")
WORK=$(mktemp -d)
BACKEND_PID=""; WORKER_PID=""

psql() { docker exec -i opensicil-test-pg psql -U testuser -v ON_ERROR_STOP=1 -qtA "$@"; }
cookie_of() { grep -i "^set-cookie: $1=" "$2" | head -1 | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r'; }
cleanup() {
  [ -n "$WORKER_PID" ] && kill "$WORKER_PID" 2>/dev/null || true
  [ -n "$BACKEND_PID" ] && kill "$BACKEND_PID" 2>/dev/null || true
  psql -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "1) binary'ler ve boş veritabanı"
(cd backend && cargo build -q) && (cd worker && cargo build -q)
psql -d postgres -c "DROP DATABASE IF EXISTS $DB" >/dev/null
psql -d postgres -c "CREATE DATABASE $DB" >/dev/null
(cd backend && DATABASE_URL="postgres://testuser:testpass@$PG_HOST/$DB" \
  POSTGRES_BACKEND_USER=hw_backend POSTGRES_BACKEND_PASSWORD=hw-backend-pw \
  POSTGRES_WORKER_USER=hw_worker POSTGRES_WORKER_PASSWORD=hw-worker-pw \
  target/debug/backend migrate >"$WORK/migrate.log")

echo "2) backend (servis rolüyle)"
env "${COMMON[@]}" DATABASE_URL="postgres://hw_backend:hw-backend-pw@$PG_HOST/$DB" \
  PUBLIC_URL=$BASE METRICS_TOKEN=hw APPROVAL_TIMELOCK_HOURS=0 \
  backend/target/debug/backend >"$WORK/backend.log" 2>&1 &
BACKEND_PID=$!
for _ in $(seq 1 30); do curl -sf "$BASE/api/health" >/dev/null && break; sleep 1; done
curl -sf "$BASE/api/health" >/dev/null || { cat "$WORK/backend.log"; exit 1; }

echo "3) Yapılandırma: gerçek Windows AD"
curl -s -D "$WORK/h" -o /dev/null -d 'username=admin&password=admin' "$BASE/login"
BOOT=$(cookie_of opensicil_operator_session "$WORK/h")
[ -n "$BOOT" ] || { echo "yerel giriş başarısız"; exit 1; }
curl -s -o /dev/null -b "$BOOT" -d 'new_password=hogwarts-bootstrap-1&confirm_password=hogwarts-bootstrap-1' "$BASE/change-password"
curl -s -o /dev/null -b "$BOOT" \
  --data-urlencode "ad_host=$AD_HOST" \
  --data-urlencode "ad_bind_dn=$AD_BIND_DN" \
  --data-urlencode "ad_service_password=$AD_PASSWORD" \
  --data-urlencode "zimbra_url=" --data-urlencode "zimbra_admin_password=" \
  --data-urlencode "oidc_issuer=" --data-urlencode "oidc_client_id=" \
  --data-urlencode "oidc_client_secret=" "$BASE/config"

echo "4) worker (kuru çalıştırma: hedefe yazma ihtimali bile yok)"
env "${COMMON[@]}" DATABASE_URL="postgres://hw_worker:hw-worker-pw@$PG_HOST/$DB" \
  DRY_RUN=true FIRST_LOGIN_CHANGE_REQUIRED=true AD_CA_FILE="$AD_CA" \
  AD_MANAGED_USER_OUS="$SCOPE_USERS" AD_MANAGED_GROUP_OUS="$SCOPE_GROUPS" AD_PASSIVE_OU="" \
  worker/target/debug/worker >"$WORK/worker.log" 2>&1 &
WORKER_PID=$!

echo "5) katalog ve mutabakat taraması"
TARGET=$(psql -d "$DB" -c "SELECT id FROM target_systems WHERE kind = 'ad'")
psql -d "$DB" -c "INSERT INTO read_jobs (kind, target_system_id) VALUES ('reconcile', $TARGET)" >/dev/null
for _ in $(seq 1 60); do
  DONE=$(psql -d "$DB" -c "SELECT count(*) FROM read_jobs WHERE kind='reconcile' AND status IN ('succeeded','failed')")
  [ "$DONE" = "1" ] && break
  sleep 2
done

echo
echo "--- katalog ---"
psql -d "$DB" -c "SELECT result FROM read_jobs WHERE kind='catalog_refresh' ORDER BY id DESC LIMIT 1"
echo "--- mutabakat ---"
psql -d "$DB" -c "SELECT status || ': ' || COALESCE(result,'(sonuç yok)') FROM read_jobs WHERE kind='reconcile'"
echo
echo "--- sınıf dağılımı ---"
psql -d "$DB" -c "SELECT kind || '  ' || count(*) FROM reconcile_findings GROUP BY kind ORDER BY 1"
echo
echo "--- hesaplar (ekranda görünecek liste) ---"
psql -d "$DB" -c "SELECT rpad(account_name, 14) || rpad(COALESCE(display_name,''), 22) || \
  CASE WHEN enabled THEN 'etkin ' ELSE 'pasif ' END || container \
  FROM reconcile_findings ORDER BY container, account_name"

echo
echo "--- kişi alanları: 29 hesapta kaçı dolu? (ADR-106) ---"
psql -d "$DB" -c "SELECT 'toplam          ' || count(*) FROM reconcile_findings \
  UNION ALL SELECT 'mail            ' || count(*) FROM reconcile_findings WHERE mail <> '' \
  UNION ALL SELECT 'mobile          ' || count(*) FROM reconcile_findings WHERE mobile <> '' \
  UNION ALL SELECT 'telephoneNumber ' || count(*) FROM reconcile_findings WHERE telephone_number <> '' \
  UNION ALL SELECT 'sicil           ' || count(*) FROM reconcile_findings WHERE employee_number <> '' \
  UNION ALL SELECT 'whenCreated     ' || count(*) FROM reconcile_findings WHERE when_created IS NOT NULL \
  UNION ALL SELECT 'TC (sifreli)    ' || count(*) FROM reconcile_findings WHERE national_id_enc IS NOT NULL \
  UNION ALL SELECT 'departman       ' || count(*) FROM reconcile_findings WHERE department_name <> ''"

echo
echo "--- AD'ye yazıldı mı? (denetim kaydında worker niyeti olmamalı) ---"
psql -d "$DB" -c "SELECT COALESCE((SELECT count(*)::text FROM audit_log WHERE operation_class IS NOT NULL), '0') || ' worker niyet satırı'"
