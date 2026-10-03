# 05 — Active Directory Connector'ı

Windows Server AD ve Samba AD için geçerlidir. Farklılıklar lab'da ölçülecek ([docs/08](08-gereksinimler.md#-labda-ölçülecek)).

## Ön koşullar (kurumun işi)

| Ön koşul | Neden |
|---|---|
| Domain controller'da **LDAPS (636)** ve worker'ın güvendiği bir CA sertifikası | Parola yalnızca şifreli bağlantıda yazılabilir; OpenSicil düz LDAP konuşmaz (N-10) |
| OpenSicil'e özel bir **servis hesabı** ve aşağıdaki delegasyonlar | En az yetki. Parolası süresiz ("never expires") ya da yazılı rotasyon adımı: `.env` güncellenir, worker yeniden başlar. Süresi dolan parola ya da CA sertifikası worker'ı **sessizce** durdurur; metrik ucundaki "hedef sistem başına son başarılı bağlantı" değerine alarm kurulur ([docs/02](02-mimari.md#ölçek-ve-dağıtım)) |
| **Yönetilen kullanıcı OU'ları**, isteğe bağlı bir **pasif OU**, **yönetilen grup OU'ları** | Worker bu kapsamın dışına yazmaz ([ADR-014](decisions/014-yonetim-kapsami-ve-toplu-degisiklik-freni.md)) |
| Gruplar bir **OU** içinde | `CN=Users`, `CN=Builtin` ve `OU=Domain Controllers` yönetilen kapsam olamaz ([Yasaklı gruplar](#yasaklı-gruplar)). Gruplar `CN=Users` içindeyse önce bir OU'ya taşınır |
| **Break-glass yönetici:** en az bir `OpenSicil-Admins` üyesi yönetilen kapsam dışında | Yanlış bir ayrılış veya askı yöneticilerin OIDC girişini kilitlemesin ([docs/07](07-guvenlik-ve-kvkk.md#yönetici-hesapları)) |
| **AD Recycle Bin** açık (önerilir) | Yanlış silinen bir hesap geri alınabilir |

## Bağlantı

- Rust `ldap3` crate'i (0.12) ile yalnızca LDAPS kullanılır. Sertifika doğrulaması kapatılamaz (`set_no_tls_verify` hiç çağrılmaz; varsayılanı zaten `false`); CA sertifikası kuruluma dosya olarak verilir ve `set_connector` / `set_config` ile yüklenir. Crate gereken bütün işlemleri destekler ([docs/11](11-dogrulama-notlari.md)); üç uygulama kuralı vardır: her arama `EntriesOnly` adaptöründen geçer (domain kökünden yapılan ad çakışması araması yönlendirme döndürür, crate'in açık hatası #156 bunda panikler); `objectGUID` hem `bin_attrs` hem `attrs` haritasında aranır; `dn_escape` bütün DN'e değil tek RDN değerine uygulanır.
- Servis hesabıyla TLS üzerinden simple bind yapılır. DC'de "LDAP imzalama zorunlu" ve `LdapEnforceChannelBinding = 2` olsa da çalışır: Microsoft'un ifadesiyle TLS kanalı imzalama şartını karşılar, channel binding ayarının "TLS üstünde simple bind" oturumuna etkisi yoktur (yalnızca TLS üstündeki SASL bind'ı etkiler); imzalama zorunluluğu düz 389 üstündeki simple bind'ı reddeder, OpenSicil onu zaten konuşmaz.
- DC adresi **sıralı** bir listedir: ilk adres tercihli DC'dir (öneri: PDC emulator ya da worker'a en yakın site'taki DC), sonrakiler yedektir. Bir iş baştan sona **tek DC'ye, tek bağlantıyla** konuşur; hesabı bir DC'de açıp gruba başka bir DC'de eklemek replikasyon gecikmesinde "nesne yok" hatası üretir. Yedek DC'ye yalnızca işler arasında ve yalnızca tercihli DC'ye bağlanılamadığında geçilir; tur atılmaz ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)). Aksi halde aynı kimliğin art arda iki işi farklı DC'lerden bayat okur ve pasifleştirme gibi işlemleri denetim kaydına iki kez yazar.
- Yönetilen OU'lar ortam değişkeninde DN olarak verilir; worker başlangıçta her DN'i objectGUID'e çözer, çözemezse AD connector'ı başlamaz, çalışırken GUID'i kullanır. DC'ye o an erişilemiyorsa süreç çıkmaz: connector bekler ve bu kontrolleri bağlantı gelince yapar; Zimbra işleri sürer ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)). OU yeniden adlandırılsa kapsam bozulmaz. Aynı açılış kontrolünde domain kökünün `msDS-LogonTimeSyncInterval` değeri okunur; `0` ise (`lastLogonTimestamp` kapatılmış) connector başlamaz ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)).
- Tüm aramalar sayfalıdır; AD tek aramada varsayılan olarak en fazla 1.000 nesne döndürür.
- Nesnelere mümkün olan her yerde **objectGUID ile** erişilir. DN her işlemden önce GUID'den yeniden çözülür, çünkü taşınan veya yeniden adlandırılan bir nesnenin DN'i değişir. AD `<GUID=…>` biçimini arama tabanı olarak kabul eder; iki yazımı vardır ve **bayt sırası farklıdır**: onaltılık yazım `objectGUID`'in ham bayt sırasıdır (ilk üç alan little-endian), tireli yazım RFC 4122 metin biçimidir. Worker yalnızca tireli yazımı kullanır; ham baytı metne çeviren tek bir yardımcı fonksiyon ve onun testi vardır. MS-ADTS bu biçimi modify, modifyDN ve delete hedefi olarak açıkça saymaz; bu yüzden yazma işlemleri GUID ile aranıp bulunan **gerçek DN** üzerinden yapılır.

## İşlemler

### Hesap açma
Kullanıcı adı ve e-posta AD'ye yazılmadan önce veritabanına kaydedilir; yeniden denemede yeniden üretilmez. Üretilen taban ad AD'de OpenSicil'e bağlı olmayan bir hesapta varsa `n + 1` denenmez, iş "müdahale gerekiyor" durumuna düşer ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md)). Hesap açma saatlik verme sayacına tabidir ([ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)).

**Hedef akış tek bir `add`'dir** ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)): eşlenmiş öznitelikler, `unicodePwd`, `userAccountControl = 514` (normal hesap + pasif), `pwdLastSet = 0` ve biliniyorsa `accountExpires` aynı istekte gönderilir; ardından objectGUID okunup kaydedilir ve grup üyelikleri eklenir. MS-ADTS bunu `user` sınıfı için yasaklamaz, Samba kaynak kodu kabul eder; Faz 1c iki dizinde de çalıştırarak doğrular. AD parolayı reddederse hesap **hiç oluşmaz**; yarım hesap kalmaz. Aşağıdaki dört adım, tek `add` bir dizinde reddedilirse geri dönülecek yoldur; bilinen kusuru, hesap "parola gerekmez" bayrağıyla açıldığı için AD'nin 3. adımda parola politikasını **hiç uygulamamasıdır**.

1. Kullanıcı nesnesi, eşlenmiş özniteliklerle yönetilen OU'da oluşturulur. `userAccountControl` verilmez. AD bu durumda hesabı **pasif ve "parola gerekmez" bayrağıyla** oluşturur (`0x222`).
2. objectGUID **hemen** okunup OpenSicil'e kaydedilir. Sonraki adımlar yarıda kalırsa yeniden deneme hesabı bu bağlantıyla bulur; "parola gerekmez" bayrağı hâlâ duruyorsa 3. ve 4. adımlar tekrarlanır.
3. Parola ayrı bir değişiklik işlemiyle yazılır. AD karmaşıklık kuralı parolada `sAMAccountName`'i ve görünen adın üç harften uzun parçalarını yasaklar; rastgele parola "ali" ya da "can" içerebilir. Kısıt ihlalinde worker yeni parola üretip en fazla 3 kez yeniden dener ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)). Değer tırnak içindeki parolanın UTF-16LE kodlamasıdır ve bağlantı şifreli olmalıdır. Bu adımda yazılan parola kimseye gösterilmez ve ardından `pwdLastSet = 0` yapılır ([ADR-009](decisions/009-parola-yonetimi.md)).
4. `userAccountControl` "normal hesap + pasif" yapılır. **"Parola gerekmez" bayrağı kaldırılır**; bu bayrak kalırsa hesap boş parolayla kalabilir.
5. Bitiş tarihi biliniyorsa `accountExpires` yazılır.
6. Grup üyelikleri eklenir.

Worker hesabı oluşturan istek ile objectGUID'in kaydı arasında çökerse (tek `add`'de de, dört adımda da) hesap bağlantısız kalır. Yeniden deneme ad çakışmasıyla "müdahale gerekiyor" durumuna düşer ve hesap mutabakatta "yönetilmeyen hesap" olarak görünür. Operatör hesabı AD'den silip işi tekrar dener.

### CN kuralı
CN varsayılan olarak `{given} {surname}` olur. Hedef OU'da aynı CN varsa `{given} {surname} ({kullanıcı adı})` kullanılır. `cn` şemada **64 karakterle** sınırlıdır (`sAMAccountName` 20; onu [ADR-011](decisions/011-kullanici-adi-ve-eposta.md) üretimde kısaltır): sonuç 64'ü aşarsa ad-soyad kısmı kesilir, parantez içindeki kullanıcı adı korunur; kesilmezse AD `add`'i kısıt ihlaliyle reddeder ([docs/11](11-dogrulama-notlari.md) A18). Ad veya soyad değişince CN aynı kuralla modifyDN ile yeniden adlandırılır; `cn` öznitelik eşlemesiyle yazılmaz ([ADR-012](decisions/012-oznitelik-esleme.md)). DN her zaman kaçış yapan yardımcı fonksiyonla kurulur ([docs/07](07-guvenlik-ve-kvkk.md#enjeksiyon)).

### Etkinleştirme ve pasifleştirme
- `userAccountControl` içindeki pasif bayrağı açılır veya kapatılır. Etkinleştirme yalnızca kimlik durumu geçişinde yapılır; elle pasifleştirilmiş hesap, kimlik durumu değişmeden açılan işlerde pasif kalır ve mutabakata düşer ([ADR-032](decisions/032-elle-pasiflestirme-korunur.md)). Pasifleştirme yönünde istisna yoktur.
- `accountExpires` olması gereken durumun parçasıdır: bitiş tarihi değişince yeniden yazılır, kaldırılınca süresiz (`0`) yazılır; stajyerin kadroya geçişinde eski staj bitişi hesapta kalmaz ([ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)).
- Hesap kilitlenmesi (`lockoutTime`, yanlış parola denemeleri) ayrı bir durumdur. **OpenSicil kilitli hesabı açmaz ve bunu sapma saymaz.** Aksi halde kaba kuvvet korumasını ortadan kaldırırdı.

### İlk parola
Worker önce `pwdLastSet` ve `lastLogonTimestamp` okur. Hesap yalnızca **hiç kullanılmamışsa** ilk parola alır: `pwdLastSet` kontrolü tutuyor (açık modda 0, kapalı modda worker'ın yazdığı değer — [ADR-019](decisions/019-ilk-parola-teslimi.md)) **ve** `lastLogonTimestamp` boş. `lastLogonTimestamp` ilk girişte yazılır ve replikasyona girer; yardım masasının AD'de "sonraki girişte değiştir" işaretlemesi (`pwdLastSet = 0`) kullanımdaki hesabı OpenSicil için "kullanılmamış" yapamaz. İkinci yol, ayrılışı geri alınan kimliktir: bağlantıda worker'ın "ayrılışta parola sıfırlandı" işareti varsa ve `pwdLastSet` kontrolü tutuyorsa ilk parola verilir ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)). Başka her durumda iş reddedilir ([ADR-009](decisions/009-parola-yonetimi.md)). Teslim edilen parola okunabilir biçimdedir: 16 karakter, tireyle ayrılmış dört dörtlü, karışan karakterler (`0 O o 1 l I`) olmadan; İK'nın sesli okuduğu ya da kâğıda yazdığı parola ilk denemede doğru yazılabilmelidir ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)). `unicodePwd` yönetici sıfırlaması olarak tek bir replace işlemiyle yazılır. "İlk girişte değiştir" ayarı açıksa (varsayılan) ardından `pwdLastSet = 0` yapılır ve kullanıcı ilk girişte parolasını değiştirmek zorundadır; kapalıysa AD'nin yazdığı `pwdLastSet` okunup bağlantıya kaydedilir ([ADR-019](decisions/019-ilk-parola-teslimi.md)).

`lastLogonTimestamp` iki varsayıma dayanır ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)). Birincisi, domain kökündeki `msDS-LogonTimeSyncInterval` `0` ise öznitelik **hiç yazılmaz** ve her hesap kullanılmamış görünür; worker bu değeri açılışta okur ve `0` ise AD connector'ı başlamaz. İkincisi, öznitelik replike olur ama acil replikasyona girmez: başka site'taki DC'de yapılan ilk giriş tercihli DC'de site'lar arası replikasyon süresi kadar görünmez. "İlk girişte değiştir" açıkken bu pencereyi `pwdLastSet` kapatır (parola değişimi PDC emulator'a hemen iletilir); ayar kapalıyken pencere kabul edilmiş bir sınırdır ([docs/09](09-kurulum.md#active-directory)).

### Grup üyeliği
`memberOf` kullanıcı nesnesine **yazılamaz**. Sistem tarafından hesaplanan bir geri bağlantıdır ve grupların `member` özniteliğinden türetilir. Üyelik değiştirmek için **grubun** `member` özniteliğine kullanıcının DN'i eklenir veya çıkarılır. Bu yüzden servis hesabının yetkisi grup nesnesi üzerinde olmalıdır.

- Gerçek üyelik kullanıcının `memberOf` özniteliğinden okunur. Grubun `member` özniteliği okunmaz: 1.500'den fazla üyesi olan grupta tek okumada gelmez, aralıklı okuma gerekir. `memberOf` birincil grubu içermez; birincil grup yönetilmez.
- Sadece **doğrudan** üyelik yönetilir. İç içe grupların çözümü AD'nin ve uygulamaların işidir.
- Sadece katalogdaki gruplar ve sadece bağlı hesaplar için işlem yapılır ([docs/03](03-rol-ve-veri-modeli.md#motor-neye-dokunur)).

### Sahiplenme
Worker'ın sahiplenme ayarı açıksa çalışır ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md)). Hesap ipucundaki `sAMAccountName` ile kaçışlı filtreyle aranır ve şu koşullarda bağlanır: DN yönetilen kullanıcı OU'larının altındadır; objectGUID başka bir kimliğe bağlı değildir; `adminCount` dolu değildir; hesap yasaklı grupların doğrudan veya iç içe üyesi değildir; sicil no eşlenmişse ve hedef öznitelik doluysa değerler eşittir (baştaki sıfırlar ve boşluklar atılarak). Hedefteki `givenName`/`sn` kimliğin ad-soyadıyla ADR-011 normalleştirmesiyle karşılaştırılır; uyuşmazlık red değil **uyarı**dır, bağlantıya bayrak olarak yazılır ([ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)). Bağlantı gözlem modunda yazılır; `sAMAccountName`, UPN ve `mail` okunup kimliğe kaydedilir ve bir daha yazılmaz ([ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)). Hesaba hiçbir şey yazılmaz. `adminCount` yapışkandır: yıllar önce ayrıcalıklı gruptan çıkarılmış bir hesap da reddedilir; ret nedeni bunu söyler, kurum özniteliği temizleyip izin mirasını açtıktan sonra yeniden dener.

### OU taşıma
Aynı RDN ile yeni üst nesneye taşınır (modifyDN). Hedef OU'da CN çakışırsa CN kuralı uygulanır.

### Öznitelik güncelleme
Eşlenmiş öznitelik replace ile yazılır. Kaynak boşsa öznitelik silinir ([ADR-012](decisions/012-oznitelik-esleme.md)). Referans kaynaklarda (`manager ← etkin yöneticinin hesabı`) "boş" ile "belirsiz" ayrılır: kimlikte yönetici **kayıtlı değilse** ya da kayıtlı yöneticisinin bu hedefte bağlı ve yönetilen hesabı yoksa öznitelik yazılmaz, temizlenmez, sapma sayılmaz; yalnızca kayıtlı yönetici ayrılmış **ve** devir yöneticisi de yoksa silinir ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md), [ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md), [ADR-128](decisions/128-yonetici-kayitli-degilse-hedefteki-deger-korunur.md)). Hedef öznitelik kodda sabit izinli listeden seçilir; `sAMAccountName`, `userPrincipalName` ve `cn` eşlenemez, ADR-012'nin varsayılan eşleme tablosu bu ikisi çıkarılarak okunur ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md), [ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)). Satırda "sadece boşsa yaz" açıksa hedefteki dolu değer korunur ([ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)).

### Silme
Saklama süresi dolunca hesap GUID'den çözülen DN ile silinir. Recycle Bin açıksa geri alınabilir.

### Kayıp hesap
Bağlı hesap GUID ile bulunamıyorsa (biri elle silmiş) motor o hedef için işlem üretmez ve yeniden açmaz; mutabakat "kayıp hesap" bulgusu verir. Kurum hesabı Recycle Bin'den geri alırsa GUID aynıdır ve bağlantı kendiliğinden canlanır; Sistem yöneticisi "bağlantıyı kopar ve yeniden aç" derse aynı kullanıcı adıyla yeni hesap açılır (yeni SID). OpenSicil'in kendi sildiği hesap bağlantıda "silindi" işaretlidir ve geri almada yeniden açılır ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md), [ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md)).

### Kapsam dışına taşınmış hesap
Bağlı hesap GUID ile bulunur ama DN'i yönetilen kullanıcı OU'larının altında değilse (biri elle taşımış) motor hiçbir işlem üretmez; kaynak konteynerde silme yetkisi yoktur ve kapsam kuralı geçerlidir ([ADR-014](decisions/014-yonetim-kapsami-ve-toplu-degisiklik-freni.md)). Mutabakat "kapsam dışı hesap" bulgusu üretir; operatör hesabı yönetilen bir OU'ya geri taşır, sonraki iş normal devam eder. İş başarılı bitmez, "müdahale gerekiyor"a düşer ve zamanlayıcı yenisini açmaz. Kimlik `ayrıldı` ise hesap **açık kalmıştır** (terfi edip BT OU'suna taşınan kişinin ayrılışı tipik örnektir): "ayrılmış ama kapatılamamış" metriği bunu sayar, alarm oraya kurulur ([ADR-052](decisions/052-uygulanamayan-fark.md)).

## Servis hesabı yetkileri

Delegasyon yönetilen OU'lara verilir, domain köküne verilmez.

| Nerede | Yetki | Neden |
|---|---|---|
| Yönetilen kullanıcı OU'ları (bu nesne) | Kullanıcı nesnesi oluştur / sil | Hesap açma ve silme; OU'lar arası taşıma (kaynakta sil, hedefte oluştur) |
| Yönetilen kullanıcı OU'larındaki kullanıcı nesneleri | Parolayı sıfırla (extended right) | İlk parola ve ayrılışta rastgele parola |
| Yönetilen kullanıcı OU'larındaki kullanıcı nesneleri | Eşlenen öznitelikleri ve `userAccountControl`, `pwdLastSet`, `accountExpires`, `name`/`cn` yaz | Hesap yönetimi |
| Yönetilen grup OU'larındaki grup nesneleri | `member` özniteliğini oku/yaz | Grup üyeliği |
| Domain | Okuma (varsayılan kimliği doğrulanmış kullanıcı izni) | Çakışma kontrolü, katalog |
| Confidential işaretli eşlenmiş öznitelikler | O öznitelik için okuma izni | Mutabakatta karşılaştırma ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md)) |

Ek sıkılaştırma: Etkileşimli giriş (yerel ve RDP) GPO ile reddedilir; hesap "hassas, devredilemez" işaretlenir.

Kesin delegasyon adımları kurulum dokümanında, lab'da çalıştırılarak yazılacak.

## Yasaklı gruplar

AD'nin **korunan hesap ve grupları** SDProp süreciyle PDC emulator üzerinde varsayılan olarak 60 dakikada bir AdminSDHolder'daki izinlere sıfırlanır. Bu yüzden bu gruplara verilen delegasyon kalıcı olmaz. OpenSicil buna güvenmez, bu grupları açıkça reddeder.

Gruplar **ada göre değil SID'e göre** tanınır; Türkçe Windows'ta grup adları farklıdır.

| Grup | SID / RID |
|---|---|
| Administrators | `S-1-5-32-544` |
| Account Operators | `S-1-5-32-548` |
| Server Operators | `S-1-5-32-549` |
| Print Operators | `S-1-5-32-550` |
| Backup Operators | `S-1-5-32-551` |
| Replicator | `S-1-5-32-552` |
| Domain Admins | domain RID 512 |
| Domain Controllers | domain RID 516 |
| Cert Publishers | domain RID 517 (korunan grup değil, `adminCount` taşımaz; yetki yükseltmeye açık) |
| Group Policy Creator Owners | domain RID 520 (aynı gerekçe) |
| Schema Admins | domain RID 518 |
| Enterprise Admins | domain RID 519 |
| Read-only Domain Controllers | domain RID 521 |
| Key Admins | domain RID 526 |
| Enterprise Key Admins | domain RID 527 |

Ayrıca `adminCount` özniteliği dolu olan her grup reddedilir; SDProp bu özniteliği izinlerini değiştirdiği nesnelere yazar. Öznitelik yapışkandır: gruptan çıkarılan nesnede kendiliğinden temizlenmez. SDProp'u beklememek için yukarıdaki grupların ve OpenSicil yönetim gruplarının iç içe üyesi olan gruplar da reddedilir. Worker bunları `LDAP_MATCHING_RULE_IN_CHAIN` (`memberOf:1.2.840.113556.1.4.1941:=<grup DN>`) ile tek aramada bulur. OpenSicil'in yönetim grupları da reddedilir ([ADR-005](decisions/005-yonetim-girisi-oidc.md)). Sorgunun sonucu worker belleğinde grup başına 5 dakika önbelleklenir; kontrol yine her işlemden önce yapılır ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)).

`CN=Users`, `CN=Builtin` ve `OU=Domain Controllers` yönetilen kapsam olarak verilemez; verilirse worker başlamaz. DnsAdmins gibi sabit RID'i olmayan, `adminCount` taşımayan ama yetki yükseltmeye açık gruplar varsayılan olarak bu konteynerlerde durur; SID ile tanınamazlar. Kurulum dokümanı bu yüzden `CN=Users`'tan yalnızca OpenSicil'in yöneteceği grupların taşınmasını ister ([docs/09](09-kurulum.md)).

## Hassas öznitelikler

Bir kurum kimlik numarası gibi hassas bir veriyi AD'ye yazmak isterse ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md)):

- Varsayılan kurulumda **`employeeNumber` ve `employeeID` özniteliklerini tüm kimliği doğrulanmış kullanıcılar okuyabilir.** İkisi de bir property set içinde değildir. Okuma izni, "Pre-Windows 2000 Compatible Access" grubunun tüm kullanıcılar üzerindeki okuma izninden gelir. Bu grup varsayılan olarak Authenticated Users veya Everyone içerir.
- Bir özniteliği **confidential** yapmak (`searchFlags` 0x80) okumayı ayrıca izin verilenlerle sınırlar. **Temel şema özniteliklerinde bu yapılamaz.** Microsoft'un kendi örneği `employeeID`'dir.
- `employeeNumber`, Windows Server 2003'ten beri temel şema özniteliği değildir, bu yüzden confidential yapılabilir.
- Pratik sonuç: Kimlik numarası AD'ye yazılacaksa **`employeeNumber` kullanılmalı ve confidential işaretlenmelidir.** `employeeID`'ye yazılmamalıdır. Şema değişikliği kurumun işidir; OpenSicil şemaya dokunmaz.

## Kerberos ve açık oturumlar

Hesabı pasifleştirmek o ana kadar verilmiş biletleri iptal etmez:

- Kullanıcı biletinin (TGT) varsayılan en uzun ömrü **10 saattir**.
- KDC, TGT'si **20 dakikadan eski** olan kullanıcı için yeni servis bileti isterken hesabın durumunu yeniden kontrol eder ve pasif hesaba yeni servis bileti vermez.
- Daha önce alınmış servis biletleri süreleri dolana kadar geçerli kalır. Oturum açık bir Windows bilgisayarda, zaten bağlanılmış kaynaklara bir süre daha erişilebilir.
- Pasifleştirme tek DC'ye yazılır ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)); diğer DC'lerdeki KDC'ler bunu replikasyonla öğrenir: aynı site'ta saniyeler, site'lar arası varsayılan zamanlamayla dakikalar ya da saatler.
- Domain'e bağlı bir bilgisayar çevrimdışıyken **önbelleğe alınmış kimlik doğrulayıcıyla** (varsayılan son 10 kullanıcı) açılır. Hesabı pasifleştirmek, `accountExpires` ve parolayı sıfırlamak bu önbelleği temizlemez; ayrılan kişi dizüstünü kuruma teslim etmediyse yerel dosyalara erişmeye devam eder. Cihazın geri alınması ya da uzaktan silinmesi kurumun işidir; acil ayrılış ekranı bunu söyler ([docs/04](04-yasam-dongusu.md#acil)).

Acil ayrılışta bu gecikme ekranda uyarı olarak gösterilir ([docs/04](04-yasam-dongusu.md#acil)). Cihazdaki oturumu kapatmak OpenSicil'in kapsamı dışındadır.

## Samba AD

Lab ortamı Samba AD'dir (OpenBerat ADR-0010 ile aynı yaklaşım). Kaynak koddan doğrulananlar:
- Samba da parola değişikliğini yalnızca şifreli LDAP bağlantısında kabul eder.
- `accountExpires` süresi dolmuş hesabı hem LDAP/NTLM doğrulamasında hem Kerberos'ta reddeder.

Samba SDProp'u uygulamaz: `adminCount` kendiliğinden yazılmaz (kaynak kodunda karşılığı yok); lab seed betiği bu özniteliği yasaklı grup ve üyesi için elle yazar ([ADR-027](decisions/027-test-stratejisi-ve-lab.md)).

Kaynak kodundan doğrulanan diğer davranışlar ([docs/11](11-dogrulama-notlari.md)):

| Konu | Samba | Sonucu |
|---|---|---|
| Tek `add`'de parola, UAC 514, `pwdLastSet = 0` | Kabul eder; istemcinin yazdığı 0 korunur | Hesap açma iki dizinde aynı |
| UAC verilmezse varsayılan | `0x222`, Windows ile aynı | |
| `pwdLastSet` | Yalnızca 0 ve -1; -1 "Unexpire-Password" hakkı ister | Worker -1 yazmaz |
| `lastLogonTimestamp` | 4.4.0'dan beri; simple bind dahil ilk girişte yazılır; `msDS-LogonTimeSyncInterval = 0` iken Windows gibi yazmaz | [ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md) ve açılış kontrolü ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)) lab'da sınanabilir. 2026-10-01: simple bind sonrası özniteliğin dolduğu lab testiyle doğrulandı (`issues_first_password_in_lab_only_to_unused_account`) |
| `LDAP_MATCHING_RULE_IN_CHAIN` | 4.4.0'dan beri; her adımda tam arama yapar; 4.18'de boş sonuç raporu var | Seçilen sürümde lab'da doğrulanır |
| `<GUID=…>` | Arama, modify, delete ve rename hedefi olarak çözülür | Worker yine gerçek DN ile yazar (Windows için belgeli değil) |
| Silinen hesabı geri alma | Recycle Bin **yok**; tombstone reanimation elle LDAP işlemidir. GUID ve SID korunur, `memberOf` ve parola silinir | "Geri alınırsa bağlantı canlanır" Samba'da da doğrudur; üyelikleri sonraki iş geri yazar, parola için ilk parola yolu gerekir |
| Parola karmaşıklık kuralı | Yalnızca karakter sınıfı sayar; **ad parçasına bakmaz** | "Reddedilen parolayı yeniden üret" ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)) Samba'da sınanamaz: üreteç birim testiyle ve bir kez Windows VM'de doğrulanır |
| 1.000 nesne ve 1.500 değer sınırı | Ayrıştırılır ama **uygulanmaz** | Sayfalı arama kodu Samba'da sınır görmeden geçer; sınır testi Windows VM ister |
| ModifyDN | Taşıma ve yeniden adlandırma tek işlemde; `deleteoldrdn = false` reddedilir | Worker her zaman `true` gönderir |

Lab imajı adayı: `quay.io/samba.org/samba-ad-server` (bakımı süren resmi proje); kararlı sürüm 4.24. Delegasyon ACL'leri lab'da ölçülecek. Windows Server'a özgü bir davranış gerekirse lab'a Windows Server eklenir.

## Hibrit (Entra Connect)

OpenSicil'in AD'de açtığı hesaplar Entra Connect ile buluta normal şekilde senkronlanır; OpenSicil bulut tarafına karışmaz. Entra Connect'in kendi kazara silme koruması (varsayılan 500 nesne) OpenSicil'in frenlerinden bağımsızdır. Pasif OU, Entra Connect'in eşitleme kapsamının **içinde** olmalıdır: kapsam dışına taşınan hesap bulutta silinir (30 gün geri alınabilir). Sahiplenilen hesabın UPN'i yeniden yazılmaz ([ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)); hibritte UPN değişimi bulut girişini değiştirirdi.

## Kaynaklar

- LDAPS, imzalama ve channel binding: https://learn.microsoft.com/en-us/troubleshoot/windows-server/active-directory/ldap-session-security-settings-requirements-adv190023
- `<GUID=…>` DN biçimleri: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-adts/92a54869-38d7-4e71-a3be-5f67a0dcdd7e
- Add işleminde izinli öznitelikler: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-adts/7dfeb38c-3cb9-4215-ae1c-ef209fd251ae · tek `add` ile parola ve varsayılan `0x222`: https://learn.microsoft.com/en-us/archive/blogs/fieldcoding/user-provisioning-and-password_not_required-set-why
- Parola karmaşıklık kuralı: https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/password-must-meet-complexity-requirements
- `lastLogonTimestamp`: https://learn.microsoft.com/en-us/archive/blogs/askds/the-lastlogontimestamp-attribute-what-it-was-designed-for-and-how-it-works
- Replikasyon aralıkları: https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/plan/determining-the-interval · parola değişiminin PDC'ye iletilmesi: https://learn.microsoft.com/en-us/troubleshoot/windows-server/active-directory/password-change-processing-conflict-resolution-function
- ModifyDN (aynı işlemde taşıma ve yeniden adlandırma): https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-adts/95fe5b94-859b-4001-a3e8-a98d6a417546
- Well-known RID'ler: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/81d92bba-d22b-4a8c-908a-554ab29148ab
- `unicodePwd`: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-adts/6e803168-f140-4d23-b2d3-c3a8ab5917d2 · https://learn.microsoft.com/en-us/troubleshoot/windows-server/active-directory/set-user-password-with-ldifde
- `memberOf` ve bağlı öznitelikler: https://learn.microsoft.com/en-us/windows/win32/adschema/a-memberof · https://learn.microsoft.com/en-us/windows/win32/ad/linked-attributes
- Confidential öznitelik: https://learn.microsoft.com/en-us/troubleshoot/windows-server/windows-security/mark-attribute-as-confidential
- `employeeNumber`: https://learn.microsoft.com/en-us/windows/win32/adschema/a-employeenumber
- Pre-Windows 2000 Compatible Access: https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/manage/understand-security-groups
- `sAMAccountName` 20 karakter: https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/plan/active-directory-domain-services-maximum-limits
- Kullanıcı oluşturma varsayılan bayrakları: https://learn.microsoft.com/en-us/windows/win32/ad/creating-a-user
- TGT ömrü: https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/maximum-lifetime-for-user-ticket
- KDC hesap kontrolü (20 dakika): https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-kile/519392b1-625a-420d-be90-d588c852dda3
- Korunan hesaplar ve gruplar: https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/plan/security-best-practices/appendix-c--protected-accounts-and-groups-in-active-directory · https://learn.microsoft.com/en-us/windows/win32/adschema/a-admincount
- Samba: https://github.com/samba-team/samba/blob/master/source4/dsdb/samdb/ldb_modules/password_hash.c · https://github.com/samba-team/samba/blob/master/source4/auth/sam.c · https://github.com/samba-team/samba/blob/master/source4/kdc/db-glue.c
- Entra Connect kazara silme koruması: https://learn.microsoft.com/en-us/entra/identity/hybrid/connect/how-to-connect-sync-feature-prevent-accidental-deletes
