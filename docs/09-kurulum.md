# 09 — Kurulum

> Yaşayan dosyadır. Ortam değişkeni, ön koşul veya ayar ekleyen her kutucuk aynı commit'te bu dosyayı günceller. `<lab'da doldurulur>` yer tutucuları ilgili fazda çalıştırılarak yazılır. Değişken adları `.env.example`'dadır; burada anlam ve öneri vardır.

## Ön koşullar

### Active Directory
| Ön koşul | Not |
|---|---|
| LDAPS (636) ve worker'ın güvendiği CA sertifikası | Düz LDAP desteklenmez (N-10). CA sertifikası PEM dosyası olarak verilir: `.env`'de `AD_CA_PATH` (host yolu), compose bunu **worker'a ve backend'e** `/etc/opensicil/ad-ca.pem` olarak bağlar (`AD_CA_FILE`); doğrulama kapatılamaz. Backend'in de ihtiyacı var çünkü operatör girişi AD'ye bind ediyor ([ADR-095](decisions/095-giris-kendi-ekranimiz-ad-bind-asil.md), [ADR-105](decisions/105-ad-bind-kapisi-uygulamasi.md)): CA dosyası yoksa AD giriş kapısı çalışmaz, ekranda "Active Directory'ye ulaşılamıyor" yazar ve yalnızca yerel `admin` hesabı girebilir. DC adresi Yapılandırma sayfasındaki "Host" alanına yazılır; `;` ile ayrılmış sıralı liste olabilir (`dc1;dc2:636`), şema yazılmazsa `ldaps://…:636` varsayılır. **Adres IP değil FQDN olmalı:** DC sertifikasının SAN'ında yalnızca DC'nin FQDN'i vardır, `ldaps://<ip>` doğrulamadan geçmez; container'ın bu adı çözebildiğinden emin olun (`dns:` ya da `extra_hosts:`). **Kök CA'yı DC vermez:** el sıkışmada yalnızca kendi sertifikasını yollar, `AD_CA_PATH`'e konan PEM kök CA'dır — `certutil -ca.cert ca.cer` ile ya da DC sertifikasının AIA uzantısındaki LDAP kaydından (`cACertificate`) alınır ([docs/11](11-dogrulama-notlari.md) W7) |
| OpenSicil servis hesabı | Hiçbir ayrıcalıklı grupta değil; etkileşimli giriş GPO ile kapalı; "hassas, devredilemez"; parolası süresiz ya da yazılı rotasyon adımı (`.env` güncelle, worker'ı yeniden başlat). Süresi dolan parola ya da CA sertifikası worker'ı sessizce durdurur: metrik ucundaki "hedef sistem başına son başarılı bağlantı" değerine alarm kurun ([docs/05](05-active-directory.md#servis-hesabı-yetkileri)) |
| Delegasyon adımları | `<lab'da doldurulur — Faz 1c>` |
| Yönetilen kullanıcı OU'ları, isteğe bağlı pasif OU, yönetilen grup OU'ları | `CN=Users`, `CN=Builtin`, `OU=Domain Controllers` kapsam olamaz. **Gruplarınız `CN=Users` içindeyse önce bir OU'ya taşıyın; yalnızca OpenSicil'in yöneteceği grupları taşıyın.** DnsAdmins, Group Policy Creator Owners, Cert Publishers ve benzeri yerleşik gruplar yerinde kalsın: DnsAdmins'in sabit SID'i yoktur, yönetilen OU'ya taşınırsa worker onu tanıyamaz ([docs/05](05-active-directory.md#yasaklı-gruplar)). DC listesi sıralıdır: ilk adres tercihli DC (PDC emulator önerilir), diğerleri yalnızca yedek ([docs/05](05-active-directory.md#bağlantı)). Kapsam Yapılandırma sayfasından değil worker'ın `.env`'inden verilir: `AD_MANAGED_USER_OUS`, `AD_PASSIVE_OU`, `AD_MANAGED_GROUP_OUS` (DN listeleri `;` ile ayrılır) ve `ZIMBRA_MANAGED_DOMAINS` ([ADR-014](decisions/014-yonetim-kapsami-ve-toplu-degisiklik-freni.md), [ADR-077](decisions/077-yonetilen-kapsam-ve-yasakli-gruplar-tablo-degil.md)) |
| Yönetilen grup OU'sunda yalnızca OpenSicil'in tek kaynak olacağı gruplar | Katalog bu OU'daki **her** grubu alır; seçici OU'nun kendisidir. Katalogdaki bir grupta rolde olmayan üyelik sonraki işte kaldırılır ([docs/03](03-rol-ve-veri-modeli.md#motor-neye-dokunur)). Yardım masasının ticket'la verdiği paylaşım ve uygulama grupları ayrı bir OU'da kalsın; aksi halde ilk ay "erişimim gitti" biletleri gelir. Bir grubu role bağlamak istediğinizde OU'ya taşıyın |
| `lastLogonTimestamp` kapatılmamış | İlk parola ve kayıt iptali "hiç giriş yapılmamış hesap"ı bu öznitelikten tanır. Kontrol: `Get-ADObject (Get-ADDomain).DistinguishedName -Properties msDS-LogonTimeSyncInterval` boş ya da 1 ve üstü dönmeli; `0` özniteliği kapatır ve worker'ın AD connector'ı **başlamaz** ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)). Öznitelik acil replike olmaz: "ilk girişte değiştir" işaretini kapatan çok site'lı kurum tercihli DC olarak PDC emulator'ı verir ve şube personelini önceden kaydeder |
| AD Recycle Bin | Önerilir; silinen hesap geri alınabilir |
| Break-glass yönetici | En az bir `OpenSicil-Admins` üyesi yönetilen kapsam **dışında** bir hesap olmalı; yanlış bir ayrılış o operatörün girişini üç kapıda da keser. Son çare yerel `admin` hesabıdır ([ADR-095](decisions/095-giris-kendi-ekranimiz-ad-bind-asil.md) madde 3): kalıcıdır, parolası ilk girişte değiştirilir ve güvenli yerde saklanır; AD'de `admin` adlı bir operatör AD kapısından giremez, ad yerel hesaba ayrılmıştır ([docs/07](07-guvenlik-ve-kvkk.md#yönetici-hesapları)) |
| Kişi olmayan hesaplar yönetilen OU dışında | Servis hesapları, ortak posta kutuları ve test hesapları kimlik değildir. Yönetilen OU'da kalırsa mutabakatta "yönetilmeyen hesap" olarak görünür; Sistem yöneticisi gerekçeyle bilinen istisna işaretler |
| Hibritte pasif OU Entra Connect kapsamı içinde | Kapsam dışına taşınan hesap bulutta silinir ([docs/05](05-active-directory.md#hibrit-entra-connect)) |
| Confidential öznitelik | Kimlik numarası AD'ye yazılacaksa `employeeNumber` kullanılır ve confidential işaretlenir; `employeeID` işaretlenemez. İşaretlenmemişse kimlik numarasını domain'deki **herkes okur ve hiçbir şey hata vermez**; eşlemeyi açmadan önce kontrol edin: `Get-ADObject "CN=Employee-Number,$((Get-ADRootDSE).schemaNamingContext)" -Properties searchFlags` değerinde 128 (0x80) biti olmalı ([docs/05](05-active-directory.md#hassas-öznitelikler)). Adımlar: `<lab'da doldurulur>` ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md)) |

### Zimbra

> Zimbra v1 kapsamında değildir ([ADR-090](decisions/090-zimbra-v1-sonrasina-alindi.md)): bu ön koşullar Zimbra bölümü yapıldığında, v1.x'te geçerli olur. v1 kurulumunda atlanır.

| Ön koşul | Not |
|---|---|
| Admin portu 7071 yalnızca worker'ın IP'sine açık | İnternete asla |
| `zimbraAuthMech = ad` ya da `ldap` | Yerel kısım her zaman kullanıcı adına eşitse mevcut bind DN şablonu yeter, iki mekanizma da olur. Şablon kısaltma üretebiliyorsa mekanizma **`ldap` olmalıdır**: arama filtresi (`zimbraAuthLdapSearchFilter=(mail=%n)` ve AD'de salt okuma bir arama hesabı) `ad` mekanizmasında **yok sayılır** ([docs/06](06-zimbra.md#ön-koşullar-kurumun-işi)) |
| `zimbraAuthFallbackToLocal` **`TRUE` değil**; COS'ta webmail'den parola değiştirme kapalı | Geri düşme varsayılan olarak kapalıdır; yalnızca birisi açmışsa sorun vardır: `zmprov gd <alanadı> zimbraAuthFallbackToLocal` boş ya da `FALSE` dönmeli. Açıksa webmail'den değiştirilen parola (Zimbra'nın yerel parolasına yazılır) AD hesabı kapatıldıktan sonra da postaya girer. `zimbraFeatureChangePasswordEnabled = FALSE` Zimbra yönetici kılavuzunun AD doğrulaması için zaten şart koştuğu ayardır; `zimbraChangePasswordURL` yalnızca istemcide bağlantıdır, sunucu zorlamaz ([docs/06](06-zimbra.md#ön-koşullar-kurumun-işi)) |
| Kullanıcıların dışarıya yönlendirmesi (öneri) | COS'ta `zimbraFeatureMailForwardingEnabled = FALSE` ve `zimbraFeatureMailForwardingInFiltersEnabled = FALSE` kullanıcının postasını dış adrese yönlendirmesini baştan engeller. OpenSicil ayrılışta yine temizler ([ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)); bu ayar çalışırken de sızıntıyı keser |
| Auto-provisioning kapalı | Aksi halde aynı kişi için iki hesap açılır |
| OpenSicil'e özel admin hesabı, **yönetilen alan adının dışında** | v1'de global admin; domain admin desteği lab sonucuna bağlı. Zimbra 9.0.0 P40, 10.0.8 ve sonrasında admin hesapları da geri düşme ayarına uyar: AD doğrulamalı alan adındaki admin yerel parolasıyla **giremez** ve worker "kimlik doğrulama hatası" alır. Hesabı yerel doğrulamalı ayrı bir alan adında açın ya da yönetilen alan adına `zimbraAuthMechAdmin = zimbra` verin ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)) |

#### Bugün hesapları elle açan kurum

Tipik başlangıç durumu şudur: BT, AD'de hesabı açar, sonra Zimbra admin panelinden **aynı adla** mailbox'ı elle açar. Burada iki ayrı şey vardır ve karıştırılmamalıdır:

- **Hesap açma** hiçbir kurulumda kendiliğinden entegre değildir; OpenSicil'in yaptığı iş budur. Mevcut hesaplar CSV ve sahiplenmeyle bağlanır; adlar aynı olduğu için ipucu kolonu `kullanıcıadı` ve `kullanıcıadı@alanadı`dır ([docs/04](04-yasam-dongusu.md#mevcut-personel-içe-aktarma-ve-sahiplenme)).
- **Parola doğrulama** entegre olabilir de olmayabilir de. OpenSicil entegre olmasını ister ([ADR-009](decisions/009-parola-yonetimi.md)). Kontrol: `zmprov gd <alanadı> zimbraAuthMech` → `ad` ya da `ldap` ise entegre, `zimbra` ise Zimbra kendi parolasını tutuyor. Komutsuz kontrol: Windows parolasını değiştiren kişinin webmail parolası da değişiyor mu? Pratik işaret: BT mailbox'ı açarken **parola belirlemiyorsa** ve kişi yine de girebiliyorsa doğrulama zaten AD'dedir; bu kurum ön koşulu karşılıyordur ve aşağıdaki geçişe ihtiyacı yoktur, yalnızca `zimbraAuthFallbackToLocal` değerine bakar.

`zimbra` çıkarsa geçiş alan adı düzeyinde birkaç ayardır ve OSE'de vardır (admin panelde Alan Adları → Kimlik Doğrulama sihirbazı ya da aşağıdaki komutlar). Adlar aynı olduğu için bind şablonu yeter:

```
zmprov md <alanadı> zimbraAuthMech ad
zmprov md <alanadı> zimbraAuthLdapURL ldaps://<dc>:636
zmprov md <alanadı> zimbraAuthLdapBindDn "%u@<ad-alanadı>"
zmprov gd <alanadı> zimbraAuthFallbackToLocal      # boş ya da FALSE olmalı
zmprov mc <cos> zimbraFeatureChangePasswordEnabled FALSE
```

DC'nin CA sertifikası Zimbra'nın güven deposuna eklenir. Kullanıcılar için değişen tek şey, postaya artık Windows parolasıyla girmeleridir; telefon ve Outlook/IMAP istemcilerinde kayıtlı parola bir kez güncellenir. Geri düşme varsayılan olarak kapalıdır; açılmışsa herkesin eski Zimbra parolası ikinci bir geçerli parola olarak kalır, o yüzden değer kontrol edilir. Geçişten önce bir yönetici hesabının yerel doğrulamalı bir alan adında durduğundan emin olun; aksi halde kendinizi de kilitlersiniz. Adımlar `<lab'da doğrulanır — Zimbra bölümü>`. Bu geçiş OpenSicil'den bağımsızdır ve tek başına kazançtır: tek parola, AD'de kapatılan hesabın postaya da girememesi.

### Kimlik sağlayıcı (OIDC)
- Groups claim token'da olmalı; altı yönetim grubu ([ADR-005](decisions/005-yonetim-girisi-oidc.md), [ADR-019](decisions/019-ilk-parola-teslimi.md)).
- **Keycloak:** hazır bir "groups" kapsamı yoktur (`microprofile-jwt` kapsamındaki `groups` realm rolleridir). İstemciye claim adı `groups` olan bir **"Group Membership"** protokol mapper'ı eklenir. AD federasyonunda "MSAD User Account Control" mapper'ı eklenir ve **"Always Read Enabled Value From LDAP"** açılır; varsayılanı kapalıdır ve kapalıyken AD'de pasifleştirilen kullanıcı bir sonraki eşitlemeye kadar Keycloak'ta etkin görünür ([docs/11](11-dogrulama-notlari.md)).
- **Entra ID:** 200'den fazla gruptaki kullanıcı için groups claim hiç gelmez (overage). Uygulama kaydında "uygulamaya atanmış gruplar" filtresini kullanın.
- Kullanıcının kendi kullanıcı adını IdP'de değiştirmesi kapalı olmalı; kendi kaydına işlem yasağı buna dayanır.
- **Lab:** `compose.lab.yaml` (`quay.io/keycloak/keycloak:26.7.5`, host portu 8081) `keycloak-lab/realm-opensicil.json`'ı içe aktarır: istemci `opensicil-backend`, yukarıdaki `groups` mapper'ı, altı yönetim grubu ve test kullanıcıları (`test-admin` → `OpenSicil-Admins`, `test-hr` → `OpenSicil-HR`, `test-none` → grupsuz). **Samba AD** (`samba-lab/config.json`, ADR-027, ADR-074) aynı dosyada: realm `OPENSICIL.LAB`, NetBIOS `OPENSICIL`, DC `DC1`, LDAPS host portu 6360. Kurulum sırası: `sh samba-lab/gen-tls.sh` (SAN'lı lab sertifikası; Samba'nın kendi ürettiği sertifika yalnızca `dc1.opensicil.lab` için geçerlidir, host'tan `localhost` ile bağlanılamaz) → `docker compose -f compose.yaml -f compose.lab.yaml up -d samba-ad` → `sh samba-lab/seed.sh` (yönetilen OU'lar `OU=Personel`, `OU=Pasif,OU=Personel`, `OU=Gruplar`; katalog grupları; yasaklı grup örnekleri; `mevcut.personel`; AD giriş kapısı için kapsam dışı `OU=Disarida`'da `lab.operator` → `GG-Lab-Operators` → `OpenSicil-Admins`, ADR-105). Kapsam değişkenleri lab için: `AD_MANAGED_USER_OUS=OU=Personel,DC=opensicil,DC=lab`, `AD_PASSIVE_OU=OU=Pasif,OU=Personel,DC=opensicil,DC=lab`, `AD_MANAGED_GROUP_OUS=OU=Gruplar,DC=opensicil,DC=lab`; Yapılandırma sayfasında Host `localhost:6360` (host'tan) ya da `samba-ad` (container'dan), Bind DN `CN=Administrator,CN=Users,DC=opensicil,DC=lab`. Worker lab testi: `AD_LAB_URL=ldaps://localhost:6360 AD_LAB_BIND_DN=… AD_LAB_PASSWORD=… AD_CA_FILE=samba-lab/tls/ca.pem cargo test -- --include-ignored`. **Backend'in AD giriş testi aynı değişkenleri ister** (`ad_login_flow_against_lab_samba`): `cd backend && DATABASE_URL=… AD_LAB_URL=… AD_LAB_BIND_DN=… AD_LAB_PASSWORD=… AD_CA_FILE=../samba-lab/tls/ca.pem cargo test -- --include-ignored`. Sağlık kontrolü `samba-tool domain info 127.0.0.1`. **Uçtan uca:** test Postgres'i, lab Keycloak ve lab Samba ayaktayken `sh scripts/e2e-lab.sh` gerçek backend + worker ile kayıt → AD'de pasif hesap → ekranda "açıldı" yolunu ve "kaydet ve ilk parolayı ver" → parola ekranda yolunu (N-13 süresini ölçer, ADR-056) yürütür ve temizler (ADR-079); faz kapanışlarında tekrarlanır.
- **Gerçek Windows Server AD:** Samba'nın taklit edemediği davranışlar (parola politikasının ad kontrolü, `<GUID=…>` yazma hedefi) için `worker/src/ad_account.rs::windows_ad_answers_open_questions` testi vardır; `AD_WIN_URL` (DC'nin FQDN'i), `AD_WIN_BIND_DN` (UPN ya da DN), `AD_WIN_PASSWORD`, `AD_WIN_CA_FILE` (kök CA PEM'i) ve `AD_WIN_OU` (test hesabının açılacağı OU) verilerek `cargo test windows_ad_answers_open_questions -- --ignored` ile çalışır; açtığı tek hesabı siler. Sonuçlar [docs/11](11-dogrulama-notlari.md) W1–W10.
- **İkisi birden (worker'ın tam koşusu):** Samba lab ve gerçek Windows AD birbirinin yerine geçmez, birlikte çalışır — Samba tekrarlanabilir ve ağdan bağımsızdır, gerçek AD yalnızca onun taklit edemediği davranışları ölçer. İki ortamın değişkenleri aynı komutta verilebilir ve `--include-ignored` hepsini tek koşuda çalıştırır:
  ```
  cd worker && DATABASE_URL="postgres://testuser:testpass@localhost:15432/testdb" \
    AD_LAB_URL=ldaps://localhost:6360 \
    AD_LAB_BIND_DN="CN=Administrator,CN=Users,DC=opensicil,DC=lab" \
    AD_LAB_PASSWORD=… AD_CA_FILE=../samba-lab/tls/ca.pem \
    AD_WIN_URL=ldaps://<dc-fqdn>:636 AD_WIN_BIND_DN=<upn> AD_WIN_PASSWORD=… \
    AD_WIN_CA_FILE=../tmp/<kök-ca>.pem AD_WIN_OU="<test OU'su>" \
    cargo test -- --include-ignored
  ```
  Değişkenler verilmezse o testler **atlanmaz, başarısız olur** ("AD_LAB_URL ayarlanmalı") — bir ortamın kapalı olduğu sessizce geçmesin diye böyle. Yalnızca birini koşmak için diğerinin değişkenleri verilmez ve o testler elenir (`cargo test lab -- --ignored`).
- **Ayrılan operatörü yönetim gruplarından elle çıkarın.** OpenSicil yönetim grupları (`OpenSicil-HR`, `OpenSicil-Admins`…) kataloğa alınamaz ([ADR-005](decisions/005-yonetim-girisi-oidc.md)); ayrılış bu üyeliği **kaldırmaz**. Kişi `ayrıldı` iken zararsızdır (hesap kapalı, her isteği reddedilir — [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)); ama ayrılış geri alınırsa ya da kişi aylar sonra başka bir göreve dönerse operatör yetkisi **sessizce** geri gelir. Acil ayrılış ekranı, ayrılan kişi operatörse bunu hatırlatır.
- MFA IdP'de zorunlu tutulur.

### Sunucu
- **İmajları kendiniz derliyorsanız BuildKit gerekir** (`docker buildx`; Docker'ın güncel sürümlerinde varsayılan derleyicidir, Arch/CachyOS'ta `docker-buildx` paketi ayrıdır). Backend imajı arayüz kaynaklarını `frontend/` dizininden adlandırılmış ek bağlamla alır ([ADR-097](decisions/097-frontend-dizini.md)); klasik derleyici bunu desteklemez ve `the classic builder doesn't support additional contexts` hatası verir. Hazır imaj kullanan kurum için gerekmez.
- `.env` dosyası `600` izinli ve servis kullanıcısına ait. **Docker grubuna üyelik sır erişimi demektir**; `docker inspect` ortam değişkenlerini düz metin gösterir ([ADR-006](decisions/006-sirlar-env.md)).
- Şifreleme ve blind index anahtarlarının yedeği veritabanı yedeğinden **ayrı** tutulur; anahtar kaybolursa kimlik numaraları okunamaz ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md)).
- **PostgreSQL yedeği kurumun işidir** (günlük döküm ya da PITR). Yedek yoksa hesap bağlantıları (kimlik ↔ objectGUID/zimbraId) kaybolur ve bütün kurum yeniden sahiplenilir; [yedekten dönüş](#yedekten-dönüş) adımları bir yedeğin var olduğunu varsayar.
- Backend'in AD'ye ve Zimbra'ya ağ düzeyinde ulaşamaması: `<kurulumda kararlaştırılır>`.
- Ortak ayarlar (sahiplenme, saatlik sayaç sınırları ve acil kota, hassas kaynak eşlemesi, saat dilimi) `.env`'den hem backend'e hem worker'a verilir; `.env` değişince iki servis birlikte yeniden başlatılır ([ADR-039](decisions/039-ortak-ayarlar-env.md)). Servisler ayrı host'larda ya da Kubernetes'teyse: [dağıtım biçimleri](#dağıtım-biçimleri).

## Dağıtım biçimleri

Aynı imajlar üç biçimde çalışır ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)). Kubernetes ölçek için değil, kurum iş yüklerini zaten orada işlettiği için seçilir: **worker'ı çoğaltmak hız kazandırmaz**, hızın sınırı tek sıra ve aşağıdaki saatlik sayaçlardır.

| Topoloji | Nerede ne çalışır | Dikkat |
|---|---|---|
| **A. Tek sunucu** | Hepsi tek VM'de: `docker compose up -d` | Referans kurulum; küçük ve orta kurum |
| **B. Uygulama + veritabanı** | Sunucu 1: nginx, backend, worker, tek seferlik `migrate`. Sunucu 2: kurumun PostgreSQL'i (tek sunucu ya da küme) | compose'daki `db` başlatılmaz. `DATABASE_URL` içinde **`sslmode=verify-full`** ve CA dosyası: `sqlx` varsayılanı `prefer`'dir, sunucu TLS sunmazsa hata vermeden düz metne düşer ([docs/11](11-dogrulama-notlari.md#dağıtım) D13). Yedek kurumun işidir ([sunucu](#sunucu)) |
| **C. Ön yüz + worker** | Sunucu 1 (kullanıcı bölgesi): nginx, backend. Sunucu 2 (yönetim bölgesi; DC'lere ve Zimbra 7071'e erişen): worker. PostgreSQL ikisinden birinde ya da dışarıda | Her sunucunun `.env`'inde **yalnızca orada çalışan servislerin sırları** durur: AD ve Zimbra parolası sunucu 1'e hiç konmaz ([ADR-006](decisions/006-sirlar-env.md)). Backend'in AD'ye ağ düzeyinde ulaşamaması burada güvenlik duvarıyla kendiliğinden sağlanır |
| **D. Kubernetes** | Aşağıdaki eşleme | Backend bir ya da daha çok kopya, worker tek kopya |

Dört topolojide de imajlar ve `compose.yaml` aynıdır; B ve C'de aynı dosyadan yalnızca o sunucunun servisleri başlatılır: önce `docker compose run --rm --no-deps migrate` ile şema hazırlanır, sonra `docker compose up -d --no-deps <servis...>` ile o sunucuya ait servisler açılır (ör. B'de sunucu 1: `nginx backend worker`; C'de sunucu 1: `nginx backend`, sunucu 2: `worker`). `--no-deps` compose'un `depends_on` üzerinden yerel `db` servisini kendiliğinden başlatmasını engeller; o sunucuda çalışmayan servis dosyadan silinmez, yalnızca başlatılmaz. Ölçekle ilişkisi gevşektir: 50.000 kimlik A'da da çalışır; B ve C'yi kimlik sayısı değil kurumun veritabanı ve ağ bölgesi düzeni seçtirir. Eşikler [boyutlandırma](#boyutlandırma) tablosundadır. Kaynak ihtiyacı (CPU, bellek): `<Faz 5 N-03 ölçümünde doldurulur>`.

**B ve C'de `.env` birden fazla kopyadır;** [ADR-039](decisions/039-ortak-ayarlar-env.md)'un "aynı dosya, aynı komut" varsayımı kalkar. Ortak ayarları (sayaç sınırları, sahiplenme, saat dilimi, hassas kaynak eşlemesi) ve AEAD ile blind index anahtarlarını **tek kaynaktan** dağıtın. Anahtar farkı gürültülüdür (ilk parola çözülemez); sayaç sınırı farkı sessizdir: ekran bir sayı gösterir, worker başkasını uygular. Kontrol: iki servis de açılışta etkin ortak ayarları tek satır loglar (`backend: ortak ayarlar: …` / `worker: ortak ayarlar: …`); değerler birebir aynı olmalı. Eksik ya da geçersiz değer (sıfır saatlik sınır, `true`/`false` dışı bayrak, biçimsiz `TZ`) servisi başlatmaz.

### compose → Kubernetes eşlemesi

Ürün chart ya da manifest yayımlamaz. `compose.yaml` referanstır:

| compose | Kubernetes |
|---|---|
| `nginx` ve `ports:` | Deployment + Service, önünde Ingress; TLS Ingress'te sonlanır, `PUBLIC_URL` dış adrestir |
| `backend` | Deployment, bir ya da daha çok kopya. Hazır olma yoklaması `GET /api/health`, canlılık yoklaması TCP |
| `worker` | Deployment, **`replicas: 1`, `strategy: Recreate`**, Service yok. Varsayılan `RollingUpdate` tek kopyada yenisini eskisi ölmeden başlatır; `Recreate` de elle silinen pod'da "en fazla bir" garantisi vermez. İkisi de güvenlidir, yalnızca saatlik sayaç bir iş aşabilir ([ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md)). Canlılık yoklaması `exec: worker-health`. `terminationGracePeriodSeconds` 30'un altına inmesin |
| tek seferlik `migrate` servisi | Job; sahip rolünün Secret'ı yalnızca burada. Servisler şema eskiyse çıkıp yeniden dener, sıralama gerekmez |
| `db` | Kurumun PostgreSQL'i ya da bir operator; yedeği yine kurumun işidir |
| servis bazında `environment:` | Sırlar Secret'ta, container başına `secretKeyRef` ile: her sır yalnızca [ADR-006](decisions/006-sirlar-env.md) tablosundaki servise. Ortak ayarlar **tek** ConfigMap'te, iki Deployment'ta `envFrom` |
| CA sertifikası dosyası | ConfigMap volume |
| `/tmp` | `readOnlyRootFilesystem: true` ise `emptyDir` |
| `stop_grace_period: 30s` | varsayılan zaten 30 sn |

### Kubernetes'e özel ön koşullar

| Ön koşul | Not |
|---|---|
| Worker'ın çıkış adresi | Pod trafiğinin küme dışında hangi adresle görüneceği ağ eklentisinin masquerade ayarına bağlıdır: o an çalıştığı **node'un IP'si** ya da **pod IP'si**; ikisi de sabit tek bir adres değildir ([docs/11](11-dogrulama-notlari.md#dağıtım) D9). "7071 yalnızca worker'ın IP'sine açık" ([Zimbra](#zimbra)) ve DC güvenlik duvarı kuralları için worker'ı sabit bir node'a bağlayın (`nodeSelector`) ya da çıkış ağ geçidi kullanın. Bütün node'lara ya da pod ağına açarsanız backend pod'ları da aynı adreslerle çıkar ve backend ile worker'ın ağ ayrımı güvenlik duvarında kaybolur |
| Çıkış yönlü NetworkPolicy | Backend yalnızca PostgreSQL'e ve IdP'ye, worker yalnızca PostgreSQL, DC'ler ve Zimbra'ya; worker'a giriş yok (N-08). NetworkPolicy'yi uygulamayan ağ eklentisinde nesne **sessizce etkisizdir**. Kontrol: backend pod'undan DC'nin 636 portuna bağlantı denemesi başarısız olmalı |
| Ingress gövde sınırı | Ingress controller'ınızın istek gövdesi sınırını nginx'teki `client_max_body_size` ile eşitleyin; aksi halde büyük CSV içe aktarmada 413 döner. Örnek: ingress-nginx varsayılanı 1 MB'tır (`nginx.ingress.kubernetes.io/proxy-body-size`); bu proje Mart 2026'da emekliye ayrıldı, yeni kurulumda başka bir controller ya da Gateway API seçin |
| Metrik toplama | Prometheus backend pod'una doğrudan gider; `authorization` ile Bearer token verin |
| Bağlantı havuzlayıcısı | Gerekmez: OpenSicil az sayıda bağlantı açar; **doğrudan** ya da session modunda bağlanın. PgBouncer `transaction` modu zorunluysa migration Job'ı yine doğrudan bağlanır: `sqlx` migration kilidi oturum düzeyi `pg_advisory_lock`'tur ve bu modda desteklenmez (D5, D6). Backend ve worker için PgBouncer ≥ 1.21 ve `max_prepared_statements` > 0 gerekir; bu yol v1'de **sınanmaz** |

## Boyutlandırma

Eşikler ortam değişkenidir ve mutlak sayıdır: değişiklik seti eşiği ve onay zaman kilidi backend'de ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md)), saatlik sayaçlar ve acil kota worker'da ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md), [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)). Değişiklik seti eşiği, yetki veya hesap durumu farkı üreten kimlik sayısını sayar; ekleme de dahildir ([ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md)). Ölçüldükçe güncellenir.

| Aktif kimlik | Değişiklik seti eşiği | Saatlik yıkıcı | Saatlik verme | Saatlik ilk parola | Acil kota |
|---|---|---|---|---|---|
| < 500 (varsayılan) | 10 | 50 | 50 | 50 | 5 |
| 500–5.000 | 25 | 150 | 150 | 150 | 10 |
| 5.000–50.000 | 50 | 500 | 500 | 300 | 20 |

Eşik, bağlantısı gözlem modunda olan ya da o hedefte hesabı olmayacak kimlikleri saymaz ([ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md)). Verme sayacı, hesabı açılan ya da mevcut hesabına grup, liste veya COS yazılan kimliği sayar; bir iş gereken sayaçlardan biri doluysa bütünüyle bekler ([ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)). v1'de eşzamanlılık ayarı yoktur: yazma işleri tek sırada, mutabakat gibi okuma işleri ayrı şeritte çalışır ([ADR-051](decisions/051-okuma-seridi.md)).

**İlk alarm:** metrik ucundaki "ayrılmış ama kapatılamamış" değeri sıfırdan büyükse bir ayrılanın hesabı açık kalmıştır ([ADR-052](decisions/052-uygulanamayan-fark.md)). İkincisi "hedef sistem başına son başarılı bağlantı", üçüncüsü "silinmeyi bekleyen en eski hesabın yaşı"dır.

**Tek Sistem yöneticisi olan kurum:** onay zaman kilidini açın (öneri 4 saat); aksi halde eşiği aşan değişiklik setini onaylayacak kimse olmaz ([ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md)). İki yönetici varsa kapalı bırakın.

## Ayarlar ve öneriler

| Ayar | Varsayılan | Not |
|---|---|---|
| İlk girişte değiştirme işareti (`FIRST_LOGIN_CHANGE_REQUIRED`, worker; boş = açık) | açık | İşaretli kullanıcı **Zimbra'ya giremez** (AD bind'ı "parola değiştirilmeli" hatasıyla reddeder, Zimbra parola değiştirtemez). **Keycloak** yalnızca federasyon `WRITABLE` modda ve "MSAD User Account Control" mapper'ı ekliyse parola değiştirme ekranı açar; `READ_ONLY` modda düz "geçersiz kimlik bilgisi" der. Kural: personelin ilk girişi domain bilgisayarından ya da `WRITABLE` Keycloak'tan yapılıyorsa **açık** bırakın; ilk giriş yeri webmail ya da `READ_ONLY` Keycloak olan personeliniz varsa **kapatın** ([ADR-019](decisions/019-ilk-parola-teslimi.md), [docs/11](11-dogrulama-notlari.md)). Kapalıysa teslim edilen parola kişi değiştirene kadar geçerlidir; ilk girişte değiştirmeyi kurum kuralı yapın |
| Saklama süresi, AD | 90 gün | Dolunca hesap otomatik silinir |
| Saklama süresi, Zimbra | onayla | Otomatik silme kapalı; "silinmeyi bekliyor" listesinden onaylanır ([ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md)) |
| Ayrılışta parola sıfırlama gecikmesi | 7 gün | `0` = ayrılışta hemen; acil ayrılışta her zaman hemen ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md)) |
| Ayrılan hesabın Zimbra durumu | `locked` | `closed` gönderene hata döndürür; yönlendirme ve yanıt yazılmaz |
| Ayrılan postası yönlendirme | **kapalı** | Açıksa devir yöneticisinin bağlı Zimbra adresine; devir yöneticisi yoksa yazılmaz. Ayrılana gelen bütün postayı bir başkasına akıtır: açmadan önce aydınlatma metninizi ve e-posta politikanızı gözden geçirin ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md), [ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)) |
| Ayrılan otomatik yanıt | açık | Şablon metni ayardır; `{given} {surname}` ve `{devir_mail}` yer tutucuları ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md)) |
| Ayrılan postası gecikmesi | 24 saat | Otomatik yanıt, yönlendirme ve adres defterinden gizleme bitişten bu kadar sonra yazılır; acil ayrılışta hemen. Kullanıcının kendi yönlendirmesi ve filtresi gecikmeden, ayrılış anında temizlenir ([ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)) |
| Kuru çalıştırma | kapalı | Açıkken worker hedefe hiçbir şey yazmaz, farkı gösterir. İlk kurulumda **açık başlatın**; bağlantıyı, kapsamın çözülmesini ve kataloğu doğrulayıp kapatın. Delegasyon kuru modda **sınanamaz** (yazma yapılmaz): kapattıktan sonra bir test kimliğiyle hesap açıp gruba ekleyin, pasifleştirin ve kaydı iptal edin; eksik yetki işi "müdahale gerekiyor"a düşürür ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)) |
| Denetim kaydı saklama | 24 ay | |
| Saat dilimi | `Europe/Istanbul` | Tüm tarihler bu dilimde yorumlanır |
| Sahiplenme | kapalı | Yalnızca geçiş döneminde açın, bitince kapatın; açıkken CSV'de ipucu zorunludur ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md), [ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md)) |
| Onay zaman kilidi | 0 (kapalı) | Yukarıdaki tek yönetici notu; backend ayarı ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md)) |
| Hassas kaynak eşlemesi | kapalı | Kimlik numarası ve telefonun hedefe eşlenmesi; worker ayarı ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md)) |
| Kuyruk yoklama | 5 sn | ([ADR-028](decisions/028-worker-zamanlamasi.md)) |
| İş kirası | 5 dk (ayar değil) | Worker iş ortasında öldürülürse o iş en fazla bu kadar bekler, deneme hakkı azalmaz. Sürüm yükseltmede beklemez: worker SIGTERM'de elindeki işi bitirir ([ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md)) |
| Metrik ucu erişimi | Bearer token | Backend ortam değişkeni; token'sız istek 401 alır ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)) |
| İstek hız sınırı (nginx) | gezinme 30 r/s (burst 60), giriş yolları 12 r/dk (burst 5), IP başına 24 eşzamanlı bağlantı | `nginx/nginx.conf`, ortam değişkeni değil. Sınır **kaynak IP başına**dır: bütün operatörler tek NAT adresinin arkasındaysa payları ortaktır — o kurulumda değerleri yükseltin. Aşan istek backend'e hiç gitmez, 429 ve "İstek kabul edilmedi" sayfası alır. Yerel hesabın 5 deneme/15 dk kilidi ([ADR-095](decisions/095-giris-kendi-ekranimiz-ad-bind-asil.md)) bundan bağımsız, uygulamada durur |
| İzin verilen HTTP metotları | `GET`, `HEAD`, `POST` | Uygulamanın kullandığı küme; gerisi nginx'te 405 alır ve backend'e ulaşmaz |

**Soyad değişimi (v1):** OpenSicil görünen adı ve CN'i günceller; kullanıcı adı ve e-posta değişmez (F-25 v2'dedir). Hesabı AD'de ya da Zimbra'da **elle yeniden adlandırmayın**: kaynak OpenSicil'deki addır, eşlenmiş `mail` bir sonraki işte eski adrese geri yazılır ([ADR-012](decisions/012-oznitelik-esleme.md)) ve kişi sayfası eski adı gösterir. Yeni soyadla adres gerekiyorsa Zimbra'da takma ad ekleyin; OpenSicil takma adlara dokunmaz.

## Aynı dakika giriş

"Kaydet ve ilk parolayı ver" ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)) hesabı bir dakika içinde hazırlar; kişinin o dakika giriş yapabilmesi ayrıca şunlara bağlıdır:

| Konu | Ne olur | Ne yapın |
|---|---|---|
| AD replikasyonu | Worker tercihli DC'ye yazar. Kişinin bilgisayarı başka bir DC'ye giderse hesap oraya replike olana kadar "kullanıcı bulunamadı" alır: aynı site'ta saniyeler, site'lar arası varsayılan **180 dakika** (en az 15) | Şube personelini bir gün önceden kaydedin (ürünün asıl akışı budur; hesap pasif bekler) ya da site link'te değişiklik bildirimini açın |
| Zimbra o sırada kapalı | AD hesabı ve parola beklemez; ad üretimi Zimbra kontrolünü atlar, mailbox bağlantı gelince açılır ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md)) | Bir şey yapmayın; kişi sayfası "Zimbra'ya ulaşılamıyor" der |
| Zimbra'nın doğrulama yaptığı DC | Zimbra başka bir DC'ye soruyorsa aynı gecikme webmail'de görülür | `zimbraAuthLdapURL` worker'ın tercihli DC'sini göstersin |
| "İlk girişte değiştir" işareti | İşaret açıkken ilk giriş domain bilgisayarından yapılmalıdır; webmail ve çoğu SSO ilk giriş yeri olamaz | Domain bilgisayarı olmayan personeli olan kurum işareti kapatır ([ADR-019](decisions/019-ilk-parola-teslimi.md)) |

AD grubundan okunmayan şeyler (kartlı geçiş, kendi kullanıcı tablosu olan uygulamalar, ev dizini klasörü) v1'de OpenSicil'in işi değildir ([docs/02](02-mimari.md#uygulamalar-nasıl-bağlanır)); "her şeyim hazır" iddiası AD hesabı, grupları, OU'su, mailbox'ı ve bunlardan yetki okuyan her şey içindir.

## Yedekten dönüş

Veritabanı yedeğe dönünce yedekten sonra açılan hesapların bağlantısı, kaydedilen ayrılışlar ve kullanılmış adlar kaybolur. Worker doğrudan başlatılmaz ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)):

1. Worker'ı durdurun, veritabanını geri yükleyin.
2. Worker'ı **kuru çalıştırma** açıkken başlatın ve mutabakatı çalıştırın.
3. Bulguları giderin: "elle pasifleştirilmiş hesap" (sonra kaydedilmiş ayrılışlar; ayrılışı yeniden girin), "kayıp hesap" (sonra silinenler), "yönetilmeyen hesap" (yedekten sonra açılanlar; kişiyi **mevcut hesap ipucuyla** yeniden kaydedin ya da hesabı silin).
4. Kuru çalıştırmayı kapatın. Kuru modda sahiplenme isteği reddedilir ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)); 3. adımda ipucuyla kaydedilenler şimdi sahiplenilir (sahiplenme ayarı açık olmalı). İpucusuz yeniden kaydedilen kişi ikinci hesap açtırmaz: ad çakışması işi müdahaleye düşürür ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md)).

Motor, kapatılmış hesabı geri açmaz ve silinmiş hesabı yeniden açmaz ([ADR-032](decisions/032-elle-pasiflestirme-korunur.md), [ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)); ama kaybolan ayrılışların grupları geri yazılır. 3. adım bu yüzden atlanmaz. Aynı mod sürüm yükseltmeden sonraki ilk açılışta ve DC ya da Zimbra bakım penceresinde de kullanılır.

## Sır sızarsa

`.env` (ya da bir yedeği) yanlış bir yere düşerse — git geçmişi, bir ticket eki, bir ekran paylaşımı — oradaki değer **yanmış sayılır**. Dosyayı silmek yetmez: silinen bir blob'u geçmişten çıkarmak başka iş, onu okumuş olanı geri almak imkânsız. Tek gerçek düzeltme, değeri geçersiz kılmaktır ([ADR-104](decisions/104-env-yedegi-public-repoya-push-edildi.md)).

**Hangi değer neyi açar:**

| Değer | Neyi açar | Sızarsa ne olur |
|---|---|---|
| `POSTGRES_OWNER_PASSWORD` | şema sahibi rolü | Şemayı değiştirebilir, her tabloyu okur/yazar. Veritabanının `ports:` satırı yoktur ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)), yani saldırganın ayrıca ağa girmesi gerekir |
| `POSTGRES_BACKEND_PASSWORD` | backend servis rolü | Kimlik, rol, departman, oturum tabloları; denetim kaydına yalnızca ekleme |
| `POSTGRES_WORKER_PASSWORD` | worker servis rolü | Hesap bağlantıları, iş kuyruğu, katalog |
| `AEAD_MASTER_KEY` | DB'deki **bütün şifreli değerler**: AD servis hesabı parolası, Zimbra admin parolası, OIDC client secret, kimlik numaraları, bekleyen ilk parolalar ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md), [ADR-036](decisions/036-ilk-parola-aead.md), [ADR-068](decisions/068-yapilandirma-sayfasi-ve-bootstrap-hesabi.md)) | Tek başına hiçbir şey; veritabanı dökümüyle birlikte **her şey**. Bu yüzden `.env` ile yedek asla aynı yerde durmaz |
| `BLIND_INDEX_KEY` | kimlik numarası blind index'i (HMAC) | Elindeki bir kimlik numarasının bu kurumda olup olmadığını döküm üstünde sınayabilir |
| `METRICS_TOKEN` | metrik ucu | Sayaçları okur (kişisel veri içermez) |

**Rotasyon sırası** — `sh scripts/sir-rotasyonu.sh --onayla` 1–4'ü yapar, 5–6 elle:

1. **Ön koşul:** yerel break-glass `admin` hesabıyla girebildiğinizi doğrulayın ve AD servis hesabı parolasını hazırlayın. AEAD anahtarı değişince OIDC girişi, client secret yeniden girilene kadar çalışmaz — o aralıkta tek kapı yerel hesaptır ([ADR-095](decisions/095-giris-kendi-ekranimiz-ad-bind-asil.md)).
2. `.env`'deki altı değer yeniden üretilir. Postgres parolaları DSN'e girdiği için yalnızca alfanumerik; iki anahtar `openssl rand -base64 32` (32 bayt, base64 — başka uzunluk açılışta reddedilir).
3. Şema sahibi rolü `ALTER ROLE … PASSWORD` ile döner. `POSTGRES_PASSWORD` ortam değişkeni **yalnızca ilk `initdb`'de** okunur; mevcut bir veritabanında onu değiştirmek hiçbir şey yapmaz.
4. `docker compose run --rm --no-deps migrate` backend ve worker rollerinin parolasını `ALTER ROLE` ile döndürür ([ADR-015](decisions/015-veritabani-rolleri.md)); ardından `docker compose up -d backend worker` iki servisi yeni anahtarlarla yeniden oluşturur. **Veritabanı silinmez.**
5. Yerel `admin` ile girip Yapılandırma sayfasından **AD servis hesabı parolasını ve OIDC client secret'ı yeniden girin** (Zimbra bağlıysa onun admin parolasını da). Sır alanı boş bırakılırsa backend eski şifreli değeri korur (`COALESCE`) — o değer artık çözülemez, yani bu alanlar **dolu gönderilmelidir**.
6. Doğrulama: `/` 200 dönüyor, OIDC girişi yeniden çalışıyor, hedef sistemin mutabakat ekranındaki "Yeniden tara" AD'ye bağlanabiliyor.

**AEAD anahtarı değişince ne yeniden girilir:** `app_settings`'teki üç sır (AD, Zimbra, OIDC). Bekleyen ilk parolalar okunamaz hale gelir — zaten 10 dakikalık ömürleri var, operatör yeniden ister. **Kimlik numarası dolu bir kurulumda bu rotasyon tek başına yapılamaz:** `national_id_enc` eski anahtarla şifrelidir ve `national_id_bidx` eski HMAC anahtarıyla üretilmiştir; v1'de yeniden şifreleme aracı yoktur ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md) sürüm baytını bıraktı, dönüştürücüyü değil). Betik bu durumda durur ve hiçbir şeye dokunmaz.

**Git geçmişi yeniden yazılmaz** ([ADR-104](decisions/104-env-yedegi-public-repoya-push-edildi.md)): `git push --force` proje kuralıyla yasak ve GitHub silinen blob'u kendi çöp toplamasına kadar sunmaya devam eder, yani yeniden yazma tek başına sırrı geri almaz. Rotasyon geçmişte duran değeri değersizleştirir; asıl düzeltme budur.

## Migration
Aynı imajın `migrate` alt komutu, şema sahibi rolüyle **tek seferlik container** olarak çalışır: compose'da backend ve worker'ın `service_completed_successfully` ile beklediği servis, Kubernetes'te Job. Sahip rolünün parolası yalnızca bu container'a verilir ([ADR-015](decisions/015-veritabani-rolleri.md), [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)). Backend ve worker açılışta şema sürümüne bakar; eskiyse çıkar, orkestratör yeniden başlatır. Komutlar: tek sunucuda (topoloji A) `docker compose up -d` migrate'i `depends_on: service_completed_successfully` ile otomatik önce çalıştırır; elle ya da B/C'de tek başına çalıştırmak için `docker compose run --rm --no-deps migrate`.

## İlk giriş
`.env`'de artık yalnızca DB bağlantısı ve AEAD ana anahtarı var; AD/Zimbra/OIDC ayarları web'den girilir ([ADR-068](decisions/068-yapilandirma-sayfasi-ve-bootstrap-hesabi.md)):

0. `.env`'i `.env.example`'dan türetin. `PUBLIC_URL` **tarayıcının gördüğü adres** olmalıdır (OIDC redirect URI buradan üretilir): tek makinede `https://localhost`, ağdan girilecekse `https://<sunucu-ip>`. `TLS_CERT_PATH`/`TLS_KEY_PATH` sertifikası aynı adı/IP'yi SAN'ında taşımalı — tarayıcı CN'e bakmaz. Geliştirmede: `sh nginx/gen-tls.sh <ek-ad-ya-da-ip …>` (self-signed; tarayıcı CA'yı tanımadığı için ilk açılışta uyarı verir, `nginx/tls/cert.pem`'i işletim sistemine/tarayıcıya güvenilir olarak eklerseniz uyarı kalkar). `PUBLIC_URL` değişirse OIDC sağlayıcısındaki izinli redirect URI listesi de güncellenir.
1. `docker compose up -d` (migrate önce çalışır, `bootstrap_account` tablosuna `admin`/`admin` seed edilir).
2. `https://<PUBLIC_URL>/login` adresine `admin` / `admin` ile girin. Giriş ekranı OpenSicil'in kendisidir ([ADR-095](decisions/095-giris-kendi-ekranimiz-ad-bind-asil.md)): tek form, kapıyı kullanıcı adı seçer — `admin` yerel break-glass hesabına, başka her ad AD'ye gider ([ADR-105](decisions/105-ad-bind-kapisi-uygulamasi.md)).
3. İlk girişte eski parola sorulmadan yeni bir parola girmeniz istenir (en az 12 karakter). Yerel hesap kalıcıdır ve Sistem yöneticisi (`admin`) yetkisiyle gerçek bir operatör oturumu açar; 5 başarısız denemede 15 dakika kilitlenir, her girişi denetim kaydına `source = local` ile düşer. Kurtarma içindir: günlük iş AD kapısından yürür.
4. Yapılandırma sayfasından AD bağlantısını (host, bind DN, servis hesabı parolası) girin; Zimbra (admin URL, admin parolası) ve OIDC (issuer, client id/secret) alanları isteğe bağlıdır. Sır alanları boş bırakılırsa mevcut değer korunur; kaydedilen sırlar ekranda bir daha düz metin gösterilmez.
5. AD bağlantısı kaydedildiği anda **asıl kapı** açılır: operatörler kendi AD kullanıcı adı (`sAMAccountName` ya da UPN) ve parolasıyla girer — backend kullanıcıyı servis hesabıyla arar, bulunan DN ile operatörün parolasını LDAPS üstünden `simple_bind` ile dener; parola hiçbir yere yazılmaz, loglanmaz. Yetkiler AD grup üyeliğinden okunur (`OpenSicil-Admins`, `OpenSicil-HR`, … iç içe üyelik dahil; OIDC ile aynı eşleme). Yönetim grubunda olmayan AD kullanıcısı girer ama hiçbir ekranı göremez. Servis hesabı ya da DC erişilemezse ekranda "AD'ye ulaşılamıyor" yazar, yerel kapı çalışmaya devam eder. AD'de `admin` adlı bir operatör bu kapıdan giremez: ad yerel hesaba ayrılmıştır.
6. **İsteğe bağlı OIDC:** MFA, parola sıfırlama ya da hesap kilitlemeyi bir IdP'ye bırakmak isteyen kurum Yapılandırma'dan issuer, client id ve client secret girer; `/login`'de "OIDC ile giriş" düğmesi görünür, `/oidc/login` IdP'ye yönlendirir, yetkiler dönen `id_token`'ın `groups` claim'inden aynı eşlemeyle okunur ([ADR-065](decisions/065-oidc-akisi-backend.md), [ADR-073](decisions/073-oidc-crate-secimi.md)). OIDC ön koşul değildir ve yerel form ilk OIDC girişinden sonra da gizlenmez (ADR-068'in o maddesi ADR-095 ile kalktı). `PUBLIC_URL` değişirse IdP'deki izinli redirect URI listesi de güncellenir.

## Arayüz
Ekran HTML'i backend'in içinden gelir; ayrı bir frontend container'ı, Node çalışma zamanı ya da dış CDN yoktur ([ADR-064](decisions/064-frontend-htmx-tailwind.md), [ADR-088](decisions/088-arayuz-kabugu-derlenmis-css-tema-font.md)). CSS, font ve küçük tema betiği binary'ye gömülüdür ve `/static/<dosya>` altından, bir yıl `immutable` cache ile sunulur; sayfa hiçbir dış adrese istek atmaz (nginx CSP'si `default-src 'self'`).

- **Renk teması** açık ve koyu ([ADR-100](decisions/100-renk-paleti-catppuccin-yerine-slate-blue.md) paleti). Varsayılan tarayıcı/sistem tercihidir; üst bardaki düğme elle geçiş yapar ve seçim o tarayıcıda (`localStorage`) kalır. Sunucu tarafında ayar yoktur.
- **Arayüz fontu** CaskaydiaMono Nerd Font (Regular + Bold), `backend/static/` altında self-host.
- **Dil** TR ve EN ([ADR-089](decisions/089-arayuz-dili-tr-en-uygulamasi.md)): metinler `backend/i18n/tr.toml` ve `en.toml`'dadır, binary'ye gömülüdür. Üst bardaki `EN`/`TR` düğmesi tercihi operatörün oturumuna yazar (`operator_sessions.lang`), oturum bitince varsayılana döner; ayrı bir ortam değişkeni yoktur. Giriş, parola değiştirme ve Yapılandırma ekranlarında oturum henüz yoktur: dil tarayıcının `Accept-Language` başlığından gelir (`en…` → İngilizce, aksi halde Türkçe), seçici gösterilmez. Yeni ekran metni iki dosyaya da eklenir; eksik anahtarı `cargo test` yakalar.
- **Hata sayfaları** kabuğun içindedir: 403, 404, 405 ve 500 aynı şablonu kullanır, gövdede yalnızca durum kodu ve iki hazır cümle durur — yol, SQL ya da sürüm dışarı çıkmaz; ayrıntı yalnızca sunucu log'undadır. Backend ayakta değilken (502/503/504) ve istek nginx'te reddedildiğinde (405, 429) sayfayı nginx verir: o iki sayfa binary'deki CSS'e ulaşamadığı için kendi stilini taşır ve iki dillidir (içerik pazarlığı yapılmaz).
- **CSS'i yeniden üretme** (şablonlarda yeni bir sınıf kullanıldığında): `sh scripts/build-css.sh`. Betik Tailwind'in standalone CLI binary'sini (sürüm `4.3.3`, sha256 doğrulanır) `tmp/araclar/` altına indirir, `backend/assets/app.css`'ten `backend/static/app.css`'i üretir. Çıktı commit'lenir: `cargo build` ve imaj derlemesi onu olduğu gibi gömer, derleme ağ istemez. Node ya da `package.json` yoktur.
