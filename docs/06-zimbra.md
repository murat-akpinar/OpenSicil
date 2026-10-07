# 06 — Zimbra Connector'ı

## Ön koşullar (kurumun işi)

| Ön koşul | Neden |
|---|---|
| Worker'dan Zimbra admin portuna (**7071**, HTTPS) erişim; port başka hiçbir yere açık değil | Admin API bu porttadır |
| Yönetilen domain'de **parola kontrolünün AD'ye devredilmiş olması** (`zimbraAuthMech = ad` ya da `ldap`). E-posta yerel kısmı her zaman kullanıcı adına eşitse bind DN şablonu (`%u@<ad-alanadı>`) yeter ve iki mekanizma da olur. Yerel kısım kullanıcı adından farklı olabiliyorsa mekanizma **`ldap` olmalıdır**: arama filtresi (`zimbraAuthLdapSearchFilter=(mail=%n)`, arama için AD'de salt okuma bir hesap) **`ad` mekanizmasında yok sayılır** | Tek parola AD'de (ADR-009). Zimbra kodu filtreyi yalnızca `AuthMech.ad != authMech` iken uygular; `%n` her zaman hesabın birincil adresidir (kullanıcının yazdığı değil), AD `mail` özniteliğiyle eşleşir ([Kaynaklar](#kaynaklar)) |
| Yönetilen domain'de `zimbraAuthFallbackToLocal`'ın **`TRUE` yapılmamış olması** ve COS'ta webmail'den **parola değiştirmenin kapalı olması** (`zimbraFeatureChangePasswordEnabled = FALSE`; isteğe bağlı olarak kurumun AD parola sayfasını gösteren `zimbraChangePasswordURL`) | Geri düşme **varsayılan olarak kapalıdır** (öznitelik boşsa kod `false` okur); risk yalnızca birisi açmışsa vardır. Açıksa: webmail'den parola değiştirmek AD'ye değil Zimbra'nın yerel `userPassword`'üne yazar ve o parola, AD hesabı kapatıldıktan sonra da (SOC'un gece pasifleştirmesi dahil, ADR-032) postaya girer; yerel paroladan AD doğrulamasına sonradan geçmiş kurumda herkesin eski parolası da aynı şekilde çalışır. Zimbra yönetici kılavuzu AD doğrulaması kullanılıyorsa parola değiştirmenin COS'ta kapatılmasını zaten şart koşar. Yerel parolası hiç olmayan hesapta geri düşme hiçbir koşulda başarılı olamaz; OpenSicil hesapları bu yüzden **parolasız** açar (ADR-057) |
| Yönetilen domain'de **auto-provisioning'in kapalı olması** | İki ayrı hesap açan olursa çakışma çıkar (aşağıda) |
| OpenSicil'e özel bir admin hesabı | [Admin hesabı](#admin-hesabı) |

Parola kontrolü AD'ye devredilse bile hesabın Zimbra'da da var olması gerekir. Zimbra dokümanı bunu açıkça söyler: kullanıcılar hem Zimbra'nın kendi LDAP'ında hem harici dizinde bulunmalıdır. OpenSicil'in Zimbra'da hesap açmasının nedeni budur.

Zimbra AD'de doğrularken hesabı ya bind DN şablonundan (`zimbraAuthLdapBindDn`; `%n` = birincil adres, `%u` = yerel kısım, `%d` = alan adı, `%D` = `dc=…` biçimi) ya da arama filtresinden bulur. E-posta yerel kısmı kullanıcı adından farklı olabilir: uzunluk kısaltmasında `a.karaosmanoglu` / `abdurrahman.karaosmanoglu` (ADR-011); bind DN şablonu bu durumda yanlış hesabı arar. Kısaltma üretebilen şablonlarda ön koşul bu yüzden **`ldap` mekanizması** ve `mail` üzerinden arama filtresidir; AD `mail` özniteliği varsayılan eşlemede zaten yazılır. Hesap başına `zimbraAuthLdapExternalDn` ikisini de ezer; bazı kurumlar bağlamayı böyle yapar. OpenSicil bu özniteliği v1'de yazmaz; öyle çalışan bir kurum çıkarsa motorun bağlı AD hesabından okuyup yazdığı bir öznitelik olarak eklenir (eşleme satırıyla asla: hesabın doğrulamasını başka bir AD hesabına bağlamak mailbox'ı devretmektir). Şablonu kısaltma üretmeyen ve mevcut bind şablonuyla çalışan kurum hiçbir şeyi değiştirmek zorunda değildir.

## Sürüm ve edisyon

- Zimbra'nın resmi açık kaynak kurulum paketleri **8.8.15'te bitti** (teknik rehberlik sonu 2024-12-31); Zimbra 9'un ikilileri yalnızca Network Edition lisansıyla verildi. 9.0 ve 10.x açık kaynak olarak **sadece kaynak kod**dur ve `zm-build` ile derlenir (son: 10.1.20, 2026-07-20; 10.0 hattının genel desteği 2025-06-30'da bitti). Hazır 10.x paketleri Zimbra'nın desteklemediği üçüncü taraf derlemelerdir.
- OpenSicil belirli bir sürüme değil, **Admin SOAP API işlemlerine** bağlıdır. Kullanılan işlemler 10.1 API referansında mevcut.
- Lab seçenekleri bu yüzden üçtür: kaynaktan derlenmiş 10.1, üçüncü taraf 10.1 derlemesi, ya da Carbonio CE. Kurulumda kararlaştırılır ([docs/08](08-gereksinimler.md)).
- **Carbonio CE (Zextras) aynı Admin SOAP API'sini korur:** resmi API referansı `CreateAccount`'u `urn:zimbraAdmin` ad alanında listeler, kaynak kodu `/service/admin/soap/` ucunu ve 7071 portunu taşır. v1'de doğrulanmış hedef değildir; aynı connector'la desteklenmesi v1.x adayıdır (F-41, ADR-057). Paketli ve bakımı süren tek açık kaynak dal bu olduğu için Zimbra OSE kullanan kurumların göç yönü burasıdır.

## API

- Uç: `https://<zimbra>:7071/service/admin/soap`.
- **Biçim: JSON.** Zimbra SOAP istekleri XML veya JSON olarak gönderilebilir. Kullanıcı ve admin API'si aynı dağıtıcıdan geçer; `<` ile başlamayan her gövdeyi JSON olarak işler. Bu sayede Rust tarafında XML kütüphanesi gerekmez, istekler serileştiriciyle kurulur. JSON kuralları: `Envelope` nesnesi yoktur, ad alanı `_jsns` alanında, eleman metni `_content` alanında verilir; XML'de öznitelik olan değerler (örneğin `CreateAccountRequest`'in `password`'ü) istek nesnesinin doğrudan alanıdır. Yanıt, isteğin biçimindedir.
- **Oturum:** Admin kimlik bilgisiyle alınan kısa ömürlü token. Token log'a yazılmaz; süresi dolunca yenilenir.

| İş | Admin API işlemi |
|---|---|
| Hesap aç | `CreateAccountRequest`, **parolasız** (`password` isteğe bağlıdır; API referansı: "accounts without passwords can't be logged into" — yerel doğrulama için geçerlidir, AD doğrulaması çalışır); yanıttaki `zimbraId` başka bir işlemden önce kaydedilir |
| Öznitelik, COS ve durum değiştir | `ModifyAccountRequest` |
| Hesap sil | `DeleteAccountRequest` |
| Listeye ekle / listeden çıkar | `AddDistributionListMemberRequest` / `RemoveDistributionListMemberRequest` |
| Gerçek liste üyeliği | `GetAccountMembershipRequest`; doğrudan ve dolaylı üyelikleri birlikte döndürür, **doğrudan üyelik `via` özniteliği olmayan kayıttır**; her kayıtta `dynamic` bayrağı vardır |
| Okuma, arama, katalog | Get ve Search işlemleri (lab'da netleşecek). Üyeleri özel bir `memberURL` ile tanımlanan dinamik gruplar kataloğa alınmaz: `AddDistributionListMemberRequest` onlarda "cannot add members to dynamic group with custom memberURL" hatası verir |

## Hesap durumları

| Zimbra durumu | Giriş | Gelen posta |
|---|---|---|
| `active` | Açık | Teslim edilir |
| `locked` | Kapalı | Teslim edilir (LMTP 250); `zimbraMailStatus` `enabled` kalır, yönlendirmeler, filtreler ve otomatik yanıt çalışmaya devam eder |
| `closed` | Kapalı | Geri döner (LMTP 550 5.1.1). Zimbra ayrıca `zimbraMailStatus = disabled` yapar ve hesabın bütün adreslerini **bütün dağıtım listelerinden kendisi çıkarır** |
| `maintenance` | Kapalı | MTA'da bekletilir (LMTP 450 4.2.1) |
| `pending` | Kapalı | Geri döner; `zimbraMailStatus = disabled` |
| `lockout` | Kapalı | Yanlış parola denemelerinden sonra **otomatik** gelir; posta teslim edilir; admin kaldırabilir |

**Açık oturumlar:** Durum `active` dışına çıkınca mevcut oturum jetonu bir sonraki SOAP/REST isteğinde `AUTH_EXPIRED("account not active")` ile reddedilir; IMAP her komuttan önce durumu yeniden kontrol eder ve bağlantıyı düşürür. İstisna: komut göndermeden `IDLE`'da bekleyen IMAP bağlantısı bir sonraki komuta kadar açık kalır. İki kontrol de mailbox sunucusunun önbellekteki hesap nesnesini okur; çok sunuculu kurulumda gecikme önbellek süresi kadardır (lab'da ölçülür).

### OpenSicil durumlarının karşılığı

| OpenSicil | Zimbra | Not |
|---|---|---|
| bekliyor | `locked` | Başlangıçtan önce gelen posta kaybolmaz |
| aktif | `active` | |
| askıda | `locked` | |
| ayrıldı | `locked` (varsayılan) veya `closed` | Kurulum ayarı. `locked`ta posta gelmeye devam eder; motor 24 saat sonra otomatik yanıt yazar, ayar açıksa devir yöneticisinin bağlı adresine yönlendirir (ADR-045, ADR-049); `closed` gönderene hata döndürür, yönlendirme yazılmaz. İki durumda da kullanıcının kendi yönlendirmesi ve filtresi ayrılış anında temizlenir |
| silindi | hesap silinir | Varsayılan olarak otomatik silinmez; "silinmeyi bekliyor" listesinden onayla silinir, arşivleme onaydan önce kurumun işidir (ADR-024) |

**`lockout` sapma sayılmaz.** Olması gereken durum `active` iken hesap `lockout` durumundaysa OpenSicil dokunmaz. AD'deki kilitlenme kuralıyla aynı gerekçe: kaba kuvvet korumasını ortadan kaldırmamak.

Elle verilmiş `locked`, `closed` veya `maintenance` da geri alınmaz: hesap yalnızca kimlik durumu geçişinde `active` yapılır (ADR-032).

## Hesap içeriği

- **Adres:** ADR-011 ile üretilen e-posta adresi. Alan adı departman zincirinden gelir ve yönetilen alan adlarından biri olmalıdır (ADR-017). Sahiplenmede adres ipucuyla aranır, `zimbraId` gözlem modunda bağlanır (ADR-018). Çakışma kontrolünde hesaplar, takma adlar ve dağıtım listeleri aranır; Zimbra erişilemiyorsa kontrol atlanır, AD hesabı beklemez ve dolu adres `CreateAccountRequest`'te `account.ACCOUNT_EXISTS` ile müdahaleye düşer (ADR-058).
- **Parola:** Yazılmaz; hesap parolasız açılır ve giriş AD'de doğrulanır. Yerel `userPassword`'ü olmayan hesapta yerel doğrulama `missing userPassword` ile başarısız olur; geri düşme açılmış olsa bile kullanılabilecek bir parola yoktur (ADR-057).
- **COS:** Tek değerli ayar; birincil rol → departman → hedef sistem varsayılanı (ADR-007).
- **Öznitelikler:** Varsayılan eşleme `displayName`, `givenName` ve `sn`'dir. Diğerleri eşleme ayarıyla eklenir (ADR-012). Hedef öznitelik kodda sabit izinli listeden seçilir; `zimbraAccountStatus`, `zimbraCOSId`, `zimbraIsAdminAccount`, `zimbraMailForwardingAddress` gibi motorun yazdığı ya da yetki taşıyan öznitelikler eşlenemez (ADR-029).
- **Dağıtım listeleri:** Katalogdan; sadece yönetilen domain'lerdeki listeler.
- **Ayrılışta, hemen:** kullanıcının kendi yönlendirmesi (`zimbraPrefMailForwardingAddress`) ve filtre betiği (`zimbraMailSieveScript`) temizlenir; eski değerler bağlantıda saklanır, geri almada geri yazılır. Zimbra'da iki ayrı yönlendirme özniteliği vardır: yöneticinin yazdığı, kullanıcıdan gizli `zimbraMailForwardingAddress` ve kullanıcının kendi ayarı `zimbraPrefMailForwardingAddress`. MTA teslimatta hesap durumuna değil `zimbraMailStatus`'a baktığı için `locked` hesapta ikisi de çalışmaya devam eder; temizlenmezse ayrılan kişi kurumsal postayı kişisel adresinde almaya devam eder (ADR-049).
- **Ayrılıştan 24 saat sonra (acil ayrılışta hemen):** `zimbraPrefOutOfOfficeReplyEnabled` / `zimbraPrefOutOfOfficeReply` (kurulum şablonu; kullanıcının eski yanıtı bağlantıda saklanır) ve `zimbraHideInGal` yazılır. Yönlendirme ayarı açıksa (varsayılan **kapalı**) `zimbraMailForwardingAddress` yazılır: yalnızca yönetilen alan adındaki **bağlı** bir hesabın adresi; motor bağlantıdan çözer, serbest metin asla. Hepsi geri almada kaldırılır. Bu öznitelikler eşlenemez (ADR-029, ADR-045). Kaynak kodundan doğrulananlar: otomatik yanıt teslimat anında gönderilir ve hesap durumuna bakmaz; tarih verilmezse her zaman geçerlidir; `zimbraPrefOutOfOfficeSuppressExternalReply` boşken **dış gönderenlere de** gider; aynı gönderene 7 günde bir gönderilir (`zimbraPrefOutOfOfficeCacheDuration`). Yanlışlıkla giden bir "artık çalışmıyor" yanıtı bu yüzden geri alınamaz; 24 saatlik gecikmenin nedeni budur. Yanıtın `locked` hesaptan MTA'dan gerçekten çıktığı lab'da gözlenir ([docs/08](08-gereksinimler.md)).
- **Kullanılmamış hesap (ADR-048):** `zimbraLastLogonTimestamp` hesap açılırken yazılmaz, AD doğrulaması dahil her başarılı girişte yazılır; hiç giriş yapılmamış hesapta yoktur. Sonraki güncellemeler en fazla 7 günde birdir (`zimbraLastLogonTimestampFrequency`), bu "hiç girilmedi" sorusunu etkilemez. Kurum geçici öznitelik deposunu LDAP dışına taşıdıysa (`zimbraEphemeralBackendURL`) değer LDAP'ta aranamaz.

## Admin hesabı

- Yetki devri (sadece belirli domain üzerinde yetkili admin) **resmi olarak Network Edition özelliğidir**.
- Topluluk kaynaklarında, `domainAdminRights` yetkisiyle oluşturulan domain adminlerinin OSE 8.x'te çalıştığı raporlanıyor. Bu yol resmi olarak desteklenmiyor.
- **OpenSicil'in admin hesabı yönetilen alan adında durmaz.** 9.0.0 P40, 10.0.8 ve sonrasında admin hesapları da `zimbraAuthFallbackToLocal`'a uyar; AD doğrulamalı bir alan adındaki admin hesabı artık yerel parolasıyla giremez (eski sürümlerde girebiliyordu). Hesap, yerel doğrulamalı ayrı bir alan adında açılır; ya da yönetilen alan adında yönetici bağlamı için `zimbraAuthMechAdmin = zimbra` verilir.
- v1 kararı: Varsayılan olarak **OpenSicil'e özel bir global admin hesabı** kullanılır. Riski sınırlayan şeyler şunlardır: port kısıtı, yönetilen domain kapsamının worker'da uygulanması ve hesabın başka hiçbir işte kullanılmaması. Domain admin ile çalışma, lab'da seçilen sürümde doğrulanırsa desteklenen bir seçenek olarak eklenir.

## Bilerek kullanılmayan yollar

| Yol | Neden kullanılmıyor |
|---|---|
| **Zimbra auto-provisioning** (EAGER, LAZY, MANUAL; 8.0'dan beri) | Hesabı AD'den kendisi açar ama **hiçbir modu hesabı kapatmaz veya silmez**. COS, liste ve durum yönetimi de yapmaz. OpenSicil'le birlikte açık kalırsa aynı kişi için iki ayrı hesap açan olur |
| **Zimbra'nın iç LDAP'ına doğrudan yazmak** | midPoint'in Zimbra rehberi bu yolu kullanıyor. Resmi Admin API'yi atladığı için OpenSicil kullanmaz |
| **`zmprov` komutunu SSH ile çalıştırmak** | Sunucuya kabuk erişimi, metin çıktısı ayrıştırma ve komut enjeksiyonu riski |
| **ConnId Zimbra Bundle** | Java ve neredeyse bakımsız ([docs/01](01-mevcut-cozumler.md)) |

## Kaynaklar

- Portlar: https://wiki.zimbra.com/wiki/Ports
- Admin API referansı (10.1): https://files.zimbra.com/docs/soap_api/10.1.0/api-reference/zimbraAdmin/service-summary.html
- JSON desteği: https://github.com/Zimbra/zm-mailbox/blob/develop/store/docs/soap.txt · https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/soap/SoapEngine.java
- Hesap durumları: https://github.com/Zimbra/adminguide/blob/develop/managingaccounts.adoc · durum değişince `zimbraMailStatus`: https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/account/callback/AccountStatus.java · LMTP yanıtları: https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/lmtpserver/ZimbraLmtpBackend.java
- Açık oturumların durumu kontrol etmesi: https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/service/AuthProvider.java · https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/imap/ImapHandler.java
- Geri düşme, parola değiştirme, arama filtresinin `ad` mekanizmasında yok sayılması, yer tutucular: https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/account/ldap/LdapProvisioning.java · https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/ldap/LdapUtil.java · admin hesaplarının geri düşmeye uyması: https://wiki.zimbra.com/wiki/Zimbra_Releases/10.0.8 · COS'ta parola değiştirmenin kapatılması: https://github.com/Zimbra/adminguide/blob/develop/cos.adoc
- Yönlendirmenin Postfix LDAP eşlemesinde ve `zimbraMailStatus` üzerinden yapılması: https://github.com/Zimbra/zm-core-utils/blob/develop/src/libexec/zmmtainit
- Filtreler ve otomatik yanıt: https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/filter/ZimbraMailAdapter.java · https://github.com/Zimbra/zm-mailbox/blob/develop/store/src/java/com/zimbra/cs/mailbox/Notification.java
- `CreateAccountRequest` (parola isteğe bağlı): https://files.zimbra.com/docs/soap_api/10.1.0/api-reference/zimbraAdmin/CreateAccount.html
- Dinamik gruplar ve üyelik: https://github.com/Zimbra/zm-mailbox/blob/develop/store/docs/soap-admin.txt
- Carbonio CE Admin SOAP API: https://docs.zextras.com/apidoc/api-reference/zimbraAdmin/CreateAccount.html · https://github.com/zextras/carbonio-mailbox
- Sürümler: https://wiki.zimbra.com/wiki/Zimbra_Releases
- Harici AD doğrulaması: https://github.com/Zimbra/zm-mailbox/blob/develop/store/conf/attrs/zimbra-attrs.xml · https://github.com/Zimbra/adminguide/blob/develop/ldap.adoc
- Auto-provisioning tasarım dokümanı (ayna): https://github.com/Grynn/zimbra-mirror/blob/master/ZimbraServer/docs/autoprov.txt
- Yetki devri (topluluk): https://forums.zimbra.org/viewtopic.php?t=16355 · https://imanudin.net/2021/01/07/how-to-create-admin-delegation-in-zimbra-ose/
- OSE sürüm durumu: https://blog.zimbra.com/2020/05/is-zimbra-open-source-yes-faqs-about-zimbra-ose-for-you/ · https://wiki.zimbra.com/wiki/Zimbra_Foss_Source_Code_Only_Releases
