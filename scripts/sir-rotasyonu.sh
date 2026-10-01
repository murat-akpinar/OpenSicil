#!/usr/bin/env bash
# .env'deki sirlari dondurur (ADR-104). Sizan ya da suphelenilen her deger icin
# calistirilir; veritabani SILINMEZ, sema ve veri oldugu gibi kalir.
#
# Donen degerler: uc Postgres parolasi, AEAD_MASTER_KEY, BLIND_INDEX_KEY, METRICS_TOKEN.
# Uretilen degerler yalnizca .env'e ve veritabanina yazilir; ekrana basilmaz,
# baska bir dosyaya kopyalanmaz.
#
# ONEMLI: AEAD anahtari degisince DB'de sifreli duran sirlar cozulemez hale gelir.
# Betik bittikten sonra Yapilandirma sayfasindan AD servis hesabi parolasi ve
# OIDC client secret YENIDEN GIRILIR (docs/09-kurulum.md "Sir sizarsa"). O ana
# kadar OIDC girisi calismaz; kapi yerel break-glass `admin` hesabidir.
set -euo pipefail
cd "$(dirname "$0")/.."

KEYS="POSTGRES_OWNER_PASSWORD POSTGRES_BACKEND_PASSWORD POSTGRES_WORKER_PASSWORD
      AEAD_MASTER_KEY BLIND_INDEX_KEY METRICS_TOKEN"

if [ "${1:-}" != "--onayla" ]; then
  cat <<'SON'
Kullanim: sh scripts/sir-rotasyonu.sh --onayla

Oncesinde dogrulayin:
  * Yerel `admin` hesabiyla giris yapabiliyorsunuz (rotasyondan sonra OIDC,
    client secret yeniden girilene kadar calismaz).
  * AD servis hesabi parolasi elinizde (Yapilandirma sayfasina yeniden girilecek).
SON
  exit 1
fi

[ -w .env ] || { echo ".env yok ya da yazilamiyor" >&2; exit 1; }
for k in $KEYS; do
  grep -q "^$k=" .env || { echo ".env'de $k satiri yok" >&2; exit 1; }
done

# AEAD/blind index anahtari degisince DB'deki sifreli kimlik numaralari cozulemez ve
# blind index'leri eslesmez olur; v1'de yeniden sifreleme araci yok (ADR-010 surum
# baytini birakti ama donusturucuyu yazmadi). Dolu veriyle calismaya izin verilmez.
NID=$(docker compose exec -T db sh -c \
  'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -tAc "SELECT count(*) FROM identities WHERE national_id_enc IS NOT NULL"' \
  | tr -d '\r')
if [ "${NID:-1}" != "0" ]; then
  echo "DURDU: $NID kimlikte sifreli kimlik numarasi var; AEAD anahtarini dondurmek" >&2
  echo "       bu degerleri okunamaz hale getirir. Once yeniden sifreleme gerekir." >&2
  exit 1
fi

rnd_alnum() { tr -dc 'A-Za-z0-9' </dev/urandom | head -c 40; }  # DSN'e girer: yalnizca alfanumerik

OWNER_PW=$(rnd_alnum)
BACKEND_PW=$(rnd_alnum)
WORKER_PW=$(rnd_alnum)

echo "1) .env guncelleniyor"
umask 077
sed -e "s|^POSTGRES_OWNER_PASSWORD=.*|POSTGRES_OWNER_PASSWORD=$OWNER_PW|" \
    -e "s|^POSTGRES_BACKEND_PASSWORD=.*|POSTGRES_BACKEND_PASSWORD=$BACKEND_PW|" \
    -e "s|^POSTGRES_WORKER_PASSWORD=.*|POSTGRES_WORKER_PASSWORD=$WORKER_PW|" \
    -e "s|^AEAD_MASTER_KEY=.*|AEAD_MASTER_KEY=$(openssl rand -base64 32)|" \
    -e "s|^BLIND_INDEX_KEY=.*|BLIND_INDEX_KEY=$(openssl rand -base64 32)|" \
    -e "s|^METRICS_TOKEN=.*|METRICS_TOKEN=$(openssl rand -base64 32 | tr -d '/+=')|" \
    .env > .env.rotating
mv .env.rotating .env

# Sahip rolunun parolasini POSTGRES_PASSWORD degil ALTER ROLE dondurur: o degisken
# yalnizca ilk initdb'de okunur, mevcut veritabaninda etkisizdir.
echo "2) sema sahibi rolunun parolasi donduruluyor"
docker compose exec -T -e NEWPW="$OWNER_PW" db \
  sh -c 'psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -v ON_ERROR_STOP=1 -v pw="$NEWPW" -f -' \
  >/dev/null <<'SQL'
ALTER ROLE CURRENT_USER PASSWORD :'pw';
SQL

echo "3) migrate: backend ve worker rollerinin parolasi donuyor"
docker compose run --rm --no-deps migrate

echo "4) backend ve worker yeni anahtarlarla yeniden olusturuluyor"
docker compose up -d backend worker

cat <<'SON'

Rotasyon tamam. Kalan iki adim elle yapilir:
  1. Yerel `admin` ile girin -> Ayarlar (Yapilandirma) sayfasi.
  2. AD servis hesabi parolasini ve OIDC client secret'i yeniden girin, kaydedin.
     (Bos birakilan sir alani ESKI sifreli degeri korur; o deger artik cozulemez,
     yani bu iki alan dolu gonderilmelidir.)
Sonra hedef sistemin mutabakat ekranindan "Yeniden tara" ile baglantiyi dogrulayin.
SON
