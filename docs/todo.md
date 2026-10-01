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
- [ ] Eşleme izinli listesi + "sadece boşsa yaz" ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md), [ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md))
- [ ] Belirsiz bileşen kuralı: "hesap açılsın=hayır" mevcut hesabı silmez ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md))
  - Not: etki önizlemesi burada yoktur; model farkı 3f'te tek fonksiyonla yazılır
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

### 3c. Yaşam döngüsü
- [ ] İşe giriş, görev değişikliği (önce ekleme, sonra çıkarma — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md))
- [ ] Planlı ve acil ayrılış, geri alma (yıkıcı — [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)), hedefte doğrulanan kayıt iptali ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)), tarihli askı ([ADR-053](decisions/053-tarihli-aski.md))
- [ ] `ayrıldı`dan her çıkış geri alma sayılır, askı bitişi iznin son günüdür, `accountExpires` temizlenir ([ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md))
- [ ] Yönetici ayrılışında astların etkin yöneticisi türetilir (F-38, [ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md))
- [ ] Süreli ek rol, tarih dolunca kendiliğinden kalkar (F-37, [ADR-020](decisions/020-sureli-ek-rol.md))
- [ ] Ayrılışta gecikmeli parola sıfırlama ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md))
- [ ] Yaklaşan bitişler listesi (F-36)
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

### 3d. İlk parola
- [ ] AEAD ile şifreli teslim ([ADR-036](decisions/036-ilk-parola-aead.md)), yardım masası yetkisi, ilk girişte değiştirme ayarı (F-11, [ADR-019](decisions/019-ilk-parola-teslimi.md))
- [ ] Kullanılmamış hesap kontrolü ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md))
- [ ] "Kaydet ve ilk parolayı ver" tek adım, teslim ekranı, okunabilir parola biçimi ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md))
  - Kabul: N-13 ölçümü ≤ 60 sn (kayıttan parolanın ekranda görünmesine)
  - Not: ürünün hedef sahnesi ilk kez burada uçtan uca gösterilir
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

### 3e. Tekil sahiplenme ve gözlem modu
- [ ] Formdaki mevcut hesap ipucuyla gözlem modunda bağlama ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md))
- [ ] Fark görünümü ve tek kimlik için yönetime alma ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md))
  - Kabul: motor gerçek bir lab hesabına karşı sınandı
  - Not: CSV ve toplu yönetime alma Faz 5'te (Mevcut kurum)
- [ ] Faz kapanışı: güvenlik ve test
  - (1a'daki kapanış şablonunun aynısı)

### 3f. Fren ve onay
- [ ] Saatlik sayaçlar ve acil kota worker'da (yıkıcı, verme, ilk parola; iş sayaçlara karşı bütün — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md))
- [ ] Onay anında yeniden önizleme ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md))
- [ ] Model farkı fonksiyonundan hem etki önizlemesi hem değişiklik seti eşiği (ekleme dahil, gözlemdekiler hariç — [ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md), [ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md))
- [ ] Taslak, ikinci yönetici onayı, zaman kilidi backend'de ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md), [ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md))
- [ ] Bekleme sebebi (eşik/sayaç/onay), kalan süre ve onaylayacak grup ekranda gösterilir (F-12)
- [ ] Faz kapanışı: güvenlik ve test
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
