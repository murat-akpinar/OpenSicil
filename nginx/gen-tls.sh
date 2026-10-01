#!/bin/sh
# Yerel gelistirme icin nginx'in TLS sertifikasi (ADR-066: TLS nginx'te sonlanir).
# Uretimde kurumun sertifikasi kullanilir, bu betik yalnizca lab/gelistirme icindir.
# Cikti: nginx/tls/{cert.pem,key.pem} — git'e girmez (.gitignore).
# Tarayici CN'e bakmaz, SAN ister: localhost'un yaninda makinenin LAN adresi de
# yazilmali, yoksa https://<ip> "sertifika gecersiz" verir.
# Kullanim: sh nginx/gen-tls.sh [ek-ad-ya-da-ip ...]
#   ornek: sh nginx/gen-tls.sh 192.168.1.112 opensicil.local
set -eu
cd "$(dirname "$0")"
mkdir -p tls

SAN="DNS:localhost,IP:127.0.0.1,IP:::1"
for extra in "$@"; do
  case "$extra" in
    *[!0-9.]*) SAN="$SAN,DNS:$extra" ;;
    *) SAN="$SAN,IP:$extra" ;;
  esac
done

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout tls/key.pem -out tls/cert.pem -days 365 -subj "/CN=OpenSicil Dev" \
  -addext "subjectAltName=$SAN" -addext "extendedKeyUsage=serverAuth"
# nginx-unprivileged container icinde uid 101 ile calisir; bind mount host
# sahipligini tasidigi icin anahtar 0600 olursa okuyamaz (samba-lab/gen-tls.sh
# ayni sebeple 644 veriyor). Gelistirme anahtari, git'e girmiyor.
chmod 644 tls/key.pem tls/cert.pem
echo "nginx TLS uretildi ($SAN)"
