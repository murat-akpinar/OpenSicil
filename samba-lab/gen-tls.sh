#!/bin/sh
# Lab Samba AD icin LDAPS sertifikasi (ADR-027 lab, docs/05 "LDAPS zorunlu").
# Samba'nin kendi urettigi sertifika yalnizca dc1.opensicil.lab icin gecerli;
# host'tan (testler) localhost:6360 ile baglanmak icin SAN'li sertifika gerekir.
# Cikti: samba-lab/tls/{ca.pem,ca-key.pem,cert.pem,key.pem} — git'e girmez.
# Kullanim: sh samba-lab/gen-tls.sh && docker compose -f compose.yaml -f compose.lab.yaml up -d --force-recreate samba-ad
set -eu
cd "$(dirname "$0")"
mkdir -p tls
cd tls
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout ca-key.pem -out ca.pem -days 3650 -subj "/CN=OpenSicil Lab CA"
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
  -keyout key.pem -out csr.pem -subj "/CN=dc1.opensicil.lab"
printf 'subjectAltName=DNS:dc1.opensicil.lab,DNS:localhost,DNS:samba-ad,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n' > ext.cnf
openssl x509 -req -in csr.pem -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
  -out cert.pem -days 3650 -extfile ext.cnf
rm -f csr.pem ext.cnf ca.srl
# container icindeki samba kullanicisi okuyabilsin
chmod 644 ca.pem cert.pem key.pem ca-key.pem
echo "lab TLS uretildi: $(pwd)"
