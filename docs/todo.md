# Görevler

Kurallar:
- Her kutucuk tek oturumda ve tek commit'te biter. Büyükse böl.
- Her kutucukta en az bir ölçülebilir kabul kriteri vardır.
- **Her fazın (ve alt fazın, ek hedef sistemin) son kutucuğu güvenlik ve test kapanışıdır.** Atlanamaz, geçmeden sonraki faza geçilmez.
- Kapanış kutucuğundaki `<...>` yer tutucuları, `docs/08-gereksinimler.md` → 🟡 Kurulumda kararlaştırılacak listesindeki test/format/lint/kapsam kararları verilince doldurulur.
- **Ek Hedef Sistem** bölümleri (Zimbra ve ileride Carbonio) "Faz" olarak numaralanmaz: AD (Faz 3) çekirdektir, hedef sistemler onun üstüne eklenir. Ama Faz 4 (İşletme) ve Faz 5 (Mevcut kurum), ilgili hedef sistem bitmeden o sistemi kapsamaz — bkz. her ikisinin başındaki not.

## Faz 1: Altyapı

### 1a. İskelet
- [x] Compose + nginx + backend + worker + PostgreSQL ayağa kalkar
  - Kabul: `docker compose up -d` sonrası tüm servisler `healthy`
  - Kabul: `curl -s -o /dev/null -w "%{http_code}" localhost/api/health` → `200`
  - Kabul: nginx dışında hiçbir serviste `ports:` yok
- [x] Süreç sözleşmesi ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md))
  - Kabul: backend/worker SIGTERM alınca `stop_grace_period` içinde düzgün kapanıyor
  - Kabul: aynı imajın `migrate` alt komutu şema sahibi rolüyle tek seferlik container olarak çalışıyor
  - Kabul: `worker-health` alt komutu ve eski şema sürümünde açılışta çıkma davranışı çalışıyor
- [x] `docs/09-kurulum.md` açılır, bu fazdaki ön koşullar eklenir
  - Kabul: dosya var, bu faza ait `<...>` yer tutucuları dolduruldu
- [x] Faz kapanışı: güvenlik ve test
  - Kabul: testler, format, lint, bağımlılık taraması temiz (her crate ayrı, [ADR-070](decisions/070-bagimsiz-crate-per-crate-komut.md); `cargo audit`'te üst akımda düzeltmesi olmayan ve kullanım şeklimizde geçersiz saldırı önkoşullu bulgular kabul edilen risktir — [ADR-073](decisions/073-cargo-audit-rsa-bulgusu-kabul-edilen-risk.md)) → `for d in backend worker; do (cd "$d" && cargo test && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo audit); done`
  - Kabul: yeni kodda satır kapsamı ≥ %80 (her crate ayrı, gerçek Postgres gerektiren entegrasyon testleri dahil — [ADR-070 Ek](decisions/070-bagimsiz-crate-per-crate-komut.md#ek-entegrasyon-testleri-ve-kapsam-ölçümü)) → tek kullanımlık test Postgres'i açılır, sonra `for d in backend worker; do (cd "$d" && DATABASE_URL="postgres://testuser:testpass@localhost:15432/testdb" cargo llvm-cov --fail-under-lines 80 -- --include-ignored); done`
  - Kabul: imaj taraması temiz — "temiz" tanımı [ADR-071](decisions/071-imaj-taramasi-temiz-tanimi.md) ([ADR-070](decisions/070-bagimsiz-crate-per-crate-komut.md)'la aynı gerçeklik çatışması: Debian taban imajının yama takvimi projenin kontrolünde değil) → `docker run --rm -v /var/run/docker.sock:/var/run/docker.sock aquasec/trivy:latest image --severity CRITICAL,HIGH --scanners vuln <imaj>`; `Status: fixed` bulgu olmamalı
  - Kabul: sır sızıntısı yok → `git log -p <faz başı>..HEAD | grep -nEi '(password|secret|token|api[_-]?key)[[:space:]]*[:=]'` boş
  - Kabul: `.claude/rules/security.md` kontrol listesi gözden geçirildi; bulgular ya düzeltildi ya karar kaydına yazıldı
  - Kabul: `.env.example` güncel, `docs/MAP.md` güncel

### 1b. Giriş
- [x] Yerel bootstrap hesabı ve Yapılandırma sayfası ([ADR-068](decisions/068-yapilandirma-sayfasi-ve-bootstrap-hesabi.md))
  - Kabul: migration ile `admin`/`admin` seed ediliyor; ilk girişte eski parola sorulmadan yeni parola zorunlu
  - Kabul: bu hesapla yalnızca Yapılandırma sayfasına (AD, Zimbra, OIDC bağlantı ayarları) erişilebiliyor, başka hiçbir ekrana değil
  - Kabul: AD servis hesabı parolası, Zimbra admin parolası, OIDC client secret DB'de AEAD ile şifreli saklanıyor, `.env`'de değil
- [x] OIDC girişi ve altı yönetim yetkisi ([ADR-005](decisions/005-yonetim-girisi-oidc.md), [ADR-065](decisions/065-oidc-akisi-backend.md))
  - Kabul: Keycloak lab'ına karşı giriş yapılıyor, yetkiler `groups` claim'inden okunuyor
  - Kabul: oturum PostgreSQL'de saklanıyor
  - Kabul: en az bir `OpenSicil-Admins` girişi doğrulanınca yerel bootstrap giriş formu gizleniyor/pasifleşiyor
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Not: "Ayrılmış/askıdaki operatör reddi" ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md), [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)) buradan Faz 2'nin sonuna taşındı: eşleşme, kimlik tablosu ve durum türetme fonksiyonu (ADR-038) olmadan kurulamıyor. Bu kapanış o kutucuğu beklemez.

### 1c. Lab kod olarak + midPoint denemesi
- [x] `compose.lab.yaml` ile Samba AD ve Keycloak ayağa kalkar ([ADR-027](decisions/027-test-stratejisi-ve-lab.md))
  - Kabul: `docker compose -f compose.yaml -f compose.lab.yaml up -d` sonrası ikisi de `healthy`
- [x] midPoint denemesi ([ADR-002](decisions/002-hazir-urun-yerine-gelistirme.md))
  - Kabul: lab AD'ye karşı en fazla 1 gün denenir; sonuç ADR-002'nin altına yeni bir karar dosyası olarak yazılır
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

### 1d. Zimbra keşfi
- [ ] Lab Zimbra'sı ayağa kalkar (sürüm kurulumda kararlaştırılan)
  - Kabul: JSON Admin API'ye oturum açılıyor
- [ ] `curl` ile `CreateAccount`, `ModifyAccount`, `DeleteAccount`, liste üyeliği denenir
  - Kabul: dördü de başarıyla çalışıyor; OpenSicil admin hesabı yönetilen alan adının dışında duruyor ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md))
- [ ] `docs/08`'deki Faz 1d lab sorularının sekizi cevaplanır
  - Kabul: cevaplar `docs/11-dogrulama-notlari.md`'ye yazıldı; ADR-045/049 ve `docs/06` gerekirse güncellendi
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

## Faz 2: Kayıt ve model

- [x] Veritabanı rolleri ve denetim tablosunun `current_user` kolonu ([ADR-015](decisions/015-veritabani-rolleri.md), [ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md))
- [x] Kimlik, departman ağacı, rol, yönetilen kapsam, yasaklı grup listesi tabloları ([ADR-077](decisions/077-yonetilen-kapsam-ve-yasakli-gruplar-tablo-degil.md): kapsam worker env'i, yasaklı grup kod sabiti; tablo yalnızca kimlik modeli)
- [x] Katalog tabloları (test verisiyle dolu; AD'den gerçek dolum 3a'da)
- [x] Denetim kaydı + worker işlem türleri sabitlenir (yıkıcı, verme, ilk parola, öznitelik — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md))
- [x] Kimlik numarası AEAD + blind index ile şifrelenir ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md))
- [x] Ortak ayarlar iki serviste de `.env`'den okunur ([ADR-039](decisions/039-ortak-ayarlar-env.md))
- [x] Olması gereken durum fonksiyonu saf modül olarak yazılır, tablo testleriyle gelir ([ADR-038](decisions/038-kimlik-durumu-turetilir.md))
  - Kabul: 3a, 3f ve Faz 5 aynı fonksiyonu çağırır; ikinci bir fark hesabı yok
  - Not: bu fazın sonunda ekranda çalışan bir şey yoktur, demo yapılmaz
- [x] Ayrılmış/askıdaki operatör reddi ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md), [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)) — Faz 1b'den taşındı, bağımlılık burada çözülüyor
  - Kabul: oturum açılışında ve her istekte kontrol edilen bir test var; ayrılmış/askıdaki operatörün isteği 403 dönüyor
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

## Faz 3: AD provisioning

### 3a. İlk dilim
- [x] Motor ve kuyruk: tekilleştirme, öncelik, 5 sn yoklama, iş kirası ve niyet satırı ([ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md), [ADR-028](decisions/028-worker-zamanlamasi.md)); worker tek sırada ([ADR-047](decisions/047-worker-tek-sirada.md))
  - Not: zamanlayıcı (sorguya dayalı geçiş yakalama) hesap bağlantısı `applied_state` yazılmaya başlayınca, 3a'nın "hesap aç/etkinleştir/pasifleştir" kutucuğuyla birlikte gelir
- [x] Müdahaledeki iş açık sayılır, bağlantı hatası deneme tüketmez ([ADR-052](decisions/052-uygulanamayan-fark.md))
- [x] Connector yazma çağrıları tek noktadan geçer + kuru çalıştırma modu ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md))
  - Not: ekrandaki kalıcı şerit ve metrik bayrağı backend'in metrik ucuyla (Faz 4) gelir; worker tarafı (yazma kesimi, "uygulanacaktı" sonucu, açılış logu) burada
- [x] AD connector okuma yolu + katalog (OU, grup, SID, yasaklı grup, iç içe üyelik)
  - Not: lab için `sh samba-lab/gen-tls.sh` (SAN'lı LDAPS sertifikası) ve `sh samba-lab/seed.sh` (OU/grup/yasaklı grup seed'i); lab testi `AD_LAB_URL`, `AD_LAB_BIND_DN`, `AD_LAB_PASSWORD`, `AD_CA_FILE` ile çalışır (docs/09 lab)
- [x] Açılış kontrolleri: kapsam DN'leri, `msDS-LogonTimeSyncInterval` ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md))
- [x] Şablonla kullanıcı adı üretimi; yalnızca veritabanı ve AD çakışması kontrol edilir
- [x] Varsayılan eşleme + tek `add` ile hesap aç/etkinleştir/pasifleştir ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md))
  - Not: `manager` eşlemesi (ADR-040/041), üyelik farkı ve OU taşıma 3b/3c'de; zamanlayıcı (türetilen ≠ `applied_state` → iş) uçtan uca kutucuğuyla
- [x] Kimlik kayıt formu + kişi sayfası + operatör dilinde hata + hedefteki fark görünümü (F-12)
  - Not: ADR-078 — form 3a alt kümesi (ipucu Faz 5, elle kullanıcı adı ve ek rol 3b, askı 3c); fark görünümü durum düzeyinde (türetilen ↔ `applied_state`), üyelik/öznitelik farkı 3b/3c; "neden bekliyor" (eşik/sayaç/onay) 3f
- [x] Uçtan uca: kayıt → AD'de pasif hesap → ekranda "açıldı"
  - Kabul: elle bir kayıt girilir, AD'de pasif hesap görünür, ekranda durum "açıldı" olur
  - Not: `sh scripts/e2e-lab.sh` aynı yolu gerçek backend + worker + lab AD/Keycloak ile yürütür (ADR-079; 2026-10-01'de geçti: `uctan.uca`, UAC 514, sayfada "açıldı"/"bekliyor"/"uyumlu"). Zamanlayıcı (ADR-028/038) burada geldi: `worker/src/scheduler.rs`. Betik, worker rolünün `app_settings` okuma yetkisinin eksik olduğunu yakaladı; eklendi
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Not (2026-10-01): backend 100 / worker 60 test (lab dahil), format ve clippy temiz, `cargo audit` yalnızca ADR-073 `rsa`; kapsam backend %92,94 / worker %91,27; imaj taraması Faz 2 tabanıyla aynı (backend 60, worker 58, `Status: fixed` yok); sır sızıntısı taraması boş; TODO yok; 50 satırı aşan yalnızca test fonksiyonları. docs/07 listesinden üç madde işaretlendi (backend hesap bağlantısı yazamaz, `bekliyor`+bitiş geçmiş tek tik tek iş, ADR-060); özel karakterli ad (virgül/yıldız) ile lab'da hesap açma ve elle pasifleştirilen hesabın korunması lab testine girmedi, 3b/3c kapanışında

### 3b. Roller ve adlar
- [x] Rol ve departman ekranları, katalogdan seçim, grup ve OU yönetimi
  - Not: ADR-080 — kayıt doğrudan modele yazılır ve etkilenen kimlikler için öncelik 2 iş açılır (taslak/eşik 3f'te `org::enqueue_affected`'ı sahneleme yoluna taşır); rol/departman silme yok; `/targets` hedef sistem varsayılanlarını da kapsar
- [x] Elle kullanıcı adı; bağlı olmayan hesapla ve kullanılmış adla çakışmada müdahale; serbest bırakma ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md), [ADR-035](decisions/035-kullanilmis-ad-duz-metin-serbest-birakma.md), [ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md))
  - Not: ADR-081 — istek ve karar `identities.requested_username`/`name_conflict_override` (0010, backend yazar), adı yine worker üretir; kişi sayfasında müdahale bloğu (farklı ad / sıradaki ad / `/used-names` serbest bırakma); sahiplenme (ipucu) seçeneği Faz 5
- [x] Eşleme izinli listesi + "sadece boşsa yaz" ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md), [ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md))
  - Not: ADR-082 — `attribute_mappings` tablosu + `/targets/{id}/mappings` ekranı + ikiz `mapping_rules.rs`; worker her işte doğrular (ihlal müdahale), mevcut hesapta öznitelik farkını tek modify ile yazar; `manager` eşlemesi ADR-040/041 ile burada geldi (yönetici hedefte yoksa dokunulmaz); Zimbra satırları Zimbra connector'ıyla çalışır
- [x] Belirsiz bileşen kuralı: "hesap açılsın=hayır" mevcut hesabı silmez ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md))
  - Not: etki önizlemesi burada yoktur; model farkı 3f'te tek fonksiyonla yazılır
  - Not: `desired_state` zaten ayarı yalnızca hesap yokken okuyordu; `provision_not_expected` bayrağı eklendi (iş sonucunda "rol hesap öngörmüyor" bilgisi, 3d mutabakat aynı bayrağı bulgu yapar). Kayıp hesap ve yönetici belirsizliği lab/birim testinde; "bağlantıyı kopar ve yeniden aç" (Sistem yöneticisi) 3c silme/saklama kutucuğuyla
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Not (2026-10-01): backend 109 / worker 68 test (lab Keycloak + Samba dahil), format ve clippy temiz, `cargo audit` yalnızca ADR-073 `rsa`; kapsam backend %92,71 / worker %91,22; imaj taraması Faz 3a tabanıyla aynı (backend 60, worker 58, `Status: fixed` yok); faz commit'lerinde sır sızıntısı taraması boş; TODO yok; 50 satırı aşan yalnızca test fonksiyonları. docs/07'den ADR-022, ADR-029 ve ADR-040 maddeleri işaretlendi; ADR-034 "boş değeri dolduruyor" ve ADR-035 "serbest bırakılan ad yeniden üretiliyor" lab'da sınanmadı (3c/3e kapanışında)

### 3c. Yaşam döngüsü
- [x] İşe giriş, görev değişikliği (önce ekleme, sonra çıkarma — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md))
  - Not: ADR-083 — worker bağlı hesapta katalog grubu farkını uygular (ekleme → OU taşıma → çıkarma), `/identities/{id}/edit` ve ek rol ekle/kaldır; saatlik sayaç "bütün iş bekler" kuralı 3f'te
- [x] Planlı ve acil ayrılış, geri alma (yıkıcı — [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)), hedefte doğrulanan kayıt iptali ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)), tarihli askı ([ADR-053](decisions/053-tarihli-aski.md))
  - Not: ADR-084 — kişi sayfasında yaşam döngüsü formları; worker iptali hedefte doğrular (`verified_unused`), doğrulanmış iptal ve saklama sonu aynı silme yolu (hesap silinir, bağlantı işaretlenir, son hesapta kimlik `silindi` + kişisel veri temizliği + ad yakma; iptalde yakılmaz). Zimbra onaylı silme Zimbra bölümünde; yıkıcı sayaç/eşik 3f
- [x] `ayrıldı`dan her çıkış geri alma sayılır, askı bitişi iznin son günüdür, `accountExpires` temizlenir ([ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md))
  - Not: ileri tarihli bitiş `ayrıldı`dan çıkarıyorsa `identity.departure_reverted` olarak denetlenir; form iznin son gününü alır ve dönüş gününü gösterir; `accountExpires` her işte bitişe göre yazılır, bitiş yoksa `0` (lab testinde doğrulandı)
- [x] Yönetici ayrılışında astların etkin yöneticisi türetilir (F-38, [ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md))
  - Not: türetme ve `manager` eşlemesi ADR-082 kutucuğuyla gelmişti; burada yöneticinin `ayrıldı`ya giriş/çıkış işi astlara iş açıyor (`enqueue_subordinates`), kişi sayfası "(ayrıldı → devir: X)" notunu, ast sayısını ve yöneticisiz kalan astlar uyarısını gösteriyor
- [x] Süreli ek rol, tarih dolunca kendiliğinden kalkar (F-37, [ADR-020](decisions/020-sureli-ek-rol.md))
  - Not: zamanlayıcı tiki bitişi geçmiş atamayı siler, `identity.role_expired` ("süresi doldu") yazar ve her hedefe iş açar; gruplar o işte üyelik farkıyla düşer (ADR-050 yıkıcı sınıf)
- [x] Ayrılışta gecikmeli parola sıfırlama ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md))
  - Not: zamanlayıcı pencere dolunca (hedefin `password_reset_delay_days`; acilde hemen) iş açar, worker parolayı rastgeleleştirip `pwdLastSet = 0` yazar ve bağlantıda `password_reset_at_departure` işaretler (bir kez; öznitelik sınıfı, yıkıcı sayaca girmez); doğrulanmış iptalde yapılmaz
- [x] Yaklaşan bitişler listesi (F-36)
  - Not: `/upcoming?days=N` (varsayılan 30, en çok 365; ayrı kurulum ayarı açılmadı — ekran başına değer yeter): bitişler, ek rol bitişleri, askı başlangıçları ve dönüş günleri tek sorguda, kurulum saat diliminde
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Not (2026-10-01): backend 111 / worker 73 test (lab dahil), format ve clippy temiz, `cargo audit` yalnızca ADR-073 `rsa`; kapsam backend %91,75 / worker %91,79; imaj taraması Faz 3b tabanıyla aynı (backend 60, worker 58, `Status: fixed` yok); sır sızıntısı taraması boş; TODO yok; `writes::detail_json` 55 satıra çıkmıştı, bölündü; kapanışta saklama süresi sonu silme işinin zamanlayıcıda açılmadığı görüldü ve `open_retention_jobs` eklendi (ADR-024/028). docs/07'den ADR-028, ADR-041 ve ADR-048 maddeleri işaretlendi; ADR-032 (elle pasifleştirilmiş hesabın korunması), ADR-053 askı geçişleri ve özel karakterli ad lab'da sınanmadı (3d/3e kapanışında)

### 3d. İlk parola
- [x] AEAD ile şifreli teslim ([ADR-036](decisions/036-ilk-parola-aead.md)), yardım masası yetkisi, ilk girişte değiştirme ayarı (F-11, [ADR-019](decisions/019-ilk-parola-teslimi.md))
  - Not (2026-10-01): `first_passwords` tablosu (0012) + bağlantıda `first_password_pwd_last_set`; operatör ister → tek kimlik işi → worker iş sonunda parolayı yazar ve sürüm baytlı AEAD ile bırakır → durum sayfası bir kez gösterir ve boşaltır (Postgres 18 `RETURNING old`); 10 dk gösterilmeyen ya da cevapsız istek zamanlayıcıda kapanır. `helpdesk` yalnızca ilk parola ister, "tekrar dene" hr/admin'e daraltıldı. `FIRST_LOGIN_CHANGE_REQUIRED` (worker, boş = açık). Kullanılmamış hesap kuralı (ADR-046) bu kutuda uygulandı; lab testi kuru red / açık mod `pwdLastSet 0` / kapalı mod damga + gerçek bind / kullanılmış hesap reddi (ADR-085)
- [x] Kullanılmamış hesap kontrolü ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md))
  - Not (2026-10-01): kural `worker/src/first_password.rs::account_unused` — (`lastLogonTimestamp` boş **veya** bağlantıda "ayrılışta sıfırlandı") **ve** (`pwdLastSet = 0` **veya** worker'ın son yazdığı damgaya eşit). Verilince işaret temizlenir, damga yazılır. Birim testi altı durumu, DB testi işaret/damga yazımını, lab testi gerçek reddi sınar; docs/07 ADR-046 maddesi işaretlendi. Samba `lastLogonTimestamp`'ı simple bind'da yazıyor mu lab'da ölçülmedi (3d kapanışında)
- [x] "Kaydet ve ilk parolayı ver" tek adım, teslim ekranı, okunabilir parola biçimi ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md))
  - Kabul: N-13 ölçümü ≤ 60 sn (kayıttan parolanın ekranda görünmesine)
  - Not: ürünün hedef sahnesi ilk kez burada uçtan uca gösterilir
  - Not (2026-10-01): kayıt formunda ikinci düğme (`issue_first_password`); `identity::create` kimlik + her hedefe iş + ilk parola isteğini tek transaction'da yazar (işler artık her kayıtta transaction içinde), başlangıç bugün/geçmiş değilse form hatası (`starts_by_today`, kurum saati). Worker hesabı açar açmaz aynı işte parolayı verir (`provision` → `issue_first_password`). Bekleme ekranı hedef işlerinin ilerlemesini gösterir; teslim ekranı kullanıcı adı + e-posta + parola. `scripts/e2e-lab.sh` adım 10: **N-13 ölçümü 4 sn** (kayıt → parola ekranda), parola ikinci açılışta yok, AD'de etkin hesap (UAC 512). Betik yarıda kalan çalışmanın lab hesaplarını başta siler
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Not (2026-10-01): backend 113 / worker 79 test (lab dahil), format ve clippy temiz, `cargo audit` yalnızca ADR-073 `rsa` (backend); kapsam satır backend %93,57 / worker %94,28 (bölge %91,86 / %92,48); imaj taraması backend 61 / worker 59 — bir yeni `affected` bulgu (Debian yaması yok, ADR-071), `Status: fixed` yok; sır sızıntısı taraması ve TODO taraması boş; 50 satırı aşan yeni fonksiyonlar bölündü (`show`/`person_header`, `create`/`open_jobs`, `create`/`finish_create`, `first_value`), kalan tek aşım önceki fazdan `engine::sources` (52). docs/07'den ADR-019 (iki mod), yardım masası yetkisi, ADR-036 (gösterim/10 dk silme), ADR-046 ve ADR-054 (kuru modda ilk parola reddi) maddeleri işaretlendi; ADR-056 maddesi "giriş yapılmış kayıt iptal edilemiyor" kısmı lab'da sınanmadığı için açık kaldı. Samba simple bind'da `lastLogonTimestamp` yazıyor (lab testiyle doğrulandı, docs/05). Worker test altyapısındaki `DATABASE_URL` yarışı (`run()` testi env'i değiştiriyordu) `OnceLock` ile giderildi

### 3e. Tekil sahiplenme ve gözlem modu
- [x] Formdaki mevcut hesap ipucuyla gözlem modunda bağlama ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md))
  - Not (2026-10-01): kayıt formundaki AD ipucu (`existing_ad_account_hint`, yalnızca `OWNERSHIP_MODE_ENABLED=true` iken görünür) istektir; worker ipuçlu kimlik için hesap açmaz, `engine::adopt` kurallardan geçirir — her ret müdahale (ayar kapalı, kuru mod, ipucu yok, kapsam dışı, başka kimliğe bağlı, `adminCount`, iç içe yasaklı grup, sicil uyuşmazlığı) — kabulde `adopted`/`observed` bağlantı, sAM/UPN/mail kimliğe (boşsa), ad uyuşmazlığı `name_mismatch` uyarısı (ADR-042), `ad.account.adopted` denetimi. Gözlem bağlantısında motor hiçbir şey uygulamaz. Lab testi `mevcut.personel` ile dokuz durumu sınar (ADR-086). Zimbra ipucu formda yok (Zimbra ertelendi)
- [x] Fark görünümü ve tek kimlik için yönetime alma ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md))
  - Kabul: motor gerçek bir lab hesabına karşı sınandı
  - Not: CSV ve toplu yönetime alma Faz 5'te (Mevcut kurum)
  - Not (2026-10-01): ADR-087 — fark motorun kuru yolundan hesaplanır (ayrı fark fonksiyonu yok, ADR-038), iş sonucuna yazılır ve kişi sayfasındaki hesap satırında görünür; ilk fark sahiplenme işinin kendi içinde (zamanlayıcı gözlem bağlantısına iş açmaz, yoksa operatör farkı hiç görmezdi); yönetime alma isteği `account_links.manage_requested_at` (0013, backend'in bu tabloda yazabildiği tek kolon), modu worker işin başında çevirir ve farkı aynı işte uygular (`ad.account.managed`); kuru çalıştırmada istek bekler; yetki hr/admin; ayrılış (planlı/acil) gözlemdeki bağlantıya isteği de yazar (ADR-018, "ayrılış kaydedildi ama hiçbir şey olmadı" olmasın). Lab testi gerçek Samba hesabında: sahiplen → fark (hedefe yazma yok) → yönetime al → `OU=SistemUzmanlari`, `GG-VPN`, `applied_state active`. Eşik/sayaç freni 3f'te
- [x] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Not (2026-10-01): backend 115 / worker 83 test (lab Keycloak + Samba dahil), format ve clippy temiz, `cargo audit` yalnızca ADR-073 `rsa`; kapsam satır backend %93,76 / worker %94,92; imaj taraması backend 61 / worker 59 — 3d tabanıyla birebir aynı, `Status: fixed` yok (hepsi `affected`/`fix_deferred`/`will_not_fix`); sır sızıntısı ve TODO taraması boş; `scripts/e2e-lab.sh` geçti (N-13 = 4 sn). security.md gözden geçirmesinde `worker/src/main.rs::run` 51 satıra çıkmıştı, açılış adımları `startup`'a alındı. Önceki kapanışlardan devredilen altı lab doğrulaması burada kapandı: ADR-032 (elle `userAccountControl 514` yapılan hesap yeniden etkinleştirilmiyor), ADR-053 (askıda hesap pasif, üyelik ve OU korunuyor, askı kalkınca etkin), özel karakterli ad (`Öz*el Te,st"(x)` → DN kaçışlı, `ozel.testx`), ADR-034 (boşsa yaz boş özniteliği dolduruyor), ADR-035 (serbest bırakılan ad sonek almadan yeniden veriliyor), ADR-056 (gerçek bind'dan sonra kayıt iptali reddediliyor, hesap silinmiyor). docs/07'den dört madde işaretlendi (özel karakterli ad, ADR-032, ADR-035, ADR-053), ADR-056 maddesine lab notu düştü — "mailbox başarısızken de" kısmı Zimbra bölümüne kaldı

### 3f. Fren ve onay
- [ ] Saatlik sayaçlar ve acil kota worker'da (yıkıcı, verme, ilk parola; iş sayaçlara karşı bütün — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md))
- [ ] Onay anında yeniden önizleme ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md))
- [ ] Model farkı fonksiyonundan hem etki önizlemesi hem değişiklik seti eşiği (ekleme dahil, gözlemdekiler hariç — [ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md), [ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md))
- [ ] Taslak, ikinci yönetici onayı, zaman kilidi backend'de ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md), [ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md))
- [ ] Bekleme sebebi (eşik/sayaç/onay), kalan süre ve onaylayacak grup ekranda gösterilir (F-12)
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

## Arayüz kabuğu

> Fazlar arası iş, "Faz" olarak numaralanmaz: AD fazının kutucuklarıyla bağımlılığı yok. 3e kapanışından sonra, 3f'ten önce yapıldı (kullanıcı isteği, 2026-10-01) — 3f ve Faz 4 ekranları bu kabuğun bileşenleriyle yazılsın, eski ekranlar ikinci kez elden geçmesin. [ADR-064](decisions/064-frontend-htmx-tailwind.md), [ADR-067](decisions/067-arayuz-dili-tema-font.md) ve [ADR-088](decisions/088-arayuz-kabugu-derlenmis-css-tema-font.md).

- [x] Derlenmiş CSS, Catppuccin token'ları, self-host font, kabuk ([ADR-088](decisions/088-arayuz-kabugu-derlenmis-css-tema-font.md))
  - Kabul: `grep -rn "cdn.tailwindcss.com\|style=\"" backend/templates` boş — şablonlarda ne CDN script'i ne satır içi renk kalır
  - Kabul: `GET /static/app.css` 200 ve `text/css`; sayfa hiçbir dış adrese istek atmaz (nginx CSP'si `script-src 'self'` ile uyumlu, `style-src`'ten `'unsafe-inline'` kalkar)
  - Kabul: `sh scripts/build-css.sh` sabit sürümlü Tailwind standalone CLI'yi sha256 doğrulayarak indirir ve `backend/static/app.css`'i yeniden üretir; çıktı commit'li
  - Kabul: açık (Latte) ve koyu (Mocha) tema sistem tercihine göre açılır, elle geçiş tercihi `localStorage`'da kalır; arayüz fontu CaskaydiaMono (Regular + Bold, `backend/static/`)
  - Kabul: 18 şablonun hepsi aynı kabuğu (üst bar + gezinme + içerik) ve ortak bileşenleri (`.btn`, `.card`, `.tbl`, `.badge`, `.alert`, `.field`) kullanır; mevcut testler geçer
  - Not (2026-10-01): `backend/assets/app.css` (token + bileşen) → `scripts/build-css.sh` (Tailwind 4.3.3 standalone, sha256) → `backend/static/app.css`; `src/assets.rs` CSS/JS/iki fontu `include_bytes!` ile gömer ve `/static/{dosya}`dan bir yıl `immutable` sunar, listede olmayan ad 404. `base.html` uygulama kabuğu (üst bar + gezinme + tema düğmesi), `base_auth.html` giriş/parola kartı; `config.html` gezinmeyi kapatır (ADR-068). Durum rengi `identity::state_kind`/`status_kind` ile rozet. nginx CSP'sinden `style-src 'unsafe-inline'` kalktı. İki bulgu uygulamada yakalandı ve ADR-088'e yazıldı: Tailwind'in `/20` saydamlık değiştiricisi `var()` token'ında düşüyor, `color-mix` eski tarayıcıda düz renge dönüyor — yumuşak tonlar tema başına token oldu. htmx eklenmedi (tek ihtiyaç bekleme ekranı, `meta refresh` yetiyor); fontun Italic/BoldItalic'i alınmadı. Backend 117 test geçti
- [x] TR/EN dil seçimi: gömülü `tr.toml`/`en.toml` + `t()` ([ADR-067](decisions/067-arayuz-dili-tema-font.md) madde 1, [ADR-088](decisions/088-arayuz-kabugu-derlenmis-css-tema-font.md) madde 6)
  - Kabul: dil seçicisiyle EN'e geçince bütün ekran metinleri İngilizce; tercih operatör oturumunda saklanır
  - Kabul: iki dosyanın anahtar kümesi birebir aynı ve şablonlarda kullanılan her anahtar var — test bunu doğrular
  - Not (2026-10-01): ADR-089 — `backend/i18n/tr.toml` + `en.toml` (`include_str!`, ~30 satırlık ayrıştırıcı, yeni crate yok), şablonlarda `lang.t("anahtar")`, veritabanı anahtarları `lang.key("state", "active")`, yer tutucu `t1`/`tn`. Doğrulama ve veri katmanı artık metin değil i18n anahtarı döner (`identity::validate`, `national_id::parse`, `mapping_web::validate`, `org::SaveError::Invalid`); durum/iş/köken/çalışma tipi/rol türü alanları veritabanı anahtarını taşır, `upcoming` sorgusu olay adı yerine tür anahtarı döndürür. Tercih `operator_sessions.lang` (0014); üst bardaki tek düğme `POST /lang` ile yazar ve `Referer`'ın yalnızca yerel yoluna döner (açık yönlendirme reddi, testli). Oturumsuz ekranlarda (giriş, parola, Yapılandırma) dil `Accept-Language`'dan gelir, seçici gösterilmez. Üç test: anahtar kümeleri aynı + tarama (en az 200 anahtar) + grup üyeleri; uçtan uca: seçici → `lang = 'en'`, ekran İngilizce, `<html lang="en">`. Worker'ın ürettiği iş sonucu/hata metinleri Türkçe kalır (öneri: ayrı kutucuk)
- [ ] Kapanış: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

## Ek Hedef Sistem: Zimbra

> AD (Faz 3) çekirdektir; bu bölüm onun üstüne eklenir, "Faz" olarak numaralanmaz. Ama Faz 4 (İşletme) ve Faz 5 (Mevcut kurum) bu bölüm bitmeden Zimbra'yı kapsamaz — mutabakat raporu ve sahiplenme yalnızca AD üstünde çalışır.

- [ ] Kayıtlı yanıt stub'ı ile connector testleri ([ADR-027](decisions/027-test-stratejisi-ve-lab.md))
- [ ] Gerçek Zimbra connector'ı: parolasız hesap açma ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md))
- [ ] COS ve dağıtım listesi kataloğu, liste üyeliği yönetimi
- [ ] Yaşam döngüsü karşılıkları: hesap aç/girişe kapat/sil
- [ ] Ayrılışta kullanıcının kendi yönlendirme/filtresi temizlenir, gecikmeli otomatik yanıt, devir yöneticisine yönlendirme ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md), [ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md))
- [ ] Ad üretimine Zimbra çakışma kontrolü eklenir; erişilemezken atlanır ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md))
- [ ] Kapanış: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

<!-- İleride: Ek Hedef Sistem: Carbonio CE (F-41, v1.x adayı) — lab doğrulaması sonrası buraya aynı yapıda eklenir. -->

## Faz 4: İşletme

> Zimbra bölümü bitmemişse mutabakat raporu ve metrik ucu yalnızca AD'yi kapsar; eksik kapsam ekranda/raporda belirtilir.

- [ ] Okuma şeridi: mutabakat, katalog yenileme, toplu yönetime alma fark hesabı ayrı görevde, sayaca dokunmaz ([ADR-051](decisions/051-okuma-seridi.md))
- [ ] Mutabakat raporu: gece ve istendiğinde, "yeniden uygula" ile (F-13)
- [ ] Gösterge paneli ([ADR-076](decisions/076-gosterge-paneli-v1-kapsami.md)): tarih aralığı filtresi (varsayılan son 30 gün); işe giren/ayrılan/görev değiştiren sayısı; departman ve rol kırılımı (pasta, native CSS `conic-gradient`, kütüphane yok); "ayrılmış ama kapatılamamış" ve onay bekleyen taslak sayısı
  - Kabul: beş panel de doğru sayıları gösteriyor; tarih aralığı değiştirilince sayılar güncelleniyor; yeni JS/CSS grafik bağımlılığı eklenmedi
- [ ] Hedef sistem başına saklama + "silinmeyi bekleyenler" listesi ([ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md))
- [ ] Metrik ucu: hedef sistem başına son başarılı bağlantı dahil (F-19)
- [ ] N-03 yük testi (20.000 kimlik) ve ölçüme göre worker eşzamanlılığı
- [ ] `docs/09` eşlemesinin tek node'lu bir Kubernetes kümesinde (k3s/kind) doğrulanması (N-14)
  - Kabul: backend iki kopya, worker tek kopya ayağa kalkıyor; migration Job'ı bitmeden servisler hazır olmuyor; worker pod'u silinince yenisi kuyruğu sürdürüyor
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

## Faz 5: Mevcut kurum

> Sahiplenme ve mutabakat Zimbra hesaplarını da kapsaması için Zimbra bölümünün bitmiş olması gerekir.

- [ ] CSV ile toplu kimlik içe aktarma (kolon ve ipucu kuralları — [ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md), [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md))
- [ ] Sicil no değişimi önerisi, tarihli ek role dokunmama kuralı ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md))
- [ ] Toplu sahiplenme ve toplu yönetime alma ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md))
  - Not: tekil sahiplenme 3e'de zaten var
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)
  - Kabul ek: "v1 hazır" — ilk beş faz + Zimbra bölümü sıfırdan kurulan ve mevcut personeli olan kurum için tek başına kullanılabilir
