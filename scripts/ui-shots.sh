#!/usr/bin/env sh
# scripts/ui-shots.mjs'i sabit surumlu Playwright'la calistirir (ADR-114 dogrulamasi).
# Playwright modulu ve tarayicisi tmp/araclar altinda durur, git'e ve imaja girmez;
# yoksa bir kez indirilir. Node yalnizca bu dogrulama icin gerekiyor, uygulamada yok.
set -eu
cd "$(dirname "$0")/.."

VERSION=1.63.0
PW=tmp/araclar/pw
export PLAYWRIGHT_BROWSERS_PATH="$PWD/tmp/araclar/playwright"

if [ ! -d "$PW/node_modules/playwright" ]; then
  echo "playwright $VERSION kuruluyor ($PW)"
  npm install --silent --no-fund --no-audit --prefix "$PW" "playwright@$VERSION"
fi
if [ ! -d "$PLAYWRIGHT_BROWSERS_PATH" ]; then
  echo "chromium indiriliyor"
  NODE_PATH="$PWD/$PW/node_modules" node "$PW/node_modules/playwright/cli.js" install chromium
fi

NODE_PATH="$PWD/$PW/node_modules" node scripts/ui-shots.mjs "$@"
