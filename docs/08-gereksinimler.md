# 08 — Gereksinimler ve Açık Sorular

> Yaşayan bir dosyadır. Açık bir soru cevaplanınca buradan silinir ve karar `docs/decisions/` altına yazılır.

## İşlevsel gereksinimler

### v1

| ID | Gereksinim |
|---|---|
| F-01 | Yönetim ekranına OIDC ile giriş yapılır; yönetim yetkileri groups claim'inden okunur; kimliği `ayrıldı` ya da `askıda` olan operatörün her isteği reddedilir ([ADR-005](decisions/005-yonetim-girisi-oidc.md), [ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md), [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)) |
| F-02 | Kimlik kaydı v1 şemasıyla yapılır; kimlik numarası, telefon ve tarih kuralları sunucuda doğrulanır; ad-soyad mükerrer uyarısı; kimliği olan rol ve departman silinemez ([docs/03](03-rol-ve-veri-modeli.md)) |
| F-03 | Departmanlar ağaç olarak tanımlanır; yetki öğesi ve tek değerli varsayılan taşır, alt departmanlar miras alır ([ADR-017](decisions/017-departman-hiyerarsisi.md)) |
| F-04 | Roller (temel, birincil, ek) katalogdan tanımlanır; kaydetmeden önce etki önizlemesi gösterilir ([ADR-007](decisions/007-rol-modeli.md)) |
| F-05 | Katalog AD'den (OU, grup) ve Zimbra'dan (COS, dağıtım listesi) içe alınır; kaybolan öğeler işaretlenir; ayrıcalıklı gruplar alınamaz |
| F-06 | Kullanıcı adı ve e-posta şablonla üretilir ya da isteğe bağlı elle girilir, çakışma kontrol edilir, bağlı olmayan hesapla çakışmada durulur; silinen kimliğin adları kullanılmış ad kaydına girer ve Sistem yöneticisi serbest bırakabilir, kayıt iptali adı yakmaz; kullanılmış adla çakışma müdahaledir ([ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)); Zimbra erişilemezken ad üretimi beklemez, Zimbra kontrolü atlanır ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md)) ([ADR-011](decisions/011-kullanici-adi-ve-eposta.md), [ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md), [ADR-035](decisions/035-kullanilmis-ad-duz-metin-serbest-birakma.md)) |
| F-07 | Hedef sistem başına öznitelik eşlemesi ayarlanır; hedef öznitelik kodda sabit izinli listeden seçilir, hassas kaynak eşlemesi worker ayarıyla açılır, satırda "sadece boşsa yaz" seçeneği vardır ([ADR-012](decisions/012-oznitelik-esleme.md), [ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md), [ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)) |
| F-08 | AD'de hesap açılır, öznitelikler yazılır, gruplar ve OU yönetilir, hesap etkinleştirilir/pasifleştirilir, `accountExpires` yazılır, hesap silinir ([docs/05](05-active-directory.md)) |
| F-09 | Zimbra'da hesap açılır, COS ve liste üyelikleri yönetilir, hesap girişe kapatılır ve silinir ([docs/06](06-zimbra.md)) |
| F-10 | Yaşam döngüsü durumları tarihlere göre işler; askıya alma, acil ayrılış, ayrılışı geri alma, kayıt iptali ve saklama sonrası silme yapılır; saklama süresi hedef sistem başınadır, mailbox onayla silinir; etkinleştirme yalnızca durum geçişinde; ayrılışta parola 7 gün sonra sıfırlanır; geri alma yıkıcı sayılır; durum saklanmaz, tarihlerden türetilir; rol ayarı mevcut hesabı silmez ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)); ayrılışta kullanıcının kendi yönlendirmesi ve filtresi temizlenir, otomatik yanıt 24 saat sonra yazılır, devir yöneticisine yönlendirme ayarla açılır ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md), [ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)); kayıt iptali hedefte doğrulanır, sahiplenilen hesap iptal edilemez ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)); askı tarihlidir, askı bitişi iznin son günüdür ([ADR-053](decisions/053-tarihli-aski.md)); `ayrıldı`dan her çıkış geri almadır, `accountExpires` bitiş kaldırılınca süresiz yazılır ([ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)) ([ADR-013](decisions/013-yasam-dongusu.md), [ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md), [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md), [ADR-032](decisions/032-elle-pasiflestirme-korunur.md), [ADR-033](decisions/033-ayrilista-parola-gecikmesi.md), [ADR-038](decisions/038-kimlik-durumu-turetilir.md)) |
| F-11 | İlk parola bir kez gösterilir, hiçbir yerde saklanmaz ve sadece parolası henüz belirlenmemiş hesaba verilir; İK operatörü ve yardım masası verebilir; ilk girişte değiştirme işareti kurulum ayarıdır; şifreli değer bir kez gösterilip silinir; yalnızca hiç giriş yapılmamış hesaba ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)), `lastLogonTimestamp`'ın domain'de kapatılmamış olduğu açılışta doğrulanır ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)); aynı gün kaydında "kaydet ve ilk parolayı ver" tek adımdır, teslim edilen parola okunabilir biçimdedir ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)) ([ADR-009](decisions/009-parola-yonetimi.md), [ADR-019](decisions/019-ilk-parola-teslimi.md), [ADR-036](decisions/036-ilk-parola-aead.md)) |
| F-12 | Kişi sayfası: durum, tarihler, departman ve roller, hesaplar, iş durumu ve son olaylar tek ekranda. Başarısız işin sebebi operatör dilinde yazılır, teknik hata ayrıntıda kalır; bekleyen iş neden beklediğini (eşik, sayaç, onay), kalan süreyi ve onaylayacak grubu gösterir; başarısız işler "müdahale gerekiyor" listesine düşer ve tekrar denenebilir |
| F-13 | Mutabakat raporu gece ve istendiğinde üretilir; her bulgu için "yeniden uygula" vardır; elle pasifleştirilmiş ve kapsam dışı hesaplar ayrı bulgudur; kişi olmayan hesaplar "bilinen istisna" işaretlenir ([docs/04](04-yasam-dongusu.md#mutabakat-raporu), [ADR-032](decisions/032-elle-pasiflestirme-korunur.md)) |
| F-14 | Yönetilen kapsam, yasaklı gruplar ve saatlik sayaçlar worker'da uygulanır; değişiklik seti eşiği model farkından backend'de hesaplanır ve ekleme işlemlerini de sayar, eşiği aşan düzenleme taslak olarak bekler; eşik gözlemdeki kimlikleri saymaz ([ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md)); saatlik sayaçlar yıkıcı, verme ve ilk paroladır, iş sayaçlara karşı bütündür ([ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)); onay ekranı farkı onay anında yeniden hesaplar ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)); onay zaman kilidi ([ADR-014](decisions/014-yonetim-kapsami-ve-toplu-degisiklik-freni.md), [ADR-021](decisions/021-yeni-hesap-saatlik-siniri.md), [ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md), [ADR-031](decisions/031-degisiklik-seti-sahneleme.md), [ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md)) |
| F-15 | Kimlik numarası şifreli saklanır, maskeli gösterilir; açık görüntüleme yetki ister ve kaydedilir ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md)) |
| F-16 | Her durum değiştiren işlem ve her hassas veri görüntüleme denetim kaydına yazılır; kayıt ekranda aranabilir |
| F-17 | CSV ile toplu kimlik içe aktarma: sicil no ile eşleştirme, doğrulama ve önizleme, tek değişiklik seti; dosyada olmayana dokunulmaz; başlığı olmayan kolon dokunmaz, boş hücre temizler; sahiplenme açıkken ipucu zorunlu; ayrılmış kimliğin bitiş tarihini boşaltamaz; aynı ad-soyad farklı sicil satırları "olası mükerrer" onayı ister ([ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)); aynı kimlik no farklı sicil satırı "sicil no değişimi" önerisidir, ek roller kolonu tarihli atamaya dokunmaz ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)) ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md), [ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md), [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)) |
| F-18 | Mevcut hesapların sahiplenilmesi: worker doğrulaması, gözlem modu, yönetime alma; worker ayarıyla açılır; hedefteki ad-soyad uyuşmazlığı uyarı bayrağıdır ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md), [ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)) |
| F-19 | İzleme için metrik ucu ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)); "ayrılmış ama kapatılamamış" bağlantı sayısı ([ADR-052](decisions/052-uygulanamayan-fark.md)), silinmeyi bekleyen en eski hesabın yaşı, kuru çalıştırma bayrağı ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)) |
| F-36 | Yaklaşan bitişler: önümüzdeki 30 gün (ayar) içinde bitiş tarihi olan kimlikler, süresi dolacak ek roller, yaklaşan askı başlangıçları ve dönüşler ([ADR-053](decisions/053-tarihli-aski.md)) listelenir; İK sözleşme yenilemeyi buradan görür |
| F-40 | Worker kuru çalıştırma modu: hedefe yazmadan farkı gösterir; ilk kurulum, yükseltme, yedekten dönüş ve bakım penceresi için ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)) |
| F-37 | Ek role isteğe bağlı bitiş tarihi; tarih dolunca rol kendiliğinden kaldırılır ([ADR-020](decisions/020-sureli-ek-rol.md)) |
| F-38 | Yönetici ayrılışında astların etkin yöneticisi, ayrılanın kaydındaki devir yöneticisinden bitiş anında türetilir; geri almada kendiliğinden döner; kişi sayfası ast sayısını ve yöneticisiz kalanları gösterir ([ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md)) |
| F-42 | Gösterge paneli: tarih aralığı filtresi (varsayılan son 30 gün); işe giren/ayrılan/görev değiştiren sayısı; departman ve rol kırılımı (pasta, native CSS `conic-gradient`, kütüphane yok); "ayrılmış ama kapatılamamış" ve onay bekleyen taslak sayısı ([ADR-076](decisions/076-gosterge-paneli-v1-kapsami.md)) |

### v2

| ID | Gereksinim |
|---|---|
| F-20 | Mevcut üyeliklerden rol çıkarma yardımı (role mining). Sahiplenme v1'e alındı (F-18) |
| F-21 | İK sisteminden otomatik besleme: içe aktarma ucunun makine istemcisiyle çağrılması. CSV v1'e alındı (F-17) |
| F-22 | İleri tarihli görev değişikliği ve eski yetkiler için devir süresi. Devir ihtiyacının çoğunu v1'deki süreli ek rol karşılar (F-37) |
| F-23 | Süreli kişisel istisna yetkisi (role bağlı olmayan tek yetki öğesi). Süreli ek rol v1'de (F-37) |
| F-24 | Kuruma özel ek kimlik alanları |
| F-25 | E-posta yeniden adlandırma; eski adres takma ad olarak kalır. **v2'nin ilk maddesi:** evlilikle soyad değişimi İK'ya gelen en sık taleptir |
| F-26 | Parola teslimi SMS veya aktivasyon bağlantısıyla; parola değiştirme sayfası |
| F-27 | Olay bildirimi: e-posta ve webhook (acil ayrılışta OpenBerat kill switch çağrısı dahil) |
| F-28 | Hedef sistem bazında mutabakat bulgularının otomatik düzeltilmesi |
| F-29 | Mailbox devri: paylaşım, dışa aktarma, silmeden arşiv. Yönlendirme ve otomatik yanıt v1'e alındı ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md)) |
| F-30 | Denetim kaydının SIEM'e gönderilmesi (syslog, webhook) |
| F-32 | SCIM 2.0 connector'ı ve manuel görev connector'ı |
| F-33 | Birden fazla AD domain'i veya forest'ı |
| F-34 | Operatör yetkisinin departman alt ağacıyla sınırlanması (yetki devri) |
| F-35 | Yetki dökümünün (kimlik × rol × yetki öğesi) CSV olarak dışa aktarılması |
| F-41 | Carbonio CE hedefi: aynı Admin SOAP API'si (`urn:zimbraAdmin`, 7071); aynı connector, lab doğrulaması. v1.x adayı ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)) |
| F-39 | Kayıp katalog öğesini (silinip aynı adla yeniden açılan grup) tüm rollerde tek işlemde yenisiyle değiştirme. v1.1 adayı |

### Kapsam dışı

- Kimlik doğrulama, SSO, MFA → Keycloak gibi bir IdP
- Uygulama erişim kararı → uygulamanın kendisi veya OpenBerat gibi bir IAP
- Ayrıcalıklı hesap yönetimi (PAM)
- Yetki gözden geçirme kampanyaları ve SoD politika motoru
- Entra ID / Microsoft 365 lisans ve bulut hesabı yönetimi (AD'ye yazılan hesaplar Entra Connect ile zaten senkronlanır)
- Carbonio v1'de doğrulanmış hedef değildir; aynı Admin SOAP API'sini koruduğu için v1.x adayıdır (F-41)
- İK süreçleri: izin, bordro, performans
- AD şemasını değiştirmek
- Kişi olmayan hesaplar (servis hesabı, ortak posta kutusu, test hesabı) → yönetilen OU dışında durur; içindeyse raporda bilinen istisna ([docs/09](09-kurulum.md))

## İşlevsel olmayan gereksinimler

Hedefler tahmindir; ilgili fazda ölçülüp bu tabloya ölçülen değer yazılır.

| ID | Gereksinim | Hedef |
|---|---|---|
| N-01 | Kayıttan sonra hesapların pasif olarak açılması | ≤ 60 sn |
| N-02 | Acil ayrılışta AD hesabının pasifleşmesi | ≤ 30 sn, toplu bir değişiklik seti ya da mutabakat sürerken de (okuma işleri ayrı şeritte, [ADR-051](decisions/051-okuma-seridi.md); acil ayrılış kotası dolu değilse, [ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)); kuyruk 5 sn'de bir yoklanır ([ADR-028](decisions/028-worker-zamanlamasi.md)) |
| N-03 | Ölçek | 20.000 kimlik, 1.000 rol, 500 departman, 10.000 katalog öğesi; gece mutabakatı ≤ 30 dk; 2.000 kimliği etkileyen değişiklik seti ≤ 30 dk ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)) |
| N-04 | Tek makinede `docker compose up` ile çalışma | v1 |
| N-14 | Tek sunucu, iki sunucu (uygulama + veritabanı; ön yüz + worker) ve Kubernetes'te çalışma | Aynı imajlar, [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md) süreç sözleşmesi; chart yayımlanmaz, [docs/09](09-kurulum.md#dağıtım-biçimleri) eşlemeyi verir. Faz 5'te bir kez tek node'lu kümede doğrulanır. Worker'ı çoğaltmak N-03'ü iyileştirmez |
| N-05 | Yüksek erişilebilirlik | v1'de yok: tek worker, tek backend. İkinci worker'ı güvenli kılan kurallar (tekil zamanlayıcı, sayaçlar veritabanında) baştan uygulanır |
| N-06 | Denetim kaydı saklama süresi | Kurulum ayarı, varsayılan 24 ay |
| N-07 | İdempotentlik | Aynı iş ikinci kez çalıştığında hedef sistemde değişiklik olmaz (testle) |
| N-08 | Worker'ın ağ yüzeyi | Yayımlanmış port yok, gelen bağlantı yok |
| N-09 | Parola ve kimlik numarasının düz metin olarak bulunmaması | Veritabanı dökümünde, log'larda ve kuyrukta yok (lab testi) |
| N-10 | AD bağlantısı | Sadece LDAPS; sertifika doğrulaması kapatılamaz, CA sertifikası verilir |
| N-11 | Kuyrukta öncelik | 2.000 kimliklik bir değişiklik seti sürerken tek kimlik işlemi ≤ 60 sn içinde başlar |
| N-12 | Liste ve arama uçları | Sunucu tarafında sayfalanır; N-03 ölçeğinde kimlik araması ≤ 1 sn |
| N-13 | Aynı gün işe başlama: "kaydet ve ilk parolayı ver"den parolanın ekranda görünmesine | ≤ 60 sn, tek operatör işlemi ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)). Ürünün hedef sahnesidir; AD replikasyonu ve ilk giriş kanalı bu ölçümün dışındadır ([docs/09](09-kurulum.md#aynı-dakika-giriş)) |

## Netleştirilenler

### Mimari ve altyapı

Karar listesi tek yerde tutulur: [PROJECT.md → Kararlar](PROJECT.md#kararlar).

### Kuruma özel sorular ürüne nasıl yansıdı

Önceki analizdeki "kurumunuzda nasıl?" soruları tek bir şirkete göre cevaplanmadı. Global ürünlerde olduğu gibi her biri bir **kurulum ayarına**, bir **ön koşula** ya da **bir sürüm kapsamına** dönüştürüldü.

| Soru | Üründeki karşılığı |
|---|---|
| İK sistemi var mı? | v1'de kaynak OpenSicil'in kendi kişi kaydıdır; CSV ile beslenir (F-17). İK sisteminden otomatik besleme v2 (F-21) |
| Personel sayısı | N-03 hedefi; eşikler için boyutlandırma tablosu ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)) |
| Şube, il, bölge, şirket yapısı | Departman ağacının seviyeleri ([ADR-017](decisions/017-departman-hiyerarsisi.md)) |
| Birden fazla e-posta alan adı | Departman zincirinden gelen tek değerli ayar ([ADR-017](decisions/017-departman-hiyerarsisi.md)) |
| Birden fazla domain controller | DC listesi; iş boyunca tek DC ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)) |
| Stajyer, sözleşmeli, dış kaynak | Çalışma tipi alanı; kadrolu dışında bitiş tarihi zorunlu |
| Tek domain mi? | v1 tek AD domain'i; çoklu domain v2 (F-33) |
| Entra ID hibrit | Kapsam dışı; AD tarafı olduğu gibi çalışır |
| LDAPS açık mı? | Ön koşul (N-10) |
| OU ve grup düzeni | Yönetilen kapsam ayarı ([ADR-014](decisions/014-yonetim-kapsami-ve-toplu-degisiklik-freni.md)) |
| Kullanıcı adı standardı, çakışma | Şablon ayarı; isteğe bağlı elle giriş; mevcut hesapla çakışmada durma ([ADR-011](decisions/011-kullanici-adi-ve-eposta.md), [ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md)) |
| Doldurulacak AD öznitelikleri | Eşleme ayarı ([ADR-012](decisions/012-oznitelik-esleme.md)) |
| T.C. Kimlik No zorunlu mu, AD'de nereye yazılıyor? | Zorunluluk kurulum ayarı; hangi özniteliğe yazılacağı eşleme ayarı (örnek: `employeeNumber`) |
| Telefon biçimi | E.164 saklanır; hedefe yazılacak biçim eşleme dönüşümüyle seçilir (örnek: `905321234567`) |
| Zimbra sürümü | Belirli bir sürüme değil Admin SOAP API işlemlerine bağlı; lab'da doğrulanan sürüm dokümana yazılır; Carbonio CE aynı API'yi korur, v1.x adayı (F-41) ([docs/06](06-zimbra.md)) |
| Ayrılışta Zimbra postası gelmeye devam etsin mi? | Kurulum ayarı: `locked` (varsayılan, posta gelir) veya `closed` (posta geri döner) |
| Zimbra parolası | Ön koşul: parola AD'de doğrulanır ([ADR-009](decisions/009-parola-yonetimi.md)) |
| COS ve dağıtım listeleri | Katalogdan, rol ve departmana bağlı |
| Roller neler? | Kurumun gireceği veri. Ürün sadece modeli sağlar |
| Rol sahibi, rol onayı | v1'de yok. Toplu değişiklik onayı var |
| Hesaplar ne zaman açılır/kapanır? | Kayıt anında pasif; başlangıç 00:00; bitiş günü sonu. Ayar ([ADR-013](decisions/013-yasam-dongusu.md)) |
| Acil fesih kimde? | İK operatörü yetkisi, öncelikli iş |
| Saklama süreleri | AD hesabı 90 gün; Zimbra hesabı onayla silinir; denetim kaydı 24 ay. Ayar ([ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md)) |
| Devir süresi | v2 (F-22); v1'de bitiş tarihli ek rol ([ADR-020](decisions/020-sureli-ek-rol.md)) |
| Yeniden işe alım | Kimlik silinmediyse ayrılış geri alınır (aynı adlar); silindiyse yeni kayıt, eski adla çakışma müdahaleye düşer ve Sistem yöneticisi adı serbest bırakabilir ([docs/04](04-yasam-dongusu.md#yeniden-işe-alım), [ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)) |
| Stajyer ya da taşeron kadroya geçti, sicil no değişti | Yeni kayıt değil; İK mevcut kaydın sicil no ve çalışma tipini düzenler; CSV önizlemesi aynı ad-soyadlı yeni sicil satırını "olası mükerrer" işaretler ([ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)) |
| Rolü mailbox öngörmeyen göreve geçen personel | Mevcut hesap silinmez, kapanmaz; ayar yalnızca hesap yokken okunur ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)) |
| Yardım masası hesabı elle sildi | Motor yeniden açmaz; Recycle Bin'den geri alınırsa bağlantı kendiliğinden canlanır ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)) |
| Servis hesabı parolası ya da CA sertifikası doldu | Worker sessizce durur; metrik ucundaki hedef sistem başına son başarılı bağlantı zamanına alarm ([docs/05](05-active-directory.md#ön-koşullar-kurumun-işi)) |
| Güvenlik ekibi hesabı AD'de elle kapattı | Motor geri açmaz; etkinleştirme yalnızca durum geçişinde, rapora düşer ([ADR-032](decisions/032-elle-pasiflestirme-korunur.md)) |
| Sözleşme yenilemesi unutuldu, kişi sabah giremiyor | Bitiş tarihi uzatılır; 7 gün içinde parola değişmediği için ilk parola gerekmez ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md)) |
| Mevcut hesapların UPN soneki veya görünen ad biçimi farklı | UPN yeniden yazılmaz; eşleme satırında "sadece boşsa yaz" ([ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)) |
| Servis hesapları ve ortak posta kutuları | Kapsam dışı; yönetilen OU dışında durur, içindeyse raporda bilinen istisna ([docs/09](09-kurulum.md)) |
| Yöneticisi ayrılan personel | Ayrılış formunda devir yöneticisi seçilir (varsayılan bir üst); astların etkin yöneticisi bitiş anında türetilir, geri almada döner ([ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md)) |
| İlk parola teslimi | Bir kez gösterim; İK veya yardım masası verir; ilk girişte değiştirme işareti ayar ([ADR-019](decisions/019-ilk-parola-teslimi.md)). SMS ve aktivasyon v2 (F-26) |
| Parolasını unutan personel | OpenSicil parola vermez; AD'nin kendi sıfırlama süreci (yardım masası). Kişi sayfası bunu söyler |
| Ayrılan kişinin postası ve mailbox'ı | Kullanıcının kendi yönlendirmesi ve filtresi ayrılışta temizlenir; otomatik yanıt 24 saat sonra yazılır; devir yöneticisine yönlendirme ayarla açılır, varsayılan kapalı ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md), [ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)); paylaşım ve arşiv kurumun işi, v2 (F-29) |
| Kim personel girer, kim rol tanımlar, kim parola verir, kim onaylar? | Altı yönetim yetkisi ([ADR-005](decisions/005-yonetim-girisi-oidc.md), [ADR-019](decisions/019-ilk-parola-teslimi.md)) |
| Uzun izin, askerlik, doğum izni | Tarihli askı: başlangıç ve dönüş baştan girilir, ikisi de kendiliğinden uygulanır ([ADR-053](decisions/053-tarihli-aski.md)) |
| Veritabanı yedekten dönüldü, sürüm yükseltildi, DC bakımda | Worker kuru çalıştırma modu ve yedekten dönüş adımları ([ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md), [docs/09](09-kurulum.md#yedekten-dönüş)) |
| Zimbra dört saat kapalı kaldı | Bağlantı hatası deneme tüketmez; Zimbra işleri bekler, bağlantı gelince sürer ([ADR-052](decisions/052-uygulanamayan-fark.md)). O gün işe başlayanın AD hesabı ve parolası beklemez: ad üretimi Zimbra kontrolünü atlar ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md)) |
| Terfi edip başka OU'ya taşınan kişi ayrıldı | Motor kapsam dışına yazamaz; iş müdahaleye düşer, "ayrılmış ama kapatılamamış" metriği alarm üretir ([ADR-052](decisions/052-uygulanamayan-fark.md)) |
| Toplu değişiklik eşiği | Varsayılan değişiklik setinde 10 kimlik; saatte 50 yıkıcı, 50 verme (hesap açma ve yetki ekleme, [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)), 50 ilk parola, 5 acil ayrılış. Eşik gözlemdeki kimlikleri saymaz ([ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md)). Ayar; kurum büyüklüğüne göre öneri tablosu ([docs/09](09-kurulum.md#boyutlandırma)). Tek yöneticili kurumda onay zaman kilidi ([ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md)). Eşiği aşan düzenleme taslakta bekler; sayı model farkından kesindir ve ekleme işlemlerini de sayar ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md), [ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md)) |
| SIEM | v1'de stdout'a yapılandırılmış log; gönderim v2 (F-30) |
| Hata bildirimi | v1'de ekrandaki liste, log ve kurumun izleme sistemine metrik ucu (F-19); e-posta/webhook v2 (F-27) |
| Mutabakat otomatik mi? | v1'de sadece rapor; otomatik düzeltme v2 (F-28) |
| Mevcut personel | CSV ile içe aktarılır, gözlem moduyla sahiplenilir, onayla yönetime alınır (F-17, F-18). Sahiplenme kapalıyken dokunulmaz ve raporda "yönetilmeyen hesap" olarak görünür. Geçiş döneminde ipucu zorunlu ([ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md)) |

## Açık sorular

### 🔴 Lab'da ölçülecek

Önceki listedeki soruların çoğu birincil kaynakla (Zimbra ve Samba kaynak kodu, Microsoft protokol belgeleri, crate ve Keycloak dokümanı) cevaplandı ve buradan silindi; cevaplar, alıntılar ve bağlantılar [docs/11](11-dogrulama-notlari.md)'de, kararlar [ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)'dedir. Kalanlar gerçekten yalnızca çalışan sisteme karşı ölçülebilir.

**Faz 1c — AD ve `ldap3` (Samba lab; ⊞ işaretliler bir kez Windows Server VM ister)**
- [x] Tek `add` (`unicodePwd` + `userAccountControl = 514` + `pwdLastSet = 0`) LDAPS ve simple bind ile kabul ediliyor mu; `pwdLastSet` 0 kalıyor mu? Samba kaynak kodu ve MS-ADTS "evet" der; Microsoft örneği LDAPS değil Kerberos mühürlemedir. ⊞ → gerçek Windows AD'de evet ([docs/11](11-dogrulama-notlari.md) W1)
- [ ] Bind hata kodları: `pwdLastSet = 0` (773), süresi dolmuş (701), pasif (533), kilitli (775); parola yanlışken hangisi görünür? Operatör diline çevrilecek hata tablosunun girdisi.
- [ ] Henüz replike olmamış hesapla başka bir DC'ye girişte dönen hata (docs/09 "aynı dakika giriş" notunun kanıtı). ⊞ → lab'da tek DC olduğu için ölçülemedi ([docs/11](11-dogrulama-notlari.md) W9)
- [x] Windows'ta `<GUID=…>` modify, modifyDN ve delete hedefi olarak kabul ediliyor mu? Worker'ın kuralı değişmez (gerçek DN ile yazar); yalnızca bilgi. ⊞ → üçü de kabul ediliyor ([docs/11](11-dogrulama-notlari.md) W2–W4)
- [ ] OU'lar arası taşıma için en az delegasyon ([docs/05](05-active-directory.md#servis-hesabı-yetkileri) tablosunun kanıtı); kesin delegasyon adımları buradan docs/09'a yazılır.
- [ ] Servis hesabında "Unexpire-Password" hakkı yokken parola sıfırlaması `pwdLastSet`'i kendiliğinden 0 yapıyor mu (MS-SAMR öyle yazar)? Evet ise [ADR-019](decisions/019-ilk-parola-teslimi.md)'un "işaret kapalı" modu bu hakkı delegasyon tablosuna ekletir. ⊞ → lab servis hesabı `Domain Admins` üyesi olduğu için ölçülemedi; yetkisi devredilmiş ayrı hesap gerekiyor ([docs/11](11-dogrulama-notlari.md) W10)
- [ ] Seçilen Samba sürümünde `LDAP_MATCHING_RULE_IN_CHAIN` doğru sonuç veriyor mu ve ne kadar sürüyor (4.18'de boş sonuç raporu var)?
- [ ] `ldap3` #156: domain kökünden alt ağaç aramasında `searchResRef` paniği `EntriesOnly` ile önleniyor mu? `objectGUID` hangi haritadan çıkıyor?
- [x] AD'nin ad parçası içeren parolayı reddetmesi ve worker'ın yeniden üretmesi ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)); Samba bu kontrolü yapmaz. ⊞ → gerçek AD `0000052D` ile reddediyor ([docs/11](11-dogrulama-notlari.md) W5)

**Zimbra (ve isteğe bağlı Carbonio CE)** — v1 dışı, "Ek Hedef Sistem: Zimbra" bölümünde cevaplanır ([ADR-090](decisions/090-zimbra-v1-sonrasina-alindi.md); eski planda Faz 1d'ydi)
- [ ] `locked` hesaptan otomatik yanıt MTA'dan gerçekten çıkıyor mu (kodda durum kontrolü yok, gözlenmedi)?
- [ ] Kullanıcının kendi yönlendirmesi, filtresi ve otomatik yanıtı `ModifyAccountRequest` ile temizlenip **aynen** geri yazılabiliyor mu ([ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md))?
- [ ] Parolasız `CreateAccountRequest` ile açılan hesaba AD parolasıyla giriliyor mu ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md))?
- [ ] OpenSicil'in admin hesabı ayrı, yerel doğrulamalı alan adındayken yönetilen alan adında bütün işlemleri yapabiliyor mu? `zimbraAuthMechAdmin = zimbra` seçeneği çalışıyor mu?
- [ ] Çok sunuculu kurulumda `locked` sonrası oturumların düşme gecikmesi (hesap önbelleği); tek sunucuda "bir sonraki istek"tir.
- [ ] Kaynaktan ya da üçüncü taraf derlenmiş 10.1'de `zimbraIsDelegatedAdminAccount` + `grantRight` ile oluşturulan domain admin, kullanılan bütün Admin API işlemlerini yapabiliyor mu? Evet ise global admin yerine seçenek olarak eklenir ([docs/06](06-zimbra.md#admin-hesabı)).
- [ ] Dolu adres bir takma ad ya da dağıtım listesiyken de `CreateAccountRequest` `account.ACCOUNT_EXISTS` dönüyor mu ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md); hesap için kaynak kodundan doğrulandı, [docs/11](11-dogrulama-notlari.md) Z17)?
- [ ] Carbonio CE: aynı istekler 7071'de çalışıyor mu; 6071 proxy'si `/service/admin/soap`'ı geçiriyor mu (F-41)?

**Faz 1b — Keycloak**
- [ ] AD'de pasifleştirilen kullanıcının açık Keycloak oturumu ve token yenilemesi ne kadar sürüyor ("Always Read Enabled Value From LDAP" açık ve kapalıyken)?
- [ ] `WRITABLE` federasyonda `pwdLastSet = 0` kullanıcısı parola değiştirme ekranını görüp `unicodePwd`'yi LDAPS üzerinden yazabiliyor mu (Samba'ya karşı)?

**Faz 5**

- [x] N-03 yük testi: 20.000 kullanıcılı lab Samba AD'de gece mutabakatı, 2.000 kimliklik değişiklik seti ve o sırada tek kimlik işleminin bekleme süresi (N-11). Ölçüldü (2026-10-02, `scripts/load-lab.sh`, AD; Zimbra yarısı Zimbra bölümünde): mutabakat 8 sn, 2.000 kimliklik set 5 dk (6,6 iş/sn), set sürerken tek kimlik işi 1 sn, arama 86 ms; tek sıra yetti, eşzamanlılık açılmadı ([ADR-108](decisions/108-n03-olcumu-tek-sira-yetti.md)). Zimbra mutabakatında liste üyeliğinin okunma yolu da burada ölçülür: [docs/06](06-zimbra.md) yalnızca hesap başına `GetAccountMembershipRequest`'i yazar, 20.000 hesapta 20.000 çağrıdır; yetmezse liste başına üye okuyup tersine çevirmek denenir. Tek sıra yetmezse eşzamanlılık açılır ve boyutlandırma tablosu güncellenir ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md), [ADR-047](decisions/047-worker-tek-sirada.md)).

### 🟡 Kurulumda kararlaştırılacak

- [x] Frontend yaklaşımı: derleme adımsız statik değil, htmx + Tailwind + backend şablonları ([ADR-064](decisions/064-frontend-htmx-tailwind.md))
- [x] Ekran dili: TR ve EN seçimi baştan var; renk teması açık/koyu ([ADR-100](decisions/100-renk-paleti-catppuccin-yerine-slate-blue.md)), arayüz fontu CaskaydiaMono Nerd Font ([ADR-067](decisions/067-arayuz-dili-tema-font.md))
- [x] OIDC akışı: backend'de yürütülür, oturum PostgreSQL'de ([ADR-065](decisions/065-oidc-akisi-backend.md))
- [x] TLS: `compose.yaml` referansında nginx'te sonlanır, dış proxy/Ingress'te düz HTTP moduna alınabilir ([ADR-066](decisions/066-tls-nginxte-sonlanir.md))
- [x] `ldap3`/`sqlx` TLS özelliği: rustls + `ring` arka ucu, ikisinde de aynı yığın ([ADR-069](decisions/069-kurulum-crate-ve-arac-secimleri.md))
- [x] Crate seçimleri: AEAD `chacha20poly1305`, blind index `hmac`+`sha2`; parola üretimi ve telefon ayrıştırma elle (ayrı crate yok) ([ADR-069](decisions/069-kurulum-crate-ve-arac-secimleri.md))
- [x] Lab Zimbra ortamı: üçüncü taraf 10.1 derlemesi + ayrı Carbonio CE VM'i; kesin kaynak Faz 1d'de ([ADR-069](decisions/069-kurulum-crate-ve-arac-secimleri.md))
- [x] Migration'ı şema sahibi rolüyle çalıştıran adım: aynı imajın `migrate` alt komutu, tek seferlik container (compose servisi / Kubernetes Job) — [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)
- [x] Metrik ucuna erişim: Bearer token; IP listesi Kubernetes'te çalışmaz — [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)
- [x] Test, format, lint, kapsam: `cargo test` / `cargo fmt --check` / `cargo clippy -D warnings` / `cargo llvm-cov` / `cargo audit` ([ADR-069](decisions/069-kurulum-crate-ve-arac-secimleri.md))
- [x] Lab imajları: Samba `quay.io/samba.org/samba-ad-server` — proje sürüm numarasıyla etiket yayımlamıyor (`v0.2`–`v0.9`, `latest`/`nightly`/`default-*` kanalları), bu yüzden Faz 1c'de `default-fedora-amd64` kanalının (Samba 4.24.7 içeren) dijestine kilitlendi ([ADR-074](decisions/074-samba-lab-imaji-digest-sabitleme.md), ADR-069'un yerine bu kısmı geçer); Keycloak `26.7.5` ([ADR-069](decisions/069-kurulum-crate-ve-arac-secimleri.md), [ADR-027](decisions/027-test-stratejisi-ve-lab.md))

### 🟠 İlk commit'ten önce

- [x] Kalıcı ürün adı ve grup önekleri: **OpenSicil**, AD grup öneki `OpenSicil-*` ([ADR-063](decisions/063-kalici-urun-adi-opensicil.md))
- [x] Lisans — AGPL-3.0

### ⚪ Sonra

- [ ] SMS sağlayıcısı (F-26 ile)

## Önerilen faz sırası

Kesin kutucuklar kurulumda `docs/todo.md`'ye yazılır. Her fazın son kutucuğu güvenlik ve test kapanışıdır. Alt fazlarla birlikte ondan fazla kapanış vardır; kapanış komutları (test, kapsam, lint, bağımlılık ve imaj taraması, sır taraması) kurulumda **tek bir betiğe** bağlanır, kapanış kutucuğu o betiği ve docs/07 listesini çalıştırır.

1. **Altyapı** — üç alt faz:
   - **1a. İskelet:** Compose, nginx, backend, worker, PostgreSQL; `/api/health`; süreç sözleşmesi ([ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md)): SIGTERM işleme, `worker-health` ve `migrate` alt komutları, şema sürümü kontrolü, `stop_grace_period`, aynı `compose.yaml`'dan servis alt kümesi başlatma (iki sunuculu düzenler; `--no-deps` ya da `required: false`); `docs/09-kurulum.md` açılır ve her kutucuk kendi ön koşulunu ekler.
   - **1b. Giriş:** OIDC girişi ve altı yetki; oturum süresi ve ayrılmış ya da askıdaki operatörün her isteğinin reddi ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md), [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)); frontend kararı bu alt fazdan önce kurulumda verilir.
   - **1c. Lab kod olarak:** `compose.lab.yaml` ile Samba AD ve Keycloak ([ADR-027](decisions/027-test-stratejisi-ve-lab.md)); `ldap3` doğrulaması bu lab'a karşı. **midPoint denemesi burada yapılır** ([ADR-002](decisions/002-hazir-urun-yerine-gelistirme.md)): lab hazırdır, motor henüz yazılmamıştır, batık maliyet yoktur; sonuç ADR-002'nin altına yeni karar olarak yazılır.
   - **1d. Zimbra keşfi** — _bu madde geçersiz: Zimbra v1'den sonraya alındı, keşif kutucukları `docs/todo.md`'nin sonundaki Zimbra bölümüne taşındı ([ADR-090](decisions/090-zimbra-v1-sonrasina-alindi.md)); aşağıdaki gerekçe tarihsel kayıttır._ Lab Zimbra'sı (sürüm ve ortam kararı kurulumda) ayağa kalkar; JSON Admin API ile oturum, `CreateAccount`, `ModifyAccount`, `DeleteAccount` ve liste üyeliği `curl` ile denenir; OpenSicil admin hesabının yönetilen alan adının dışında durması ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)) burada kurulur; Zimbra'ya ait lab soruları (yukarıda) burada cevaplanır. Lab sorularının sekizi Zimbra'dır ve cevapları ADR-045/049 ile docs/06 ön koşullarını değiştirebilir; Faz 4'e bırakılırsa üç fazlık tasarım cevapsız varsayım üstünde durur. OSE 10 paketinin olmaması lab kurulumunu başlı başına risk yapar.
2. **Kayıt ve model:** İlk kutucuk veritabanı rolleri ve denetim tablosunun `current_user` kolonu ([ADR-015](decisions/015-veritabani-rolleri.md), [ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md)); güvenlik modelinin temeli, geç kalırsa her tablo geri açılır. Sonra kimlik, departman ağacı, rol, yönetilen kapsam ve yasaklı grup listesi, katalog tabloları (AD'den dolması 3a'dadır: connector'ın okuma yolu orada yazılır, burada test verisiyle dolar), denetim kaydı, kimlik numarası şifreleme, ortak ayarların iki serviste de ortam değişkeninden okunması ([ADR-039](decisions/039-ortak-ayarlar-env.md)). Denetim kaydındaki worker işlem türleri (yıkıcı, verme, ilk parola, öznitelik) burada sabitlenir; 3f'teki sayaçlar bu türleri sayar, tür sonradan eklenirse eski satırlar sayılamaz ([ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)). Olması gereken durum fonksiyonu burada saf modül olarak yazılır ve tablo testleriyle gelir; 3a, 3f ve Faz 5 aynı fonksiyonu çağırır, ikinci bir fark hesabı yazılmaz. Bu fazın sonunda ekranda çalışan bir şey yoktur; demo yapılmaz.
3. **AD provisioning** — altı alt faza bölünür, her birinin kendi kapanışı vardır. Önceki plandaki 3a çok büyüktü (motor + adlar + eşleme + rol ekranları + önizleme + kişi sayfası); ilk çalışan dilim onun sonuna kalıyordu. Dilim öne alındı:
   - **3a. İlk dilim:** motor ve kuyruk (tekilleştirme, öncelik, 5 sn yoklama, kirayla iş alma ve işlemden önce niyet satırı — [ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md), sorguya dayalı zamanlayıcı ve türetilen durum — [ADR-028](decisions/028-worker-zamanlamasi.md), [ADR-038](decisions/038-kimlik-durumu-turetilir.md); worker **tek sırada** çalışır, v1'de eşzamanlılık ayarı yoktur — [ADR-047](decisions/047-worker-tek-sirada.md), [ADR-051](decisions/051-okuma-seridi.md); müdahaledeki iş açık sayılır, bağlantı hatası deneme tüketmez — [ADR-052](decisions/052-uygulanamayan-fark.md); connector yazma çağrılarının tek noktadan geçmesi ve kuru çalıştırma modu — [ADR-054](decisions/054-kuru-calistirma-ve-yedekten-donus.md)); AD connector'ın okuma yolu ve katalog (OU, grup, SID ile yasaklı grup, iç içe üyelik sorgusu), `ldap3` kuralları ve açılış kontrolleri (kapsam DN'leri, `msDS-LogonTimeSyncInterval` — [ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md), [ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)); şablonla kullanıcı adı, yalnızca veritabanı ve AD çakışması; varsayılan eşleme; tek `add` ile hesap aç ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)), etkinleştir (geçiş kuralı — [ADR-032](decisions/032-elle-pasiflestirme-korunur.md)), pasifleştir; kimlik kayıt formu ve kişi sayfası, operatör dilinde hata, hedefteki fark görünümü (F-12). Rol ekranı yoktur; temel rol ve hedef varsayılanı seed ile girilir. **İlk çalışan dilim budur:** kayıt → AD'de pasif hesap → ekranda "açıldı".
   - **3b. Roller ve adlar:** rol ve departman ekranları, katalogdan seçim, grup ve OU yönetimi; elle kullanıcı adı, bağlı olmayan hesapla ve kullanılmış adla çakışmada müdahale, serbest bırakma ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md), [ADR-035](decisions/035-kullanilmis-ad-duz-metin-serbest-birakma.md), [ADR-042](decisions/042-ayni-kisi-farkli-anahtar.md)); eşleme izinli listesi ve "sadece boşsa yaz" ([ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md), [ADR-034](decisions/034-sam-upn-esleme-disi-ve-bossa-yaz.md)); belirsiz bileşen kuralı ([ADR-040](decisions/040-motor-belirsiz-degere-dokunmaz.md)). Etki önizlemesi burada **yoktur**: model farkı eşikle aynı fonksiyondur ve 3f'te bir kez yazılır; rol düzenlemesi 3b'de doğrudan yayımlanır.
   - **3c. Yaşam döngüsü:** işe giriş, görev değişikliği (önce ekleme, sonra çıkarma — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)), planlı ve acil ayrılış, geri alma (yıkıcı — [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)), hedefte doğrulanan kayıt iptali ([ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)), tarihli askı ([ADR-053](decisions/053-tarihli-aski.md)); `ayrıldı`dan her çıkışın geri alma sayılması, askı bitişinin son gün olması, `accountExpires`'ın temizlenmesi ([ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md)); geçmiş tarihli bitiş; yönetici ayrılışında astların etkin yöneticisi (F-38, [ADR-041](decisions/041-astlarin-yoneticisi-turetilir.md)); süreli ek rol (F-37); ayrılışta gecikmeli parola sıfırlama ([ADR-033](decisions/033-ayrilista-parola-gecikmesi.md)); yaklaşan bitişler (F-36). Bağımlı kimlikler için iş açma ([ADR-052](decisions/052-uygulanamayan-fark.md)) burada gelir.
   - **3d. İlk parola:** AEAD ile şifreli teslim ([ADR-036](decisions/036-ilk-parola-aead.md)), yardım masası yetkisi ve ilk girişte değiştirme ayarı (F-11, [ADR-019](decisions/019-ilk-parola-teslimi.md)), kullanılmamış hesap kontrolü ([ADR-046](decisions/046-kullanilmamis-hesap-lastlogontimestamp.md)); "kaydet ve ilk parolayı ver" akışı, teslim ekranı ve okunabilir parola biçimi, N-13 ölçümü ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)). **İK'nın ürünü ilk kez kullanabildiği an bu alt fazın sonudur;** ürünün hedef sahnesi (kişi masada, İK kaydeder, parolayı söyler) ilk kez burada uçtan uca gösterilir.
   - **3e. Tekil sahiplenme ve gözlem modu:** formdaki mevcut hesap ipucuyla gözlem modunda bağlama, fark görünümü ve tek kimlik için yönetime alma; bağlantı kökeni ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md), [ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)) — motor gerçek hesaplara karşı sınanır; CSV ve toplu yönetime alma Faz 6'da kalır. Önceki planda 3c'nin bir yan cümlesiydi; kendi kapanışını hak edecek kadar büyüktür ve 3f'teki eşik gözlemdekileri saymak için buna dayanır.
   - **3f. Fren ve onay:** saatlik sayaçlar ve acil kota worker'da (yıkıcı, verme, ilk parola; iş sayaçlara karşı bütün — [ADR-050](decisions/050-verme-sayaci-ve-is-butunlugu.md)); onay anında yeniden önizleme ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)); model farkı fonksiyonu, ondan hem **etki önizlemesi** hem değişiklik seti eşiği (ekleme dahil, gözlemdekiler hariç — [ADR-037](decisions/037-esik-ekleme-islemlerini-sayar.md), [ADR-043](decisions/043-esik-uygulanacak-farki-sayar.md)), taslak, ikinci yönetici onayı ve zaman kilidi backend'de ([ADR-031](decisions/031-degisiklik-seti-sahneleme.md), [ADR-026](decisions/026-degisiklik-seti-onayinda-zaman-kilidi.md)); bekleme sebebinin ekranda gösterilmesi (F-12).
4. **Zimbra** — _sıra değişti: v1'den sonra, beş fazın ardından ([ADR-090](decisions/090-zimbra-v1-sonrasina-alindi.md)); içeriği aynı._ Kayıtlı yanıt stub'ı ve gerçek Zimbra ([ADR-027](decisions/027-test-stratejisi-ve-lab.md)); connector, parolasız hesap açma ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)); COS ve liste kataloğu; yaşam döngüsü karşılıkları; ayrılışta kullanıcı yönlendirmesi ve filtresinin temizlenmesi, gecikmeli otomatik yanıt, ayarla açılan yönlendirme ([ADR-045](decisions/045-ayrilan-postasi-yonlendirme.md), [ADR-049](decisions/049-ayrilan-postasi-kullanici-yonlendirmesi-ve-gecikme.md)). Ad üretimindeki Zimbra çakışma kontrolü (hesap, takma ad, liste) burada eklenir: 3a'da yalnızca veritabanı ve AD'ye bakan üretim adımı Zimbra'ya da bakacak şekilde genişler; Zimbra erişilemiyorsa kontrol atlanır, ad üretimi ve AD hesabı beklemez, olası çakışma Zimbra işinde müdahale olur ([ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md)).
5. **İşletme:** Okuma şeridi ([ADR-051](decisions/051-okuma-seridi.md)) ve onun ilk işi olarak mutabakat raporu (3a'daki kişi sayfası fark görünümünün yönetilen kapsam geneline tek sayfalı aramayla uygulanması; ayrı fark kodu yazılmaz), hedef sistem başına saklama ve "silinmeyi bekleyenler" listesi ([ADR-024](decisions/024-hedef-sistem-basina-saklama-suresi.md)), metrik ucu (hedef sistem başına son başarılı bağlantı dahil), N-03 yük testi ve ölçüme göre worker eşzamanlılığı; docs/09 eşlemesinin tek node'lu bir Kubernetes kümesinde bir kez doğrulanması (N-14).
6. **Mevcut kurum:** CSV içe aktarma (kolon ve ipucu kuralları — [ADR-023](decisions/023-ice-aktarma-ipucu-ve-kolon-kurallari.md), [ADR-030](decisions/030-ayrilisi-geri-alma-yikici.md)), sicil no değişimi önerisi ve tarihli ek rol kuralı ([ADR-055](decisions/055-netlestirmeler-onay-csv-operator-parola.md)), toplu sahiplenme ve toplu yönetime alma ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md); tekil sahiplenme 3e'de). İlk beş faz sıfırdan kurulan kurum için tek başına kullanılabilir; **mevcut personeli olan kurum için "v1 hazır" bu fazın sonudur.**

Ayrılış, işe girişle aynı fazdadır: unutulmuş eski bir hesap, geç açılmış yeni bir hesaptan daha tehlikelidir.
