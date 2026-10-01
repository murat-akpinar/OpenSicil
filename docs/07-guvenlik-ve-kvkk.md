# 07 — Güvenlik ve KVKK

OpenIAM'i ele geçiren biri istediği kişiye hesap açıp VPN yetkisi verebilir, herkesin hesabını kapatabilir ve kimlik numaralarını okuyabilir. Bu yüzden ürün, kurumun en değerli hedeflerinden biri gibi tasarlanır.

## Tehdit modeli

| Tehdit | Örnek | Kontrol |
|---|---|---|
| Backend'in ele geçirilmesi | Web açığıyla veritabanına yazma | AD/Zimbra sırları backend'de yok ([ADR-004](decisions/004-mimari-web-ve-worker.md), [ADR-006](decisions/006-sirlar-env.md)); kapsam ve saatlik frenler worker ortam değişkeninde ([ADR-014](decisions/014-yonetim-kapsami-ve-toplu-degisiklik-freni.md)); hesap bağlantısı ve kataloğa sadece worker'ın veritabanı rolü yazar ([ADR-015](decisions/015-veritabani-rolleri.md)); ilk parola sadece parolası henüz belirlenmemiş hesaba verilir ([ADR-009](decisions/009-parola-yonetimi.md)); eşlenebilir hedef öznitelikler kodda sabit, hassas kaynak eşlemesi worker ayarı ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md)); yayımlanmış tanıma doğrudan yazılan toplu yetki **verme** de saatlik verme sayacına takılır ([ADR-044](decisions/044-saatlik-yetki-ekleme-sayaci.md), [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)) |
| Yetki yükseltme | Bir role Domain Admins, OpenIAM yönetim grubu veya bunların iç içe üyesi olan bir grubu eklemek | Yasaklı gruplar (iç içe üyelik dahil); katalog zorunluluğu; kontrol her işlemden önce worker'da |
| Sahiplenme yoluyla mevcut hesaplara uzanma | Ele geçirilmiş backend'in mevcut bir personelin hesabını bağlatıp kapattırması | Sahiplenme worker ayarıdır ve varsayılan kapalıdır; kapsam, ayrıcalıklı hesap reddi ve sicil no kontrolü worker'da; bağlantı gözlem modunda başlar; yönetime alma frenlere tabidir; hesap devralınamaz (`pwdLastSet` ve `lastLogonTimestamp` kontrolü, [ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)) ve kayıt iptaliyle silinemez ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)) ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md)) |
| Hatalı içe aktarma | Yarım veya yanlış filtrelenmiş CSV | Dosyada olmayan kimliğe dokunulmaz; hatalı satır varsa dosya reddedilir; önizleme; tek değişiklik seti olarak eşiğe tabidir; ayrılmış kimliğin bitiş tarihini boşaltamaz ([ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)) |
| Hatalı operatör | Yanlış rol düzenlemesiyle 300 kişinin VPN'ini kesmek ya da 3.000 kişiye bir paylaşım vermek | Model farkından kesin etki önizlemesi; eşik ekleme işlemlerini de sayar ([ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md)); eşiği aşan düzenleme yayımlanmadan taslakta bekler, red modeli değiştirmez ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md)); saatlik sınır |
| Kayıt iptaliyle saklamasız silme | İK'nın yanlış tıkı ya da ele geçirilmiş backend, sahiplenilmiş on yıllık personelin kimliğini "iptal eder": AD hesabı ve mailbox onaysız silinir | İptal worker'da hedefe karşı doğrulanır: köken `açıldı` ve hesap kullanılmamış (`lastLogonTimestamp` boş); tutmazsa planlı ayrılış gibi uygulanır, silinmez ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)) |
| Ayrılanın kendi yönlendirmesi | Ayrılmadan önce postasını kişisel adresine yönlendiren ya da filtreyle kopyalayan kişi, `locked` hesaptan kurumsal postayı almaya devam eder | Ayrılış anında kullanıcı yönlendirmesi ve filtre betiği temizlenir, bağlantıda saklanır ([ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)) |
| Zimbra'da yerel parola | Geri düşme **açılmışsa** webmail'den parola değiştiren kullanıcı AD hesabı kapatıldıktan sonra da Zimbra'ya girer | Geri düşme varsayılan olarak kapalıdır; ön koşul `TRUE` yapılmamış olması ve COS'ta parola değiştirmenin kapalı olmasıdır. OpenIAM hesapları **parolasız** açar: yerel parolası olmayan hesapta geri düşecek bir şey yoktur ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md), [docs/06](06-zimbra.md#ön-koşullar-kurumun-işi)) |
| Ayrılmış operatörün açık oturumu | Acil ayrılışla çıkarılan İK operatörünün OpenIAM oturumu sürer | Backend **her istekte ve oturum açılışında** (okuma dahil: kişi sayfaları kişisel veridir) operatörle eşleşen kimliğin durumuna bakar; `ayrıldı` ya da `askıda` ise reddeder, oturumu sonlandırır, IdP oturumu sürse de yenisini açmaz; oturum en fazla 8 saat ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md), [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)) |
| Ayrılanın hesabının sessizce açık kalması | Yönetilen OU'dan taşınmış kişinin ayrılışı uygulanamaz; hedef kapalıyken işler tükenir | Uygulanamayan iş müdahaleye düşer; "ayrılmış ama kapatılamamış" metriği; bağlantı hatası deneme tüketmez ([ADR-052](decisions/052-uygulanamayan-fark.md)) |
| Hesap üretme | Ele geçirilmiş backend'in ya da hatalı CSV'nin yüzlerce hesap açtırması | Saatlik verme sayacı ([ADR-021](decisions/021-yeni-hesap-saatlik-siniri.md), [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)); bağlı olmayan hesapla ad çakışmasında durma ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md)); sahiplenme açıkken ipucusuz satırın reddi ([ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md)) |
| Hesap devralma: kullanımdaki hesaba ilk parola | Kişi parolasını değiştirmeden ele geçirilmiş backend'in ilk parolayı yeniden istemesi; yardım masasının AD'de "sonraki girişte değiştir" işaretlemesiyle `pwdLastSet`'in yeniden 0 olması | `pwdLastSet` yalnızca "parola değişti mi" der; ilk parola ayrıca `lastLogonTimestamp` boş olan (hiç giriş yapılmamış) hesaba verilir; özniteliğin domain'de kapatılmış olması (`msDS-LogonTimeSyncInterval = 0`) worker açılışında yakalanır ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)). Ayrılışı geri alınan kimlik için worker'ın "ayrılışta sıfırlandı" işareti gerekir ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)). Fren ayrıca saatlik ilk parola sayacıdır ([ADR-019](decisions/019-ilk-parola-teslimi.md)) |
| Tek yöneticide onay kilidi | Eşiği aşan değişiklik setini onaylayacak ikinci yönetici yok; fren kapatılır | Onay zaman kilidi: N saat sonra başlatan onaylar ([ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md)). Eşik ve onay operatör hatasına karşı backend kontrolleridir; ele geçirilmiş backend onayı uydurabilir, ona karşı fren worker'ın saatlik sayaçlarıdır ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md)) |
| Yöneticilerin kilitlenmesi | Yanlış ayrılış kaydı bütün `OpenIAM-Admins` üyelerini pasifleştirir | Break-glass: en az bir yönetici hesabı yönetilen kapsam dışında ([aşağıda](#yönetici-hesapları)) |
| Kötü niyetli operatör | Kendine rol atamak, kendi ayrılışını geri almak, toplu değişikliği kendisi onaylamak | Görev ayrılığı ve kendi kaydına işlem yasağı ([ADR-005](decisions/005-yonetim-girisi-oidc.md)); denetim kaydı |
| Veritabanı dökümü veya yedeğin sızması | Yedek diskin kaybolması | Kimlik numarası şifreli; düz metin parola hiç yok, ilk parola şifreli ve en fazla 10 dakika ([ADR-009](decisions/009-parola-yonetimi.md), [ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md), [ADR-036](decisions/036-ilk-parola-aead.md)) |
| Enjeksiyon | Adı `Yılmaz, Ahmet` veya `*)(uid=*` olan bir kayıt | LDAP DN ve filtre kaçışı; Zimbra isteklerinin serileştiriciyle kurulması (aşağıda) |
| Hesabın açık kalması | Ayrılan kişinin hesabının unutulması | Bitiş tarihi zorunluluğu (kadrolu dışı), `accountExpires`, mutabakat raporu; worker kapalı kalsa da kaçırılan geçişler yakalanır ([ADR-028](decisions/028-worker-zamanlamasi.md)) |
| Ayrılmışların diriltilmesi | Ele geçirilmiş backend'in ya da hatalı CSV'nin yüzlerce eski çalışanı geri alması | `ayrıldı → aktif` yıkıcı sayaca ve eşiğe tabi; CSV ayrılmış kimliğin bitiş tarihini boşaltamaz ([ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)); geri alınan hesaba ilk parola için worker'ın "ayrılışta sıfırlandı" işareti şarttır ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)) |
| Posta sızdırma | Eşleme satırıyla `zimbraMailForwardingAddress`'e dış adres yazmak | Öznitelik izinli liste dışıdır ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md)); ayrılış yönlendirmesini motor yalnızca yönetilen alan adındaki **bağlı** bir hesaba, bağlantıdan çözerek yazar ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md)) |
| Görev değişikliğiyle veri kaybı | "Hesap açılsın = hayır" ayarlı role geçen kişinin mailbox'ının silinmesi | Ayar yalnızca hesap yokken okunur; silme yalnızca yaşam döngüsünden doğar ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)) |
| Güvenlik ekibinin pasifleştirmesinin geri açılması | SOC'un kapattığı hesabı bir rol değişikliğinin açması | Etkinleştirme yalnızca durum geçişinde; elle pasif hesap korunur ve raporlanır ([ADR-032](decisions/032-elle-pasiflestirme-korunur.md)) |
| Adın başkasına geçmesi | Ayrılan kişinin e-postasının yeni gelen birine verilmesi | Adlar tekrar kullanılmaz ([ADR-011](decisions/011-kullanici-adi-ve-eposta.md)) |

## Parola

Özet ([ADR-009](decisions/009-parola-yonetimi.md)):
- Formda parola alanı yoktur; OpenIAM parola saklamaz.
- İlk parola worker'da üretilir, kimlik numarası AEAD anahtarıyla şifrelenir, operatöre bir kez gösterilir ve silinir; gösterilmeyen değer 10 dakika sonra silinir ([ADR-036](decisions/036-ilk-parola-aead.md)).
- Zimbra parolayı AD'de doğrular; tek parola AD'dedir.
- Ayrılıştan 7 gün sonra (acil ayrılışta hemen) parola kimsenin bilmediği bir değerle değiştirilir ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md)).
- İlk parolayı İK operatörü veya yardım masası verir; ilk girişte değiştirme işareti kurulum ayarıdır ve kapalıyken de kullanımdaki hesaba parola verilemez ([ADR-019](decisions/019-ilk-parola-teslimi.md)).
- İlk parola yalnızca hiç giriş yapılmamış hesaba (`lastLogonTimestamp` boş) ya da ayrılışta OpenIAM'in parolasını sıfırladığı hesaba verilir ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)).

## Kimlik numarası ve telefon

Özet ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md)):
- Kimlik numarası uygulama katmanında şifrelenir. Tekillik ve arama anahtarlı özet (blind index) ile yapılır.
- Ekranda maskeli görünür: `12*******34`. Açık hali sadece Kişisel veri okuyucu yetkisiyle görülür ve her görüntüleme kaydedilir.
- Log'a, hata mesajına, URL'e, kuyruğa ve denetim kaydındaki değer alanlarına hiç yazılmaz. Denetim kaydı sadece "kimlik numarası değişti" der.
- Hedef sisteme yazılması bir eşleme ayarıdır; varsayılan olarak hiçbir yere yazılmaz. AD'de yazıldığı özniteliği kimlerin okuyabildiği kurumun kontrol etmesi gereken bir konudur ([docs/05](05-active-directory.md#hassas-öznitelikler)).
- Telefon E.164 olarak saklanır, hedefe eşlemedeki biçimle yazılır.
- İçe aktarma dosyası saklanmaz: işlenir ve atılır. İçeriği log'a ve denetim kaydına yazılmaz ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md)). Eşiği aşan dosyanın doğrulanmış satırları onaya kadar taslak tablosunda durur ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md)); kimlik numarası orada da şifrelidir, onay ya da red sonrası satırlar silinir.

## Servis hesapları

### Active Directory
- OpenIAM'e özel, başka hiçbir işte kullanılmayan bir hesap.
- **Asla Domain Admin veya başka bir ayrıcalıklı grubun üyesi değil.**
- Yetkiler delegasyonla ve sadece yönetilen OU'larda verilir. Liste: [docs/05](05-active-directory.md#servis-hesabı-yetkileri).
- Etkileşimli girişi (yerel ve RDP) GPO ile kapalıdır. Kerberos delegasyonuna kapalıdır.
- Parolası uzun ve rastgeledir, `.env`'de durur ve sadece worker'a verilir.

### Zimbra
- OpenIAM'e özel bir admin hesabı. Domain bazında yetki devri resmi olarak sadece Network Edition'da olduğu için v1'de varsayılan olarak global admin kullanılır ([docs/06](06-zimbra.md#admin-hesabı)). Bu yüzden aşağıdaki port kısıtı ve worker'daki domain kapsamı kontrolü zorunludur.
- Admin portu (7071) güvenlik duvarında **sadece worker'ın çıktığı IP'ye** açıktır, internete asla.

### Yönetici hesapları
- Operatörlerin AD hesapları da OpenIAM'in kapsamındaysa yanlış bir ayrılış veya askı OIDC girişini keser ve düzeltecek kimse kalmaz. En az bir `OpenIAM-Admins` üyesi yönetilen kapsam **dışında** bir hesap olur (ya da IdP'de yerel bir hesap). Kurulum ön koşuludur ([docs/09](09-kurulum.md)).
- Tek Sistem yöneticisi olan kurumda onay zaman kilidi açılır ([ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md)).

## Ağ

| Bileşen | Gelen | Giden |
|---|---|---|
| nginx | 443 (dışarıya açık tek port) | backend |
| backend | nginx'ten | PostgreSQL; OIDC akışı backend'de yapılırsa IdP |
| worker | **Yok** | PostgreSQL, AD (636), Zimbra (7071) |
| PostgreSQL | backend, worker | Yok |

- AD bağlantısı sadece LDAPS'tir ve sertifika doğrulaması kapatılamaz; CA sertifikası kuruluma verilir.
- Zimbra bağlantısı HTTPS'tir ve sertifika doğrulanır.
- PostgreSQL başka bir sunucudaysa bağlantı `sslmode=verify-full` ile kurulur; `sqlx` varsayılanı `prefer` sessizce düz metne düşer ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)).
- Backend'in AD'ye ve Zimbra'ya ağ düzeyinde de ulaşamaması hedeflenir. Yöntemi (Docker iç ağı veya host güvenlik duvarı; Kubernetes'te çıkış yönlü NetworkPolicy — [docs/09](09-kurulum.md#kubernetese-özel-ön-koşullar)), OIDC akışının nerede yapılacağıyla birlikte kurulumda belirlenir. Çünkü backend OIDC'yi kendisi yaparsa IdP'ye çıkış yapması gerekir.

## Enjeksiyon

- **LDAP DN kaçışı (RFC 4514):** `cn=Yılmaz, Ahmet` kaçışsız yazılırsa DN yanlış ayrıştırılır. DN'ler string birleştirmeyle değil, kaçış yapan tek bir yardımcı fonksiyonla kurulur.
- **LDAP filtre kaçışı (RFC 4515):** Çakışma kontrolündeki aramalarda `*`, `(`, `)`, `\` ve NUL karakterleri kaçışlanır.
- Mümkün olan her yerde nesneye DN ile değil GUID ile erişilir.
- **Zimbra istekleri** serileştiriciyle kurulur, metin birleştirmeyle kurulmaz.
- Kullanıcı adı normalleştirmesi ([ADR-011](decisions/011-kullanici-adi-ve-eposta.md)) ek bir savunma katmanıdır, kaçışın yerine geçmez: görünen ad, departman adı ve unvan normalleştirilmez.
- Genel kurallar: `.claude/rules/security.md`.

## Denetim kaydı

**Ne yazılır:**
- Kim (OIDC `sub` ve kullanıcı adı), ne zaman, hangi işlem, hangi hedef, sonuç.
- Kimlik, rol, departman, katalog, eşleme ve ayar değişiklikleri: önce/sonra, hassas alanlar hariç.
- Hedef sistemde uygulanan her işlem: eklenen/kaldırılan grup, taşıma, pasifleştirme. Worker işlemden **önce** niyet satırını, sonra sonuç satırını yazar; niyet yazılamıyorsa işlem uygulanmaz. Sonucu olmayan niyet "sonucu bilinmiyor"dur ([ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md)).
- İlk parola verme (parola hariç), kimlik numarası görüntüleme, toplu değişiklik onayı veya reddi, fren devreye girmesi. Kullanılmış adın serbest bırakılması ve elle pasifleştirilmiş hesabın etkinleştirilmesi, gerekçesiyle.
- İçe aktarma (kim, ne zaman, kaç satır; içerik hariç), sahiplenme isteği ve sonucu, yönetime alma.

**Nasıl korunur:**
- Servislerin veritabanı rolleri denetim tablosuna sadece ekleme ve okuma yapabilir; güncelleme ve silme yetkisi yoktur. Tablonun sahibi servis rolü değildir ([ADR-015](decisions/015-veritabani-rolleri.md)).
- Her satırda onu yazan veritabanı rolü durur; değer `current_user` varsayılanından gelir, servis rolleri bu kolona değer veremez. Backend, worker adına kayıt uyduramaz ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)).
- Saklama süresi dolan kayıtlar sahip rolüyle çalışan ayrı bir komutla ve bütün dönemler halinde silinir.
- Aynı olaylar stdout'a yapılandırılmış log olarak da yazılır; SIEM'e gönderim v2'dedir.

**Saklama:** Kurulum ayarı, varsayılan 24 ay.

## KVKK

OpenIAM'i kuran kurum **veri sorumlusudur**. Hukuki dayanak, aydınlatma metni ve VERBİS kaydı kurumun sorumluluğundadır ve bu doküman hukuki görüş değildir. Ürünün görevi, kurumun bu yükümlülükleri yerine getirebilmesi için gereken teknik araçları sağlamaktır.

| Veri | Amaç | Nerede | Kim görür | Silinme |
|---|---|---|---|---|
| Ad, soyad | Hesap açma, görünen ad | OpenIAM, AD, Zimbra | Operatörler; AD'de domain kullanıcıları | Kimlik `silindi` olunca ([ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md)); denetim kaydındaki kopyası kendi süresiyle (24 ay) |
| Kimlik numarası | Mükerrer kaydı önlemek; kurum isterse hedef sisteme yazmak | OpenIAM (şifreli); eşleme varsa hedef sistem | Kişisel veri okuyucu | Kimlik `silindi` olunca |
| Cep telefonu | Kurum isterse hedef sisteme yazmak; v2'de parola teslimi | OpenIAM; eşleme varsa hedef sistem | Operatörler | Kimlik `silindi` olunca |
| Sicil no | Kurum içi eşleştirme | OpenIAM; varsayılan eşlemeyle AD `employeeID` | Operatörler | Kimlik `silindi` olunca |
| Departman, rol, tarihler | Yetki hesaplama | OpenIAM, AD | Operatörler | Kimlik `silindi` olunca |
| Ayrılan postası | Otomatik yanıt; kurum açarsa gelen postanın devir yöneticisine yönlendirilmesi | Zimbra | Yönlendirme açıksa devir yöneticisi | Geri almada ya da mailbox silinince. Yönlendirme **varsayılan kapalıdır**: ayrılana gelen bütün postayı (kişisel yazışma dahil olabilir) bir başkasına akıtır; açan kurum aydınlatma metnini ve politikasını buna göre yazar ([ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)) |
| Ayrılanın saklanan posta ayarları | Geri almada kullanıcının kendi yönlendirmesini, filtresini ve otomatik yanıtını geri yazmak | OpenIAM (hesap bağlantısı) | Sistem yöneticisi | Bağlantıyla birlikte, hesap silinince |
| Kullanılmış adlar | Adın tekrar verilmemesi | OpenIAM (düz metin; kişiye ait başka veri yok) | Sistem yöneticisi | Serbest bırakılınca ([ADR-035](decisions/035-kullanilmis-ad-duz-metin-serbest-birakma.md)) |
| Denetim kaydı | Hesap verebilirlik, güvenlik incelemesi | OpenIAM | Denetçi, Sistem yöneticisi | Kendi saklama süresi (24 ay) |

Ürünün sağladıkları:
- **Veri en aza indirme:** Kimlik numarası ve telefon isteğe bağlıdır; zorunlu olmaları kurulum ayarıdır.
- **Erişim kısıtı ve kaydı:** Kimlik numarasının açık görüntülenmesi ayrı yetki ister ve kaydedilir.
- **Şifreleme:** Kimlik numarası uygulama katmanında şifrelenir.
- **İmha:** Son hesabı silen iş kişisel verileri temizler ve kimlik `silindi` olur ([ADR-038](decisions/038-kimlik-durumu-turetilir.md)); mailbox onay beklerken veri sorumlusu bilinçli bekletir. Denetim kaydı kendi süresiyle silinir. **Onay kuyruğu çürür:** kimse onaylamazsa kimlik numarası ve telefon da süresiz kalır. "Silinmeyi bekleyenler" listesi bekleme süresini gösterir ve süreye göre toplu onaylanır; metrik ucu en eski bekleyenin yaşını verir, kurum buna alarm kurar.

> **Not:** Kullanılmış ad kaydı ([ADR-035](decisions/035-kullanilmis-ad-duz-metin-serbest-birakma.md)) adı düz metin tutar ama kişiye ait başka hiçbir veriyle ilişkilendirmez; kimlik silindiğinde ad-soyad ve kimlik numarası temizlenir. Kurum bunu envanterine "kullanıcı adı, kişiye bağlı değil" olarak yazar; kişisel veri sayıp saymayacağı kurumun değerlendirmesidir.

## Faz kapanışlarına eklenecek kontroller

`docs/todo.md`'deki genel kapanış listesine ek olarak, bu ürüne özel testler:

- [ ] Ayrıcalıklı bir grubu veya onun iç içe üyesi olan bir grubu kataloğa almak ve bir role eklemek reddediliyor. Veritabanına doğrudan yazılsa bile worker reddediyor.
- [x] Backend veritabanı rolüyle hesap bağlantısı, katalog veya denetim kaydı değiştirme reddediliyor. _(Faz 3a kapanışı: `migrate.rs` servis rolü testleri — katalog ve denetim Faz 2, hesap bağlantısı 3a)_
- [x] Kendi parolasını belirlemiş bir hesaba ilk parola verilmesi worker'da reddediliyor; ilk girişte değiştirme işareti açıkken de kapalıyken de ([ADR-019](decisions/019-ilk-parola-teslimi.md)). (2026-10-01: `account_unused` birim testi her iki mod; lab testi kapalı modda damga uyuşmazlığıyla red)
- [x] Parola teslimcisi yetkisi kimlik kaydı, rol veya ayrılış değiştiremiyor; sadece ilk parola verebiliyor. (2026-10-01: `helpdesk_requests_then_password_shows_exactly_once` — kayıt, ayrılış, rol, tekrar dene 403)
- [ ] Yönetilen kapsam dışındaki bir OU'ya veya gruba yazma worker'da reddediliyor.
- [ ] Değişiklik seti eşiği aşılınca uygulama duruyor; başlatan kişi kendi değişikliğini onaylayamıyor.
- [ ] Saatlik yıkıcı işlem sınırı aşılınca yeni kimliklerin yıkıcı işlem gerektiren işleri **bütünüyle** bekliyor; yalnızca ekleme ya da öznitelik gerektiren işler ve ilk parolalar devam ediyor ([ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)). İlk parola sınırı ayrı sayılıyor. Acil ayrılış kendi kotası kadar geçiyor, kota dolunca o da bekliyor ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)).
- [ ] Worker yeniden başlatılınca fren sayaçları sıfırlanmıyor; backend veritabanı rolü sayacı düşüremiyor ve sahte denetim satırıyla dolduramıyor.
- [x] Sahiplenme ayarı kapalıyken sahiplenme isteği reddediliyor. Açıkken `adminCount` dolu, yasaklı grup üyesi, kapsam dışı veya başka kimliğe bağlı hesap reddediliyor. (2026-10-01: lab testi `adopts_existing_lab_account_in_observed_mode_after_rule_checks` — beşi de müdahale olarak reddediliyor)
- [x] Gözlem modundaki bağlantıda motor hedef sisteme hiçbir şey yazmıyor. (2026-10-01: aynı lab testi, gözlem işinde yazma niyeti satırı yok)
- [ ] Dosyada olmayan kimlik içe aktarmadan etkilenmiyor; hatalı satır içeren dosyadan hiçbir satır uygulanmıyor.
- [ ] Veritabanı dökümünde, log'larda ve kuyruk tablosunda parola ve düz metin kimlik numarası yok (N-09).
- [x] Virgül, tırnak, yıldız ve parantez içeren adlarla hesap açma doğru DN'yi üretiyor, arama filtresi bozulmuyor. _(Faz 3e kapanışı: engine lab testi `Öz*el Te,st"(x)` ile hesap açıyor, DN'de virgül ve tırnak kaçışlı, kullanıcı adı `ozel.testx`)_
- [ ] Kişi kendi kimlik kaydında rol değiştiremiyor.
- [ ] Saatlik verme sınırı dolunca hesap açma ve mevcut hesaba grup ekleme işleri bekliyor; öznitelik ve yıkıcı işler devam ediyor. Verme sayacı doluyken görev değişikliği işi eski grupları **çıkarmıyor**; pencere açılınca ekleme ve çıkarmayı birlikte uyguluyor ([ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)).
- [x] Üretilen taban ad OpenIAM'e bağlı olmayan bir AD hesabında varsa `n + 1` üretilmiyor, iş müdahale gerekiyor durumuna düşüyor; elle girilen ad çakışınca da aynı ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md)). _(Faz 3b: `worker/src/username.rs` lab testi — `mevcut.personel` bağlı değil → müdahale, elle `ayse.yilmaz` → müdahale, "farklı kişi, sıradaki adı ver" ile `n + 1`)_
- [ ] Sahiplenme açıkken ipucusuz satır içeren CSV reddediliyor; başlığı olmayan kolon hiçbir alanı değiştirmiyor, boş hücre isteğe bağlı alanı temizliyor ([ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md)).
- [ ] Zimbra hesabı saklama süresi dolunca onaysız silinmiyor; AD hesabı siliniyor; kimlik bütün hesaplar silinmeden `silindi` olmuyor ([ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md)).
- [ ] Zaman kilidi kapalıyken başlatan onaylayamıyor; açıkken süre dolmadan onaylayamıyor, dolunca onaylayabiliyor ([ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md)).
- [x] Worker durdurulup saat ileri alındıktan sonra tek tikte bütün geçişler (başlangıç, bitiş, ek rol, saklama) yakalanıyor ([ADR-028](decisions/028-worker-zamanlamasi.md)). _(Faz 3c: `worker/src/scheduler.rs` testleri — geçmiş tarihli veriyle tek tik: başlangıç/bitiş geçişi, bitişi geçmiş ek rol, saklama sonu silme, parola penceresi; saat ileri alma yerine tarihler geçmişe çekildi, zamanlayıcı yalnızca "şimdi"ye bakar)_
- [x] İzinli listede olmayan hedef öznitelikli eşleme satırı worker'da reddediliyor; hassas kaynaklı satır ayar kapalıyken reddediliyor ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md)). _(Faz 3b: `worker/src/mapping.rs` `validate` birim testi — `userAccountControl` satırı, bilinmeyen kaynak/dönüşüm —; lab testinde `mobile ← cep` satırı `SENSITIVE_MAPPING_ENABLED=false` iken işi müdahaleye düşürüyor)_
- [ ] Ayrılışı geri alma saatlik yıkıcı sayaca giriyor; CSV'de `ayrıldı` kimliğin bitiş tarihini boşaltan satır dosyayı reddettiriyor ([ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)).
- [ ] Onay bekleyen taslak motor tarafından okunmuyor: taslak beklerken aynı kimliğe açılan iş yayımlanmış tanımı uyguluyor; red taslağı siliyor, model değişmiyor; başlatan onaylayamıyor ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md)).
- [x] AD'de ve Zimbra'da elle pasifleştirilen hesap, kimlik durumu değişmeden açılan işte pasif kalıyor; durum geçişinde etkinleşiyor; aynı iş ikinci kez fark üretmiyor ([ADR-032](decisions/032-elle-pasiflestirme-korunur.md)). _(Faz 3e kapanışı: engine lab testi — hesap AD'de elle `userAccountControl 514` yapılıyor, iş onu yeniden etkinleştirmiyor; askı/ayrılış geçişlerinde etkin-pasif yazılıyor. Zimbra karşılığı Zimbra bölümünde)_
- [ ] Ayrılıştan 7 gün önce parola ve `pwdLastSet` değişmiyor, 7 gün sonra değişiyor; acil ayrılışta hemen; pencerede geri alma ilk parola istemiyor ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md)).
- [ ] Yönetime almada `sAMAccountName` ve UPN yazılmıyor; "sadece boşsa yaz" satırı dolu değeri koruyor, boş değeri dolduruyor ve sapma raporlamıyor ([ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)).
- [x] Serbest bırakılan ad yeniden üretiliyor ve denetim kaydında gerekçesi var; kayıt iptali adı yakmıyor ([ADR-035](decisions/035-kullanilmis-ad-duz-metin-serbest-birakma.md)). _(Faz 3e kapanışı: `username::resolve` lab testi serbest bırakılmış `serbest.ad`'ı sonek eklemeden veriyor; gerekçe `used_names.rs` testinde denetim satırında; iptalin ad yakmadığı engine lab testinde)_
- [x] İlk parolanın şifreli değeri gösterildikten sonra ve gösterilmezse 10 dakika sonra siliniyor ([ADR-036](decisions/036-ilk-parola-aead.md)). (2026-10-01: backend `take` testi ikinci açılışta yok; worker `tick_expires_unshown_and_unanswered_first_passwords`)
- [ ] Yalnızca ekleme üreten geniş düzenleme (temel role grup ekleme) eşiğe takılıyor; yalnızca özniteliği değişen kimlikler sayılmıyor ([ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md)).
- [x] Worker rolü kimlik tablosunda kişisel veri kolonları ve `silindi_anı` dışında güncelleme yapamıyor; `bekliyor` iken bitişi geçmiş kimlik tek tikte tek iş üretip `ayrıldı` oluyor ([ADR-038](decisions/038-kimlik-durumu-turetilir.md)). _(Faz 3a kapanışı: kolon yetkisi `migrate.rs`, tek tik tek iş `worker/src/scheduler.rs` testi)_
- [x] "Hesap açılsın = hayır" ayarlı role geçen kimliğin mevcut hesabı silinmiyor ve kapanmıyor; GUID'i bulunamayan bağlantı için iş hesap açmıyor, "kayıp hesap" bulgusu düşüyor; yöneticisi bu hedefte bağlı olmayan kimlikte `manager` temizlenmiyor ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)). _(Faz 3b: `desired_state` birim testi + engine lab testi — ayar `hayır` iken bağlı hesap yaşam döngüsüne göre pasifleştiriliyor, silinen hesap "kayıp hesap", bağlantı korunuyor —; `mapping::changes` testi yönetici belirsizken `manager`'a dokunmuyor)_
- [x] Bitiş tarihi ileride olan yöneticinin astlarının `manager`'ı değişmiyor; bitiş anında devir yöneticisine yazılıyor; geri almada geri dönüyor ([ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md)). _(Faz 3c: `effective_manager` birim testi — ileri tarihli bitişte durum `aktif`, yönetici değişmez; `ayrıldı`da tek atlama devir, o da ayrılmışsa boş —; engine lab testi yöneticinin ayrılış işinin asta iş açtığını doğruluyor; `manager` özniteliği eşleme farkıyla yazılıyor (`mapping::changes` testi belirsizde dokunmuyor))_
- [ ] Kullanılmış adla çakışan kayıt `n + 1` almıyor, müdahaleye düşüyor; aynı ad-soyad farklı sicil satırı onaysız yayımlanmıyor; sahiplenmede ad uyuşmazlığı bayrak bırakıyor, reddetmiyor ([ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)).
- [ ] Gözlem modundaki 100 kimliği etkileyen rol düzenlemesi onay istemiyor; aynı kimlikler yönetime alınırken fark eşiğe giriyor ([ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md)).
- [ ] Ayrılışta yönlendirme yalnızca ayar açıkken ve yalnızca yönetilen alan adındaki bağlı bir adrese yazılıyor; devir yöneticisi yoksa yazılmıyor; eşleme satırıyla yazılamıyor; geri almada kaldırılıyor ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md)).
- [ ] Ayrılış anında kullanıcının kendi yönlendirmesi ve filtresi temizleniyor, geri almada geri geliyor; otomatik yanıt pencere dolmadan yazılmıyor, acil ayrılışta hemen yazılıyor ([ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)).
- [x] `lastLogonTimestamp` dolu hesap `pwdLastSet = 0` olsa bile ilk parola almıyor; ayrılışta sıfırlanan hesap geri almada alıyor ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)). (2026-10-01: `first_password::account_unused` birim testi, `issue` işareti temizleme DB testi, lab'da kullanılmış hesap reddi)
- [x] Sahiplenilip yönetime alınmış ya da `lastLogonTimestamp`'ı dolu kimlikte kayıt iptali hesabı **silmiyor**, planlı ayrılış gibi uyguluyor; kullanılmamış ve OpenIAM'in açtığı hesabı hemen siliyor ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)). _(Faz 3c: `desired_state` birim testi — sahiplenilmiş/kullanılmış bağlantıda iptal etkisiz, ayrılış gibi —; engine lab testi — kullanılmamış hesapta iptal doğrulanıp hesap siliniyor, kimlik `silindi`, ad yakılmıyor)_
- [ ] "Kaydet ve ilk parolayı ver" tek işlemde hesabı açıp parolayı gösteriyor, mailbox başarısızken de; üretilen parola karışan karakter içermiyor ve AD karmaşıklık kuralından geçiyor; parolası verilmiş ama `lastLogonTimestamp`'ı boş kayıt iptal edilebiliyor, giriş yapılmış kayıt edilemiyor ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)). _(Faz 3e kapanışı: "giriş yapılmış kayıt iptal edilemiyor" lab testiyle doğrulandı — gerçek bind'dan sonra iptal reddediliyor, hesap silinmiyor; tek adım ve parola biçimi `scripts/e2e-lab.sh` ve birim testinde. Kalan: "mailbox başarısızken de" — Zimbra bölümü)_
- [ ] Mutabakat sürerken açılan acil ayrılış N-02 içinde uygulanıyor ([ADR-051](decisions/051-okuma-seridi.md)).
- [ ] Kapsam dışına taşınmış hesabın ayrılışı tek iş üretip müdahaleye düşüyor, zamanlayıcı yenisini açmıyor, "ayrılmış ama kapatılamamış" metriği 1 gösteriyor; LDAP kapalıyken açılan işin deneme sayısı azalmıyor; devir yöneticisi ayrılınca onu gösteren ayrılmışın işi açılıyor ([ADR-052](decisions/052-uygulanamayan-fark.md)).
- [x] İleri tarihli askı başlangıç gününde pasifleştiriyor, dönüş günü etkinleştiriyor; askıdayken bitişi geçen kimlik `ayrıldı` oluyor ([ADR-053](decisions/053-tarihli-aski.md)). _(Faz 3e kapanışı: engine lab testi — askı başlayınca hesap pasif, üyelik ve OU korunuyor (pasif OU tanımlı olsa bile taşınmıyor), askı kalkınca etkin; `lifecycle_state` birim testi askıdayken bitişi geçeni `ayrıldı` sayıyor)_
- [x] Kuru çalıştırma modunda hesap açma, grup ekleme ve pasifleştirme hedefte değişiklik üretmiyor, fark iş sonucunda görünüyor; ilk parola reddediliyor ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)). (2026-10-01: lab testleri `provisions_account_in_lab_then_is_idempotent` kuru adım ve `issues_first_password_in_lab_only_to_unused_account` kuru red)
- [ ] Taslak kaydedildikten sonra role eklenen kimlik onay ekranındaki sayıya giriyor; CSV tarihli ek rolü silmiyor; aynı kimlik no farklı sicil satırı onayla sicil noyu güncelliyor; `ayrıldı` kimlikle eşleşen operatörün yazma isteği reddediliyor; AD'nin reddettiği parola yeniden üretiliyor ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)).
- [ ] Zimbra hesabı parolasız açılıyor (`userPassword` yok) ve AD parolasıyla giriliyor; AD hesabı tek `add` ile açılıyor, AD'nin reddettiği parolada hesap oluşmuyor; worker yazma işlemlerinde `<GUID=…>` kullanmıyor ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)).
- [ ] Zimbra kapalıyken "kaydet ve ilk parolayı ver" AD hesabını açıp parolayı gösteriyor, mailbox bağlantı gelince açılıyor; kontrol atlandıktan sonra dolu adrese düşen Zimbra işi müdahaleye düşüyor, `n + 1` üretilmiyor ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md)).
- [ ] `ayrıldı` kimlikle eşleşen operatörün **okuma** isteği de reddediliyor, yeniden girişi oturum açmıyor; bitişi kaldırılıp başlangıcı ileri alınan `ayrıldı` kimlik yıkıcı sayaca giriyor; askı bitişi 15'i olan kimlik 16'sı 00:00'da etkinleşiyor; bitiş tarihi kaldırılınca `accountExpires` süresiz oluyor ([ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)).
- [x] `msDS-LogonTimeSyncInterval = 0` olan domain'de AD connector'ı başlamıyor ve nedeni log'da; değer kaldırılınca başlıyor ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)). _(Faz 3a kapanışı: `ad::logon_timestamp_enabled` birim testi — `0` kapalı, yok/≥1 açık —, `startup_checks` lab testi nedeni döndürüyor, worker açılışta logluyor)_
- [ ] 20 karakteri aşan kullanıcı adı [ADR-011](decisions/011-kullanici-adi-ve-eposta.md) kuralıyla kısalıyor (önce baş harf, sonra soyad kesme, çakışma soneki için yer kalıyor); 64 karakteri aşan CN kesiliyor, hesap açılıyor ([docs/05](05-active-directory.md#cn-kuralı)).
- [ ] SIGTERM alan worker elindeki işi bitirip ≤ 25 sn'de çıkıyor; döngüsü durdurulan worker `unhealthy` oluyor, AD kapalıyken `healthy` kalıyor; şeması eski veritabanına karşı backend ve worker sıfır olmayan kodla çıkıyor; backend yeniden başlatılınca operatör oturumu sürüyor; metrik ucu token'sız 401 dönüyor ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)).
- [ ] İş ortasında `SIGKILL` alan worker'ın işi kira dolunca deneme sayısı azalmadan yeniden alınıyor, hedefte tek hesap oluşuyor; veritabanı kapalıyken hedefe yazılmıyor; iki worker aynı kuyrukta çalışırken hiçbir iş iki kez alınmıyor; uygulanan her işlemin niyet satırı var ([ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md)).
- [ ] Faz 5: docs/09 eşlemesiyle tek node'lu bir Kubernetes kümesinde (k3s/kind) backend iki kopya, worker tek kopya ayağa kalkıyor; migration Job'ı bitmeden servisler hazır olmuyor; worker pod'u silinince yenisi kuyruğu sürdürüyor (N-14).
