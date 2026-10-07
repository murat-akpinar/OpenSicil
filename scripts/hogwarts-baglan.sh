#!/usr/bin/env bash
# Calisan compose yiginini lab Samba'dan GERCEK Windows AD'ye (Hogwarts) cevirir
# ve mutabakat taramasini baslatir. Sonunda 29 Hogwarts hesabi web arayuzundeki
# /targets/<ad>/reconcile ekraninda gorunur.
#
# Ne yapar:
#   1. .env'de CA yolunu gunceller (once .env.yedek alir); kapsam ve DRY_RUN
#      Ayarlar tablosuna yazilir (ADR-131)
#   2. worker'i yeniden baslatir
#   3. Yapilandirma sayfasina AD baglantisini yazar (OIDC ayarlari korunur)
#   4. Mutabakat taramasini kuyruga koyar ve sonucu basar
#
# AD'ye hicbir sey YAZILMAZ: mutabakat salt okumadir ve DRY_RUN=true yapilir.
# Geri almak icin: .env.yedek dosyasini .env uzerine kopyala, worker'i yeniden baslat;
# kapsami Ayarlar ekranindan geri gir.
set -euo pipefail
cd "$(dirname "$0")/.."

CA=./tmp/hogwarts-ca.pem
AD_HOST=DUMBLEDORE-DC01.hogwarts.local:636
AD_BIND_DN=open.sicil@hogwarts.local
SCOPE_USERS="OU=Hogwarts,DC=hogwarts,DC=local"
SCOPE_GROUPS="OU=Groups,OU=Hogwarts,DC=hogwarts,DC=local"
BASE=https://192.168.1.112
KC_USER=test-admin
KC_PASS=test-admin-pw

[ -r "$CA" ] || { echo "CA dosyası yok: $CA" >&2; exit 1; }
AD_PASSWORD=$(sed -n 's/^- Parola: //p' tmp/lab-ad-notlari.md | head -1)
[ -n "$AD_PASSWORD" ] || { echo "parola tmp/lab-ad-notlari.md'de yok" >&2; exit 1; }

echo "1) .env güncelleniyor (yedek: .env.yedek)"
cp -n .env .env.yedek 2>/dev/null || cp .env .env.yedek
set_key() { # anahtar varsa degistirir, yoksa ekler
  if grep -q "^$1=" .env; then
    sed -i "s|^$1=.*|$1=$2|" .env
  else
    printf '%s=%s\n' "$1" "$2" >> .env
  fi
}
set_key AD_CA_PATH "$CA"
docker compose exec -T db sh -lc 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -q' <<SQL
UPDATE operational_settings s SET value = v.value FROM (VALUES ('AD_MANAGED_USER_OUS', '$SCOPE_USERS'),
  ('AD_MANAGED_GROUP_OUS', '$SCOPE_GROUPS'), ('AD_PASSIVE_OU', ''), ('DRY_RUN', 'true')) v(key, value)
WHERE s.key = v.key;
SQL

echo "2) worker yeniden başlatılıyor"
docker compose up -d worker >/dev/null

echo "3) Yapılandırma sayfasına AD bağlantısı yazılıyor"
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
cookie_of() { grep -i "^set-cookie: $1=" "$2" | head -1 | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r'; }
AUTH_URL=$(curl -sk -o /dev/null -w '%{redirect_url}' "$BASE/oidc/login")
curl -s -c "$WORK/kc" "$AUTH_URL" -o "$WORK/kc.html"
ACTION=$(grep -o 'action="[^"]*"' "$WORK/kc.html" | head -1 | sed 's/action="//; s/"$//; s/&amp;/\&/g')
CALLBACK=$(curl -s -o /dev/null -w '%{redirect_url}' -b "$WORK/kc" -c "$WORK/kc" \
  -d "username=$KC_USER&password=$KC_PASS" "$ACTION")
[ -n "$CALLBACK" ] || { echo "OIDC girişi başarısız" >&2; exit 1; }
curl -sk -D "$WORK/h" -o /dev/null "$CALLBACK"
OP=$(cookie_of opensicil_operator_session "$WORK/h")
[ -n "$OP" ] || { echo "oturum çerezi alınamadı" >&2; exit 1; }

# Sirlar bos gonderilir: backend COALESCE ile mevcut degeri korur (settings.rs).
# OIDC issuer/client_id bos gonderilemez, mevcut degerleriyle yeniden yazilir.
curl -sk -o /dev/null -b "$OP" \
  --data-urlencode "ad_host=$AD_HOST" \
  --data-urlencode "ad_bind_dn=$AD_BIND_DN" \
  --data-urlencode "ad_service_password=$AD_PASSWORD" \
  --data-urlencode "zimbra_url=" --data-urlencode "zimbra_admin_password=" \
  --data-urlencode "oidc_issuer=http://192.168.1.112:8081/realms/opensicil" \
  --data-urlencode "oidc_client_id=opensicil-backend" \
  --data-urlencode "oidc_client_secret=" "$BASE/config"

echo "4) katalog ve mutabakat taraması kuyruğa alınıyor"
docker compose restart worker >/dev/null   # açılışta katalog yenilemeyi tetikler
TARGET=$(docker compose exec -T db sh -lc 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT id FROM target_systems WHERE kind='"'"'ad'"'"'"' | tr -d '\r')
curl -sk -o /dev/null -b "$OP" -X POST "$BASE/targets/$TARGET/reconcile/scan"

for _ in $(seq 1 60); do
  STATUS=$(docker compose exec -T db sh -lc 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT status FROM read_jobs WHERE kind='"'"'reconcile'"'"' ORDER BY id DESC LIMIT 1"' | tr -d '\r')
  case "$STATUS" in succeeded|failed) break ;; esac
  sleep 2
done

echo
docker compose exec -T db sh -lc 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT kind, status, result FROM read_jobs ORDER BY id DESC LIMIT 2"'
docker compose exec -T db sh -lc 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT kind, count(*) FROM reconcile_findings GROUP BY kind"'
echo
echo "Arayüz:  $BASE/targets/$TARGET/reconcile"
