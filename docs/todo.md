# Görevler

Kurallar:
- Her kutucuk tek oturumda ve tek commit'te biter. Büyükse böl.
- Her kutucukta en az bir ölçülebilir kabul kriteri vardır.
- **Her fazın son kutucuğu güvenlik ve test kapanışıdır.** Atlanamaz, geçmeden sonraki faza geçilmez.

## Faz 1: Altyapı

- [ ] Tüm servisler nginx arkasında ayağa kalkar
  - Kabul: `docker compose up -d` sonrası tüm servisler `healthy`
  - Kabul: `curl -s -o /dev/null -w "%{http_code}" localhost/api/health` → `200`
  - Kabul: nginx dışında hiçbir serviste `ports:` yok

- [ ] Faz kapanışı: güvenlik ve test
  - Kabul: testler geçiyor → `<test komutu>`
  - Kabul: yeni kodda satır kapsamı ≥ %80 → `<kapsam komutu>`
  - Kabul: format ve lint temiz → `<lint komutu>`
  - Kabul: bağımlılık taraması temiz → `<npm audit --audit-level=high | pip-audit | govulncheck>`
  - Kabul: imaj taraması temiz → `docker scout cves --only-severity critical,high <imaj>` veya `trivy image --severity CRITICAL,HIGH <imaj>`
  - Kabul: sır sızıntısı yok → `git log -p <faz başı>..HEAD | grep -nEi '(password|secret|token|api[_-]?key)[[:space:]]*[:=]'` boş
  - Kabul: `.claude/rules/security.md` kontrol listesi gözden geçirildi; bulgular ya düzeltildi ya karar kaydına yazıldı
  - Kabul: `.env.example` güncel, `docs/MAP.md` güncel

## Faz 2: <ad>

- [ ] <iş>
  - Kabul: <ölçülebilir kriter>

- [ ] Faz kapanışı: güvenlik ve test
  - <Faz 1'deki kapanış kutucuğunun aynısı>
