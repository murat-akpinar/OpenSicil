# Proje Haritası

Yer imleri. Bir dosya eklendiğinde, taşındığında, silindiğinde veya yeni feature marker açıldığında aynı commit içinde güncellenir. Kısa tutulur.

## Dizinler
- `README.md` → Türkçe giriş noktası: konum, mimari diyagramları, ne-neden-nasıl karar tabloları, yol haritası; mimariyi ya da faz sırasını değiştiren karar burayı da günceller
- `README_ENG.md` → aynı içeriğin İngilizcesi; `README.md` güncellenince birlikte güncellenir
- `docs/00-kavramlar.md` … `docs/08-gereksinimler.md` → tasarım: kavramlar, mevcut çözümler, mimari, rol modeli, yaşam döngüsü, AD, Zimbra, güvenlik, gereksinimler ve açık sorular
- `docs/09-kurulum.md` → kurulum ön koşulları, boyutlandırma, ayarlar; env veya ön koşul ekleyen her kutucuk günceller
- `docs/10-saha-notlari.md` → gerçek kurumda bugün işlerin nasıl yürüdüğü (olgu → tasarımdaki karşılığı → sonuç) henüz bilinmeyenler, son taramanın ve dağıtım gözden geçirmesinin sahne sonuçları ve bilerek yazılmayanlar; sahne yürütmeli gözden geçirmenin girdisi ve kaydı
- `docs/11-dogrulama-notlari.md` → teknik iddiaların birincil kaynakla sınanmış hali (Zimbra, AD, Samba, `ldap3`, Keycloak, dağıtım): sonuç, alıntı, kaynak; yeni iddia önce buraya soru olarak girer
- `docs/decisions/` → karar kayıtları (001–069; 025'in yerine 041, 001'in adlandırma kısmının yerine 063 geçti)
- `compose.yaml` / `compose.override.yaml` → servisler (nginx, backend, worker, migrate, db); prod-benzeri `-f compose.yaml` ile override'sız çalışır
- `.env.example` → gereken tüm ortam değişkeni adları (değer değil)
- `nginx/` → `Dockerfile` (nginxinc/nginx-unprivileged), `nginx.conf` (TLS sonlanması, HTTP→HTTPS yönlendirme, `/healthz` — yönlendirmesiz iç healthcheck, `access_log off` — güvenlik header'ları — ADR-066)
- `backend/` → axum + sqlx; `src/main.rs` (komut yönlendirme: `migrate` / sunucu), `src/server.rs` (HTTP sunucu), `src/health.rs` (`/api/health`), `src/migrate.rs` (rol oluşturma + şema migration), `src/logging.rs` (istek log'u, gerçek istemci IP'si `X-Forwarded-For`'dan), `src/db.rs`; `migrations/` (sqlx migration dosyaları), `.sqlx/` (offline önbellek, şu an boş — macro kullanılmıyor)
- `worker/` → sqlx; `src/main.rs` (nabız döngüsü / `worker-health`), `src/heartbeat.rs` (`/tmp/worker-heartbeat`), `src/db.rs`

## Feature indeksi
Koddaki `--- START FEATURE: <ad> ---` markerlarının karşılığı. Aramak için:
`grep -rn "FEATURE: <ad>" --exclude-dir=.git --exclude-dir=tmp .`

| Feature | Nerede |
|---|---|
| <user-login> | <backend/src/auth/> |

## Ortak yardımcılar
Yeni bir şey yazmadan önce buraya bak. Aynı işi yapan varsa tekrar yazma.

| Ne yapar | Nerede |
|---|---|
| <istek doğrulama şeması> | <backend/src/common/validation> |
