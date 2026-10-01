#!/usr/bin/env sh
# Arayüz CSS'ini yeniden üretir (ADR-088): girdi frontend/assets/app.css,
# çıktı frontend/static/app.css (commit'lenir, include_bytes! ile binary'ye gömülür).
# Şablonlarda yeni bir sınıf kullanan her kutucuk bunu çalıştırıp çıktıyı aynı commit'e koyar.
# Derleyici Tailwind'in standalone CLI binary'si: Node yok, npm yok, sürüm sabit.
set -eu
cd "$(dirname "$0")/.."

VERSION=4.3.3
SHA256=dc61b3ac6b8c9ca874c0cc4c57b2409791a64c5540404ca5f5367360babc313a
BIN="tmp/araclar/tailwindcss-$VERSION"
URL="https://github.com/tailwindlabs/tailwindcss/releases/download/v$VERSION/tailwindcss-linux-x64"

if [ ! -x "$BIN" ]; then
  mkdir -p tmp/araclar
  echo "tailwindcss $VERSION indiriliyor"
  curl -sSfL -o "$BIN.indiriliyor" "$URL"
  echo "$SHA256  $BIN.indiriliyor" | sha256sum -c - || {
    rm -f "$BIN.indiriliyor"
    echo "sha256 uyuşmadı, indirme atıldı" >&2
    exit 1
  }
  mv "$BIN.indiriliyor" "$BIN"
  chmod +x "$BIN"
fi

"$BIN" --input frontend/assets/app.css --output frontend/static/app.css --minify
echo "frontend/static/app.css üretildi ($(wc -c < frontend/static/app.css) bayt)"
