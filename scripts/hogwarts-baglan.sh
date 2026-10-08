#!/usr/bin/env bash
# Test sunucusundaki (ADR-133) bos OpenSicil yiginini GERCEK Windows AD'ye (Hogwarts)
# baglar, Hogwarts modelini yukler, taratir ve hesaplari gozlem modunda sahiplenir.
# Iki yerden calisir: sunucuda (`HOST=` bos, varsayilan) ya da laptoptan
# (`HOST=vaultscan`, veritabani ve compose adimlari ssh ile gider).
#
# Ne yapar:
#   1. Kapsam, DRY_RUN ve sahiplenme modu Ayarlar tablosuna yazilir (ADR-131, ADR-018)
#   2. Yerel admin ile girilir, Yapilandirma sayfasina AD baglantisi ve CA yazilir (ADR-136)
#   3. Worker yeniden baslar (acilista katalog yenilenir), katalog beklenir
#   4. scripts/hogwarts-seed-model.sql yuklenir, migrate slug'lari doldurur
#   5. Mutabakat taramasi kuyruga alinir ve beklenir
#   6. Yonetilmeyen hesaplarin hepsi gozlem modunda sahiplenilir (ADR-102/125)
#
# AD'ye hicbir sey YAZILMAZ: tarama salt okuma, sahiplenme gozlem modunda, DRY_RUN=true.
# Parolalar ekrana ve komut satirina dusmez: ikisi de terminalden sorulur (ya da
# OPENSICIL_ADMIN_PASSWORD / AD_PASSWORD); AD parolasi sunucuda .lab-env'den, laptopta
# tmp/lab-ad-notlari.md'den okunur.
#
# Sunucuda:  cd /app/opensicil && bash scripts/hogwarts-baglan.sh
# Laptopta:  HOST=vaultscan BASE=https://192.168.1.131:8443 CA=tmp/hogwarts-ca.pem bash scripts/hogwarts-baglan.sh
set -euo pipefail
cd "$(dirname "$0")/.."

HOST=${HOST:-}
APP_DIR=${APP_DIR:-/app/opensicil}
BASE=${BASE:-https://localhost:8443}
CA=${CA:-./ad-ca.pem}
AD_HOST=DUMBLEDORE-DC01.hogwarts.local:636
AD_BIND_DN=open.sicil@hogwarts.local
SCOPE_USERS="OU=Hogwarts,DC=hogwarts,DC=local"
SCOPE_GROUPS="OU=Groups,OU=Hogwarts,DC=hogwarts,DC=local"
START_DATE=$(date +%F)

[ -r "$CA" ] || { echo "CA dosyası yok: $CA" >&2; exit 1; }
AD_PASSWORD=${AD_PASSWORD:-$(sed -n 's/^- Parola: //p' tmp/lab-ad-notlari.md 2>/dev/null | head -1 || true)}
# Sunucuda ayni servis hesabinin parolasi testler icin .lab-env'de duruyor.
if [ -z "$AD_PASSWORD" ] && [ -r .lab-env ] && grep -qx "AD_WIN_BIND_DN=$AD_BIND_DN" .lab-env; then
  AD_PASSWORD=$(sed -n 's/^AD_WIN_PASSWORD=//p' .lab-env | head -1)
fi
if [ -z "$AD_PASSWORD" ]; then
  read -rsp "AD servis hesabı ($AD_BIND_DN) parolası: " AD_PASSWORD; echo
fi
ADMIN_PASSWORD=${OPENSICIL_ADMIN_PASSWORD:-}
if [ -z "$ADMIN_PASSWORD" ]; then
  read -rsp "OpenSicil yerel admin parolası: " ADMIN_PASSWORD; echo
fi

# SQL stdin'den gelir; -tA tek degerli sorgularin ciktisini cikplak verir.
on_host() { if [ -n "$HOST" ]; then ssh "$HOST" "cd $APP_DIR && $1"; else sh -c "$1"; fi; }
db() { on_host "docker compose exec -T db sh -c 'psql -U \"\$POSTGRES_USER\" -d \"\$POSTGRES_DB\" -qtA -v ON_ERROR_STOP=1'"; }
compose() { on_host "docker compose $*"; }
wait_job() { # kind -> son isin durumu (succeeded|failed), en fazla 3 dk
  local status=""
  for _ in $(seq 1 90); do
    status=$(echo "SELECT status FROM read_jobs WHERE kind = '$1' ORDER BY id DESC LIMIT 1;" | db)
    case "$status" in succeeded|failed) break ;; esac
    sleep 2
  done
  echo "SELECT kind || ': ' || status || ' — ' || coalesce(result, '') FROM read_jobs WHERE kind = '$1' ORDER BY id DESC LIMIT 1;" | db
  [ "$status" = succeeded ]
}

echo "1) kapsam ve DRY_RUN Ayarlar tablosuna yazılıyor"
db <<SQL
UPDATE operational_settings s SET value = v.value FROM (VALUES ('AD_MANAGED_USER_OUS', '$SCOPE_USERS'),
  ('AD_MANAGED_GROUP_OUS', '$SCOPE_GROUPS'), ('AD_PASSIVE_OU', ''), ('DRY_RUN', 'true'),
  ('OWNERSHIP_MODE_ENABLED', 'true')) v(key, value)
WHERE s.key = v.key;
SQL

echo "2) yerel admin girişi ve Yapılandırma sayfasına AD bağlantısı + CA"
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
umask 077
printf '%s' "$ADMIN_PASSWORD" > "$WORK/admin"
printf '%s' "$AD_PASSWORD" > "$WORK/ad"
curl -sk -D "$WORK/h" -o /dev/null --data-urlencode "username=admin" \
  --data-urlencode "password@$WORK/admin" "$BASE/login"
OP=$(grep -i "^set-cookie: opensicil_operator_session=" "$WORK/h" | head -1 \
  | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r')
[ -n "$OP" ] || { echo "giriş başarısız (parola?)" >&2; exit 1; }

# Sirlar bos gonderilirse backend mevcut degeri korur (settings.rs COALESCE).
CODE=$(curl -sk -o /dev/null -w '%{http_code}' -b "$OP" \
  --data-urlencode "ad_host=$AD_HOST" \
  --data-urlencode "ad_bind_dn=$AD_BIND_DN" \
  --data-urlencode "ad_service_password@$WORK/ad" \
  --data-urlencode "ad_ca_pem@$CA" \
  --data-urlencode "zimbra_url=" --data-urlencode "zimbra_admin_password=" \
  --data-urlencode "oidc_issuer=" --data-urlencode "oidc_client_id=" \
  --data-urlencode "oidc_client_secret=" "$BASE/config")
case "$CODE" in 2*|3*) ;; *) echo "Yapılandırma kaydedilemedi: HTTP $CODE" >&2; exit 1 ;; esac

echo "3) worker yeniden başlıyor, katalog bekleniyor"
compose restart worker >/dev/null
sleep 3
wait_job catalog_refresh

echo "4) Hogwarts modeli yükleniyor"
db < scripts/hogwarts-seed-model.sql
compose run --rm --no-deps migrate >/dev/null

echo "5) mutabakat taraması"
TARGET=$(echo "SELECT id FROM target_systems WHERE kind = 'ad';" | db)
curl -sk -o /dev/null -b "$OP" -X POST "$BASE/targets/$TARGET/reconcile/scan"
wait_job reconcile

echo "6) yönetilmeyen hesaplar gözlem modunda sahipleniliyor"
ROLE=$(echo "SELECT id FROM roles WHERE placeholder;" | db)
FINDINGS=$(echo "SELECT id FROM reconcile_findings WHERE target_system_id = $TARGET AND kind = 'unmanaged';" | db)
ARGS=()
for f in $FINDINGS; do ARGS+=(--data-urlencode "finding=$f"); done
[ ${#ARGS[@]} -gt 0 ] || { echo "sahiplenilecek hesap yok"; exit 0; }
curl -sk -o /dev/null -b "$OP" "${ARGS[@]}" \
  --data-urlencode "primary_role_id=$ROLE" --data-urlencode "employment_type=permanent" \
  --data-urlencode "start_date=$START_DATE" "$BASE/targets/$TARGET/reconcile/adopt"
sleep 10
echo "SELECT kind || ': ' || count(*) FROM reconcile_findings WHERE target_system_id = $TARGET GROUP BY kind;" | db
echo "SELECT 'kimlik: ' || count(*) FROM identities;" | db
echo
echo "Arayüz:  $BASE/targets/$TARGET/reconcile"
