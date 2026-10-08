#!/usr/bin/env bash
# Tek sinyal (CLAUDE.md Komutlar): iki crate'te test (gercek Postgres + lab ortamlari),
# format, lint ve bagimlilik taramasi; cikis kodu 0 yalnizca hepsi gecince.
#
# Lab testleri env yokken ATLANMAZ, panikle duser (docs/09 lab: kapali ortam sessizce
# gecmesin). Bu betik bu yuzden butun lab degiskenlerini tek dosyadan verir:
# LAB_ENV (varsayilan ./.lab-env; git'e girmez) — DATABASE_URL, AD_LAB_*, AD_CA_FILE,
# AD_WIN_*, OIDC_LAB_*. Degerler ekrana basilmaz. Lab'in kendisi (test Postgres,
# Samba + seed.sh, Keycloak, Windows AD) kalici servislerdir; betik yalnizca test
# Postgres container'i durmussa baslatir (ADR-133: Samba ve AD baska makinelerde).
set -uo pipefail
cd "$(dirname "$0")/.."

LAB_ENV=${LAB_ENV:-./.lab-env}
[ -r "$LAB_ENV" ] || { echo "lab değişken dosyası yok: $LAB_ENV" >&2; exit 2; }
set -a
# shellcheck disable=SC1090
. "$LAB_ENV"
set +a

if command -v docker >/dev/null && docker inspect opensicil-test-pg >/dev/null 2>&1; then
  docker start opensicil-test-pg >/dev/null
fi

failed=()
step() { # ad, komut...
  local name=$1; shift
  if "$@" >"$LOG_DIR/$name.log" 2>&1; then
    echo "ok    $name"
  else
    echo "HATA  $name (ayrıntı: $LOG_DIR/$name.log)"
    failed+=("$name")
  fi
}
LOG_DIR=$(mktemp -d)

for d in backend worker; do
  step "$d-test"   sh -c "cd $d && cargo test -- --include-ignored"
  step "$d-fmt"    sh -c "cd $d && cargo fmt --all -- --check"
  step "$d-clippy" sh -c "cd $d && cargo clippy --all-targets -- -D warnings"
  step "$d-audit"  sh -c "cd $d && cargo audit"
done

for d in backend worker; do
  grep -h "test result" "$LOG_DIR/$d-test.log" | sed "s/^/$d: /"
  grep -h "^test .* FAILED$" "$LOG_DIR/$d-test.log" | sed "s/^/$d: /"
done
if [ ${#failed[@]} -eq 0 ]; then
  echo "TEMİZ: iki crate'te test, format, lint ve audit geçti"
  rm -rf "$LOG_DIR"
  exit 0
fi
echo "BAŞARISIZ: ${failed[*]}"
exit 1
