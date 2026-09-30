# Proje Haritası

Yer imleri. Bir dosya eklendiğinde, taşındığında, silindiğinde veya yeni feature marker açıldığında aynı commit içinde güncellenir. Kısa tutulur.

## Dizinler
- `README.md` → Türkçe giriş noktası: konum, mimari diyagramları, ne-neden-nasıl karar tabloları, yol haritası; mimariyi ya da faz sırasını değiştiren karar burayı da günceller
- `README_ENG.md` → aynı içeriğin İngilizcesi; `README.md` güncellenince birlikte güncellenir
- `docs/00-kavramlar.md` … `docs/08-gereksinimler.md` → tasarım: kavramlar, mevcut çözümler, mimari, rol modeli, yaşam döngüsü, AD, Zimbra, güvenlik, gereksinimler ve açık sorular
- `docs/09-kurulum.md` → kurulum ön koşulları, boyutlandırma, ayarlar; env veya ön koşul ekleyen her kutucuk günceller
- `docs/10-saha-notlari.md` → gerçek kurumda bugün işlerin nasıl yürüdüğü (olgu → tasarımdaki karşılığı → sonuç) henüz bilinmeyenler, son taramanın ve dağıtım gözden geçirmesinin sahne sonuçları ve bilerek yazılmayanlar; sahne yürütmeli gözden geçirmenin girdisi ve kaydı
- `docs/11-dogrulama-notlari.md` → teknik iddiaların birincil kaynakla sınanmış hali (Zimbra, AD, Samba, `ldap3`, Keycloak, dağıtım): sonuç, alıntı, kaynak; yeni iddia önce buraya soru olarak girer
- `docs/decisions/` → karar kayıtları (001–077; 025'in yerine 041, 001'in adlandırma kısmının yerine 063, 069'un test komutları kısmının yerine 070, 069'un lab Samba imajı kısmının yerine 074 geçti; 071 imaj taraması "temiz" tanımını verir; 075 ADR-002'nin altına midPoint deneme sonucunu yazar; 077 kapsam/yasaklı grubun tablo olmadığını ve kimlik modelinin rol türü kısıtını yazar)
- `compose.yaml` / `compose.override.yaml` → servisler (nginx, backend, worker, migrate, db); prod-benzeri `-f compose.yaml` ile override'sız çalışır
- `.env.example` → gereken tüm ortam değişkeni adları (değer değil)
- `nginx/` → `Dockerfile` (nginxinc/nginx-unprivileged), `nginx.conf` (TLS sonlanması, HTTP→HTTPS yönlendirme, `/healthz` — yönlendirmesiz iç healthcheck, `access_log off` — güvenlik header'ları — ADR-066)
- `backend/` → axum + sqlx; `src/main.rs` (komut yönlendirme: `migrate` / sunucu), `src/server.rs` (HTTP sunucu), `src/health.rs` (`/api/health`), `src/migrate.rs` (rol oluşturma + şema migration + bootstrap hesabı seed + servis rollerine tablo izinleri `SERVICE_GRANTS` — yeni tablo açan her migration buraya satır ekler, ADR-015), `src/logging.rs` (istek log'u, gerçek istemci IP'si `X-Forwarded-For`'dan), `src/db.rs`, `src/common_settings.rs` (ADR-039 ortak ayarlar: `.env`'den okuma + doğrulama; `worker/src/common_settings.rs` ile birebir aynı, biri değişince diğeri de — `main.rs` testi karşılaştırır), `src/desired_state.rs` (olması gereken durum saf fonksiyonu; worker kopyasıyla birebir aynı), `src/audit.rs` (denetim kaydı: operatör olayı yazma, olay adı sabitleri), `src/web.rs` (HTML route'ları, askama), `src/auth.rs` (argon2id parola hash), `src/crypto.rs` (chacha20poly1305 AEAD, anahtar ayrıştırma), `src/national_id.rs` (ulusal kimlik no: doğrulama, şifreleme, blind index, maske), `src/cookie.rs` (oturum çerezleri), `src/token.rs` (oturum token üret/hashle), `src/session.rs` (bootstrap oturumu), `src/operator_session.rs` (OIDC operatör oturumu, ADR-073), `src/operator_guard.rs` (ayrılmış/askıdaki operatör reddi: `check_operator` kimliği ADR-005 eşleşmesiyle bulur, durumu türetir; `enforce` ara katmanı `server.rs`'te web rotalarını sarar), `src/oidc.rs` (OIDC akışı: discovery, PKCE/state/nonce, `id_token` doğrulama, `groups` claim → yetki eşlemesi, ADR-073), `src/settings.rs` (AD/Zimbra/OIDC ayarları, şifreli + `load_oidc_credentials` çözülmüş sır), `src/bootstrap_account.rs` (yerel admin hesabı), `src/test_support.rs` (yalnızca test: geçici DB, servis rolüyle bağlanma, rol silme); `templates/` (askama HTML şablonları); `migrations/` (sqlx migration dosyaları; `0003_audit_log.sql` denetim tablosu, `performed_by` varsayılanı `current_user`; `0004_identity_model.sql` `departments`, `roles`, `identities`, `identity_additional_roles` — rol türü bileşik FK ile, ADR-077; `0005_catalog.sql` `target_systems` (AD + Zimbra satırı seed; `provision_account_default`, varsayılan konteyner, `retention_days`, `delete_requires_approval`, `password_reset_delay_days`), `catalog_items` (GUID/zimbraId, kayıp işareti, yalnızca worker yazar), `role_entitlements`/`department_entitlements` (grup, liste), `role_target_settings`/`department_target_settings` (tek değerli: `provision_account`, OU/COS, e-posta alan adı, UPN soneki; yalnızca birincil rol ve departman); `0006_audit_events.sql` denetim kaydı alanları — aktör (OIDC sub + ad, yalnızca backend), kimlik/hedef, worker niyet sınıfı `operation_class` (destructive/grant/first_password/attribute, yalnızca worker) + `emergency`, sonuç satırı `intent_id`/`outcome`, `hourly_counter_usage` görünümü (son 1 saat, kimlik başına); `0007_national_id.sql` `identities` kimlik no kolonları — `national_id_enc` (AEAD, sürüm baytlı), `national_id_bidx` (HMAC-SHA256, UNIQUE), `national_id_country`; üçü birlikte dolu/boş; `0008_jobs.sql` `account_links` (hesap bağlantısı: `external_id`, köken, mod, `applied_state`, ilk parola/parola sıfırlama/doğrulanmış kullanılmamış/silme onayı/ad uyuşmazlığı işaretleri; yalnızca worker yazar) ve `jobs` (öncelik 0–3, durum queued/running/succeeded/needs_intervention, deneme, kira `locked_by`/`locked_until`, `retry_requested`; kimlik+hedef başına tek açık iş)), `.sqlx/` (offline önbellek, şu an boş — macro kullanılmıyor)
- `worker/` → sqlx; `src/main.rs` (tek sıralı iş döngüsü: 5 sn yoklama, SIGTERM'de eldeki iş bitirilir; `worker-health`; açılışta ortak ayarları doğrular ve loglar), `src/queue.rs` (iş alma `SKIP LOCKED` + 5 dk kira, erişilemeyen hedefler hariç; niyet satırı + kira uzatma tek transaction, sonuç satırı, tamamla/başarısız + artan geri çekilme, 5 denemede müdahale; bağlantı hatasında `defer_unreachable` deneme tüketmez), `src/model.rs` (bir iş için olması gereken durum girdisini DB'den yükler: zaman çizgisi, departman zinciri, roller, hedef varsayılanları, bağlantı), `src/engine.rs` (`run_job`: yükle → `desired_state`; connector gelene kadar sonucu yazar, kuru modda "uygulanacaktı"), `src/writes.rs` (tek yazma noktası + `TargetWriter` sözleşmesi + `OperationClass`, ADR-054/062), `src/heartbeat.rs` (`/tmp/worker-heartbeat`), `src/db.rs`, `src/common_settings.rs` ve `src/desired_state.rs` (backend'dekilerle birebir aynı kopyalar, ADR-039/038/070), `src/test_support.rs` (yalnızca test: `../backend/migrations` ile geçici DB, docs/03 örnek modeli)
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
| audit-log | `backend/src/audit.rs` (operatör olayı yazma; `web.rs` ayar değişikliği, bootstrap parola değişimi ve operatör girişinde çağırır) |
| national-id | `backend/src/national_id.rs` (doğrulama, AEAD + sürüm baytı, HMAC blind index, maske, DB yaz/ara; kimlik kayıt formu 3a'da bağlar) |
| desired-state | `backend/src/desired_state.rs` = `worker/src/desired_state.rs` (saf: durum türetme, olması gereken durum, etkin yönetici; 3a motoru, 3f önizlemesi, Faz 5 aynı fonksiyonu çağırır) |
| operator-guard | `backend/src/operator_guard.rs` (her istekte ara katman + oturum açılışında `web.rs` `establish_operator_session`; ayrılmış/askıdaki/silinmiş operatör 403, oturum silinir, denetim kaydı) |
| job-queue | `backend/src/jobs.rs` (iş aç: tekilleştirme + öncelik yükseltme, "tekrar dene" bayrağı), `worker/src/queue.rs` (al/kira/niyet/sonuç/tamamla/başarısız) |
| engine | `worker/src/model.rs` (girdi yükleme), `worker/src/engine.rs` (`run_job`) |
| write-point | `worker/src/writes.rs` (`writes::apply`: tek yazma noktası — kuru modda hedefe ve denetime dokunmaz; canlıda niyet → connector `TargetWriter::write` → sonuç; kira kaybında yazmaz) |

## Ortak yardımcılar
Yeni bir şey yazmadan önce buraya bak. Aynı işi yapan varsa tekrar yazma.

| Ne yapar | Nerede |
|---|---|
| Parola hash (argon2id) | `backend/src/auth.rs` |
| AEAD şifreleme/çözme (chacha20poly1305), base64 32 baytlık anahtar ayrıştırma (`parse_key`) | `backend/src/crypto.rs` |
| Ulusal kimlik no: doğrula/normalize (TR kontrol hanesi), şifrele (sürüm baytlı), blind index, maske, kimliğe yaz ve blind index ile bul | `backend/src/national_id.rs` |
| Oturum çerezi kur/oku/temizle (HttpOnly+Secure+SameSite) | `backend/src/cookie.rs` |
| Oturum token üret/hashle (bootstrap ve operatör oturumu ortak) | `backend/src/token.rs` |
| Denetim kaydına operatör olayı yaz (kim/ne/hangi kimlik/detay; sırlar hariç ayar farkı) | `backend/src/audit.rs` |
| Ortak ayarları `.env`'den oku ve doğrula (`CommonSettings::from_env`; sahiplenme, saatlik sınırlar, acil kota, hassas eşleme, saat dilimi) | `backend/src/common_settings.rs` = `worker/src/common_settings.rs` |
| Operatörle eşleşen kimliğin durumunu DB'den türet ve reddet/izin ver (`check_operator`; saat dilimi çevirisi Postgres'te), kurulum saat dilimini Postgres'e karşı doğrula (`db::check_time_zone`) | `backend/src/operator_guard.rs`, `backend/src/db.rs` |
| İş aç (`jobs::enqueue`: açık iş varsa yenisi açılmaz, öncelik yükseltilir) ve "tekrar dene" iste (`request_retry`) | `backend/src/jobs.rs` |
| Kuyruktan iş al / kira uzat + niyet satırı / sonuç satırı / tamamla / başarısız (`queue::claim`, `record_intent`, `record_outcome`, `complete`, `fail`) | `worker/src/queue.rs` |
| Bir kimlik + hedef için olması gereken durum girdisini yükle (`model::load`) ve hesapla (`engine::run_job`) | `worker/src/model.rs`, `worker/src/engine.rs` |
| Kimlik durumu türet (`lifecycle_state`, ADR-038/053), olması gereken durum (`desired_state`: hesap var/aktif, OU/COS, üyelikler, unvan, alan adı, `accountExpires`, parola sıfırlama, iptal), etkin yönetici (`effective_manager`, ADR-041); zaman `Clock { now, today }` girdisi, tz kütüphanesi yok | `backend/src/desired_state.rs` = `worker/src/desired_state.rs` |
| Test: geçici migrate edilmiş DB aç/sil, servis rolüyle bağlan, rol sil, docs/03 örnek kataloğunu ve iki kimliği seed et (`seed_example_catalog`, `seed_two_identities`; yalnızca `cfg(test)`) | `backend/src/test_support.rs` |
