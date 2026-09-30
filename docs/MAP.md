# Proje Haritası

Yer imleri. Bir dosya eklendiğinde, taşındığında, silindiğinde veya yeni feature marker açıldığında aynı commit içinde güncellenir. Kısa tutulur.

## Dizinler
- `README.md` → Türkçe giriş noktası: konum, mimari diyagramları, ne-neden-nasıl karar tabloları, yol haritası; mimariyi ya da faz sırasını değiştiren karar burayı da günceller
- `README_ENG.md` → aynı içeriğin İngilizcesi; `README.md` güncellenince birlikte güncellenir
- `docs/00-kavramlar.md` … `docs/08-gereksinimler.md` → tasarım: kavramlar, mevcut çözümler, mimari, rol modeli, yaşam döngüsü, AD, Zimbra, güvenlik, gereksinimler ve açık sorular
- `docs/09-kurulum.md` → kurulum ön koşulları, boyutlandırma, ayarlar; env veya ön koşul ekleyen her kutucuk günceller
- `docs/10-saha-notlari.md` → gerçek kurumda bugün işlerin nasıl yürüdüğü (olgu → tasarımdaki karşılığı → sonuç) henüz bilinmeyenler, son taramanın ve dağıtım gözden geçirmesinin sahne sonuçları ve bilerek yazılmayanlar; sahne yürütmeli gözden geçirmenin girdisi ve kaydı
- `docs/11-dogrulama-notlari.md` → teknik iddiaların birincil kaynakla sınanmış hali (Zimbra, AD, Samba, `ldap3`, Keycloak, dağıtım): sonuç, alıntı, kaynak; yeni iddia önce buraya soru olarak girer
- `docs/decisions/` → karar kayıtları (001–074; 025'in yerine 041, 001'in adlandırma kısmının yerine 063, 069'un test komutları kısmının yerine 070, 069'un lab Samba imajı kısmının yerine 074 geçti; 071 imaj taraması "temiz" tanımını verir)
- `compose.yaml` / `compose.override.yaml` → servisler (nginx, backend, worker, migrate, db); prod-benzeri `-f compose.yaml` ile override'sız çalışır
- `.env.example` → gereken tüm ortam değişkeni adları (değer değil)
- `nginx/` → `Dockerfile` (nginxinc/nginx-unprivileged), `nginx.conf` (TLS sonlanması, HTTP→HTTPS yönlendirme, `/healthz` — yönlendirmesiz iç healthcheck, `access_log off` — güvenlik header'ları — ADR-066)
- `backend/` → axum + sqlx; `src/main.rs` (komut yönlendirme: `migrate` / sunucu), `src/server.rs` (HTTP sunucu), `src/health.rs` (`/api/health`), `src/migrate.rs` (rol oluşturma + şema migration + bootstrap hesabı seed), `src/logging.rs` (istek log'u, gerçek istemci IP'si `X-Forwarded-For`'dan), `src/db.rs`, `src/web.rs` (HTML route'ları, askama), `src/auth.rs` (argon2id parola hash), `src/crypto.rs` (chacha20poly1305 AEAD), `src/cookie.rs` (oturum çerezleri), `src/token.rs` (oturum token üret/hashle), `src/session.rs` (bootstrap oturumu), `src/operator_session.rs` (OIDC operatör oturumu, ADR-073), `src/oidc.rs` (OIDC akışı: discovery, PKCE/state/nonce, `id_token` doğrulama, `groups` claim → yetki eşlemesi, ADR-073), `src/settings.rs` (AD/Zimbra/OIDC ayarları, şifreli + `load_oidc_credentials` çözülmüş sır), `src/bootstrap_account.rs` (yerel admin hesabı), `src/test_support.rs` (yalnızca test: gecici DB); `templates/` (askama HTML şablonları); `migrations/` (sqlx migration dosyaları), `.sqlx/` (offline önbellek, şu an boş — macro kullanılmıyor)
- `worker/` → sqlx; `src/main.rs` (nabız döngüsü / `worker-health`), `src/heartbeat.rs` (`/tmp/worker-heartbeat`), `src/db.rs`
- `compose.lab.yaml` → Faz 1c lab: Keycloak (host portu 8081) + Samba AD (ADR-027, ADR-074)
- `keycloak-lab/realm-opensicil.json` → lab realm içe aktarma: istemci `opensicil-backend`, `groups` protokol mapper'ı, altı yönetim grubu, test kullanıcıları (`test-admin`, `test-hr`, `test-none`)
- `samba-lab/config.json` → lab Samba AD DC provizyon ayarları (sambacc JSON şeması): realm `OPENSICIL.LAB`, NetBIOS `OPENSICIL`, DC adı `DC1`; OU/grup/kullanıcı seed verisi yok (Faz 3a'da eklenir)

## Feature indeksi
Koddaki `--- START FEATURE: <ad> ---` markerlarının karşılığı. Aramak için:
`grep -rn "FEATURE: <ad>" --exclude-dir=.git --exclude-dir=tmp .`

| Feature | Nerede |
|---|---|
| bootstrap-admin | `backend/src/web.rs` |
| oidc-login | `backend/src/oidc.rs`, `backend/src/operator_session.rs`, `backend/src/web.rs` |

## Ortak yardımcılar
Yeni bir şey yazmadan önce buraya bak. Aynı işi yapan varsa tekrar yazma.

| Ne yapar | Nerede |
|---|---|
| Parola hash (argon2id) | `backend/src/auth.rs` |
| AEAD şifreleme/çözme (chacha20poly1305) | `backend/src/crypto.rs` |
| Oturum çerezi kur/oku/temizle (HttpOnly+Secure+SameSite) | `backend/src/cookie.rs` |
| Oturum token üret/hashle (bootstrap ve operatör oturumu ortak) | `backend/src/token.rs` |
