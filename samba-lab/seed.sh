#!/bin/sh
# Lab Samba AD seed verisi (ADR-027; docs/05 kapsam ve yasakli gruplar):
# yonetilen kullanici OU'su + pasif OU + grup OU'su, katalog gruplari, uc yasakli
# grup ornegi (yerlesik SID, adminCount, Domain Admins'in ic ice uyesi) ve
# OpenSicil yonetim grubu. Tekrar calistirmak guvenlidir ("already exists" yutulur).
# Kullanim: docker compose -f compose.yaml -f compose.lab.yaml up -d samba-ad && sh samba-lab/seed.sh
set -u
# compose exec proje .env'ini ister; container adiyla dogrudan exec .env'siz calisir
CONTAINER="${SAMBA_CONTAINER:-opensicil-samba-ad-1}"
run() { docker exec "$CONTAINER" "$@" 2>&1 | grep -v "already exists" || true; }
BASE="DC=opensicil,DC=lab"

run samba-tool ou add "OU=Personel,$BASE"
run samba-tool ou add "OU=Pasif,OU=Personel,$BASE"
run samba-tool ou add "OU=Gruplar,$BASE"
run samba-tool ou add "OU=Disarida,$BASE"

for g in GG-Internet GG-VPN GG-BT-Paylasim GG-Sistem-Uzmanlari GG-Nobet GG-Nested-Admin GG-AdminCount OpenSicil-Admins; do
  run samba-tool group add "$g" --groupou="OU=Gruplar"
done
# kapsam disi bir grup: katalog almamali
run samba-tool group add GG-Disarida --groupou="OU=Disarida"
# Domain Admins'in ic ice uyesi: yasakli (docs/05)
run samba-tool group addmembers "Domain Admins" GG-Nested-Admin
# adminCount dolu grup: yasakli; Samba SDProp calistirmaz, elle yazilir (docs/05 Samba AD)
run sh -c "printf 'dn: CN=GG-AdminCount,OU=Gruplar,$BASE\nchangetype: modify\nreplace: adminCount\nadminCount: 1\n' | ldbmodify -H /var/lib/samba/private/sam.ldb"
# mevcut personel (sahiplenme testleri, 3e)
run samba-tool user create mevcut.personel 'Lab-only-Pass1' --userou="OU=Personel" --given-name=Mevcut --surname=Personel
# AD bind giris kapisi (ADR-095): yetkisi ic ice uyelikten gelen bir operator.
# Kapsam disi OU'da durur — operator (BT personeli) yonetilen personel OU'sunda
# olmak zorunda degil, ve kapsamdaki hesap sayisini bozmaz (mutabakat testleri).
run samba-tool group add GG-Lab-Operators --groupou="OU=Disarida"
run samba-tool group addmembers OpenSicil-Admins GG-Lab-Operators
run samba-tool user create lab.operator 'Lab-only-Pass1' --userou="OU=Disarida" --given-name=Lab --surname=Operator
run samba-tool group addmembers GG-Lab-Operators lab.operator
echo "seed tamam"
