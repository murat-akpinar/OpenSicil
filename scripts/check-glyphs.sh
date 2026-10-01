#!/usr/bin/env sh
# Arayuzde kullanilan her ikonun gomulu fontta gercekten bulundugunu dogrular.
# Eksik glyph ekranda tofu kutusu (□) demek, bu yuzden ikon eklemek bu betigi
# calistirmayi gerektirir (ADR-067: ikon seti CaskaydiaMono Nerd Font'tan gelir).
#
# Kaynak: backend/assets/app.css icindeki `content: "\fXXX"` kod noktalari.
# Olcum: fc-query'nin bildirdigi charset araliklari (fontconfig, ek bagimlilik yok).
set -eu
cd "$(dirname "$0")/.."

FONT=backend/static/CaskaydiaMonoNerdFont-Regular.ttf
CSS=backend/assets/app.css

[ -r "$FONT" ] || { echo "font yok: $FONT" >&2; exit 1; }

fc-query --format='%{charset}' "$FONT" | tr ' ' '\n' > /tmp/opensicil-charset.$$
trap 'rm -f /tmp/opensicil-charset.$$' EXIT

grep -o 'content: "\\[0-9a-f]\{3,5\}"' "$CSS" \
  | sed 's/.*\\\(.*\)"/\1/' | sort -u \
  | awk -v ranges=/tmp/opensicil-charset.$$ '
      BEGIN {
        n = 0
        while ((getline line < ranges) > 0) {
          if (line == "") continue
          split(line, p, "-")
          lo[n] = strtonum("0x" p[1])
          hi[n] = (2 in p) ? strtonum("0x" p[2]) : lo[n]
          n++
        }
      }
      {
        cp = strtonum("0x" $0)
        found = 0
        for (i = 0; i < n; i++) if (cp >= lo[i] && cp <= hi[i]) { found = 1; break }
        printf "U+%s %s\n", toupper($0), found ? "var" : "YOK"
        if (!found) bad++
      }
      END {
        if (bad) { printf "%d glyph fontta yok\n", bad > "/dev/stderr"; exit 1 }
        print "tum ikonlar fontta var"
      }'
