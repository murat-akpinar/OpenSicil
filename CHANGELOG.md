# Değişiklik Günlüğü

## Yayınlanmamış

### Bakım

- Proje dokümantasyonu ve ilk yapılandırma dosyaları
- Docker iskeleti ve compose yapilandirmasini olustur
- Faz 3a kapanışı — güvenlik ve test
- Faz 3b kapanışı — güvenlik ve test
- Faz 3c kapanışı — güvenlik ve test
- Faz 3d kapanışı — güvenlik ve test
- Lab dosyaları git'ten çıkarıldı, geliştirici makinesinde kalır

### Dokümantasyon

- Readme rozetlerini yenile, lisans kaydını sadeleştir
- Kurulum kararlarını tamamla, todo.md şemasını oluştur
- Faz 1a dagitim ve migration komutlarini doldur
- MidPoint denemesi yapıldı, ADR-002 kararı degismedi
- Gösterge panelini v1 kapsamına ekle (ADR-076)
- Arayüz kabuğu kutucuğu işaretlendi, durum notu
- Zimbra v1'den sonraya alındı, todo.md'nin sonuna taşındı (ADR-090)
- Ürün adı OpenSicil, eski OpenIAM kalıntıları düzeltildi (ADR-063)
- Görsel yenileme ve gerçek Windows AD kutucukları açıldı
- Iki yeni kutucuk — müdahale listesi ve sahiplenmede AD kişi alanları
- Tekil sahiplenmede sicil ve telefon kutucuğu
- Devreye alma akışına DRY_RUN=false adımı ve gözlem modu notu (ADR-106 madde 6)
- Hogwarts AD'de bekleyen ölçümler alındı (whenCreated 29/29, sicil employeeNumber'dan, manage_diff 3 fark)
- TC özniteliği kurulum ayarı olarak cevaplandı, Zimbra bölümü kullanıcı kararıyla beklemede
- AD'den geri dolum ve ayrılışta silmeme kutucukları
- AD'de farklı listesine sicil ve ölçüm notu
- MAP'ten Inter satırları çıktı (ADR-116)
- Rol kutucuğuna saha ölçümü ve oturum devri notları
- README v1 sonrasına göre güncellendi (ADR sayısı, faz sırası, geri dolum)
- Mutabakat tarama saati ayarı ve iki yeni kutucuk todo'ya girdi
- Todo.md sıraya girdi, açık kutucuklar yapılış sırasında

### Düzeltmeler

- Arayüz kökü 404 veriyordu, kabuğun / bağlantısı rotasızdı
- Değişen statik varlıklar artık ETag ile doğrulanıyor
- Mutabakat sayaç kutuları panel yapısına alındı; iki AD ortamı belgelendi
- .env yedekleri artık git'e girmiyor
- Toplu sahiplenmede departman artık AD'den gelir, form alanı isteğe bağlı
- Hogwarts seed'i AD'deki branş departmanlarını da kuruyor
- V1 sonrası kod taraması — rol parolası DDL kaçışı, süresi geçen oturum/OIDC satırları, operatör reddinde çoklu eşleşme, parola üretiminde mod sapması, nginx HSTS
- Onay kutuları her yerde aynı boyda ve metin satırına hizalı
- Tablolarda iç dikey kaydırma kalktı, başlık üst barın altına yapışıyor
- Sayfa taramasındaki görsel hatalar ve raporlar kapağındaki yanlış bağlantı
- Giriş sonrası yönlendirme, boş filtre değeri, favicon ve panelde taşma
- Iş sonucundaki NUL baytı yazılamayınca iş sonsuza dek çalışır kalıyordu

### Geri alınanlar

- Gövde fontu CaskaydiaMono kalır

### Testler

- Faz 1a güvenlik ve test kapanışını tamamla
- Faz 1b güvenlik ve test kapanışını tamamla
- Faz 1c güvenlik ve test kapanışını tamamla
- Faz 2 güvenlik ve test kapanışını tamamla
- Kullanılmamış hesap kuralı (ADR-046) testleri ve docs/07 maddesi
- Faz 3e kapanışı — güvenlik ve test
- Arayüz kabuğu kapanışı — güvenlik ve test
- Fren ve onay fazı kapanışı — güvenlik ve test
- Gerçek Windows AD doğrulaması — üç ⊞ sorusu cevaplandı
- Mutabakat gerçek Windows AD'ye karşı doğrulandı (29 Hogwarts hesabı)
- Hogwarts test AD'si için departman ve rol modeli seed'i
- Arayüz mockup bölümü kapanışı — güvenlik ve test
- Giriş bölümü kapanışı — güvenlik ve test, belgeler ADR-095'e göre
- Fazlar arası işlerin kapanışı — güvenlik ve test, Faz 4 öncesi
- N-03 yük ölçümü — 20.000 hesaplık lab betiği, tek sıra yetti, eşzamanlılık açılmadı (ADR-108)
- N-14 — docs/09 eşlemesi kind kümesinde doğrulandı; k8s-lab manifestleri ve betiği, nginx conf küme DNS'i + FQDN
- Faz 4 kapanışı — güvenlik ve test (198/98 test, kapsam %94,65/%93,51, imaj temiz, e2e-lab, yığında duman testi)
- Faz 5 kapanışı — güvenlik ve test, v1 hazır (208/99 test, kapsam %94,22/%93,10, nginx imajına apk upgrade, e2e-lab, duman testi)
- Arayüz bölümünün kapanışı — güvenlik ve test

### Yeniden düzenleme

- Arayüz dosyaları frontend/ dizinine taşındı

### Özellikler

- Backend, worker ve nginx iskeletini ayağa kaldır
- Backend ve worker'a SIGTERM ve şema hazırlık kontrolü ekle
- Yerel bootstrap hesabı ve Yapılandırma sayfasını ekle
- OIDC yonetim girisi ve alti yonetim yetkisi
- Lab compose'una Samba AD ekle
- Veritabanı rolleri ve denetim tablosu (current_user kolonu)
- Kimlik modeli tabloları ve yönetilen kapsam kararı (ADR-077)
- Katalog tabloları ve hedef sistem kaydı
- Denetim kaydı alanları ve worker işlem türleri
- Kimlik numarası AEAD ve blind index ile şifrelenir
- Ortak ayarlar iki serviste de .env'den okunur (ADR-039)
- Olması gereken durum fonksiyonu saf modül olarak (ADR-038)
- Ayrılmış ve askıdaki operatör her istekte reddedilir (ADR-059)
- Iş kuyruğu, kira, niyet satırı ve tek sıralı motor döngüsü
- Bağlantı hatası deneme tüketmez, erişilemeyen hedefin işleri bekletilir (ADR-052)
- Connector yazmaları tek noktadan geçer, kuru çalıştırma modu (ADR-054)
- AD connector okuma yolu ve katalog (LDAPS, yasaklı gruplar, lab seed)
- AD açılış kontrolleri: kapsam DN çözümü ve msDS-LogonTimeSyncInterval (ADR-060)
- Şablonla kullanıcı adı üretimi ve DB/AD çakışma çözümü (ADR-011/022/035)
- Tek add ile AD hesabı açma, etkinleştirme ve pasifleştirme (ADR-057)
- Kimlik kayıt formu, kişi sayfası ve hedefteki fark görünümü (F-12, ADR-078)
- Zamanlayıcı ve uçtan uca lab doğrulaması: kayıt → AD'de pasif hesap → ekranda açıldı (ADR-079)
- Rol, departman ve hedef sistem ekranları, katalogdan seçim (ADR-080)
- Elle kullanıcı adı, çakışma müdahalesi ve kullanılmış ad serbest bırakma (ADR-081)
- Öznitelik eşlemesi: izinli liste, sadece boşsa yaz, yönetici eşlemesi ve ekran (ADR-082)
- Hesap açılsın=hayır mevcut hesabı silmez, kayıp hesap ve belirsiz yönetici dokunulmaz (ADR-040)
- Görev değişikliği: üyelik farkı, OU taşıma, kimlik düzenleme ve ek roller (ADR-050/083)
- Ayrılış, geri alma, kayıt iptali, tarihli askı ve hesap silme (ADR-030/048/053/059/084)
- Yönetici ayrılışında astlara iş ve etkin yönetici notu (ADR-041, F-38)
- Süreli ek rol tarih dolunca zamanlayıcıyla kalkar (ADR-020, F-37)
- Ayrılışta gecikmeli parola sıfırlama, zamanlayıcı penceresi (ADR-033)
- Yaklaşan bitişler listesi (F-36)
- Ilk parola teslimi — AEAD ile şifreli, bir kez gösterim, yardım masası yetkisi, ilk girişte değiştirme ayarı (ADR-019/036/046/085)
- Kaydet ve ilk parolayı ver tek adımı, teslim ekranı, N-13 ölçümü (ADR-056)
- Mevcut hesap ipucuyla gözlem modunda sahiplenme (ADR-018, ADR-086)
- Gözlem farkı ve tek kimlik için yönetime alma (ADR-087)
- Arayüz kabuğu — derlenmiş Tailwind CSS, Catppuccin teması, self-host font (ADR-088)
- TR/EN dil seçimi — gömülü TOML, lang.t(), tercih oturumda (ADR-089)
- Saatlik fren sayaçları ve acil kota worker'da (ADR-091)
- Etki önizlemesi ve değişiklik seti eşiği (ADR-092)
- Eşiği aşan değişiklik seti taslak olarak bekler, ikinci yönetici onaylar (ADR-093)
- Onay anında etki önizlemesi yeniden hesaplanır (ADR-055)
- Bekleme sebebi, kalan süre ve onaylayacak grup ekranda (F-12)
- Okuma şeridi — katalog yenileme ayrı görevde, sayaca dokunmaz (ADR-094)
- Gezinme sol sidebar'a taşındı
- Tek oturum mekanizması, yerel giriş kalıcı break-glass oldu (ADR-095)
- Arayüz görsel yenilemesi — yüzey, tipografi, ikon ve boş durumlar
- Arayüz mockup düzenine geçti — gösterge paneli ana sayfası ve üst bar araması
- Mutabakat taraması ve ekranı — AD'deki hesaplar artık görünüyor
- Menü yapısı, panel düzeni ve toplu sahiplenme
- Sahiplenilmeyen AD hesapları için panel ve liste yönlendirmesi
- Devreye alma kartı — kurulumun dört adımı panelde
- Hata sayfaları ve nginx istek sertleştirmesi
- AD bind giriş kapısı — yetkiler AD gruplarından okunuyor
- Müdahale bekleyen işler listesi ve panel şeridinde kısa yol
- Sahiplenmede AD'den e-posta, telefon ve sicil alanları
- Tekil sahiplenmede sicil ve telefon AD'den gelir
- Rol ve departman listeleri bölümlere ve gerçek ağaca geçti
- Rol ve departman detayında gruplanmış üyelikler ve miras satırı
- Toplu sahiplenmede tümünü seç kutusu (ADR-103 madde 3)
- Tanımsız yer tutucu rol — rolü atanmamış sayacı, liste filtresi ve yönetime alma kapısı (ADR-103 madde 4/5)
- Toplu sahiplenmede başlangıç tarihi AD whenCreated'dan gelir (ADR-103 madde 6)
- TC kimlik no özniteliği kurulum ayarı — tarama şifreli okur, toplu sahiplenme doğrulayıp yazar (ADR-106 madde 5)
- Rol ve departman adresleri okunur ada döndü, sayısal adres 301 (ADR-107)
- Mutabakat gece koşusu ve bulgudan yeniden uygula (F-13)
- Panelde tarih aralığı filtresi ve rol kırılımı halkası (ADR-076)
- Silinmeyi bekleyenler listesi ve silme onayı (ADR-024)
- Metrik ucu — Prometheus metni, Bearer token, worker durum satırı ve kuru çalıştırma şeridi (F-19)
- CSV ile toplu kimlik içe aktarma — kolon ve ipucu kuralları, önizleme, tek transaction, eşiği aşan dosya şifreli sahneleme ve onay (F-17)
- CSV'de sicil no değişimi önerisi — kimlik no eşleşen satır mevcut kaydı günceller, onay kutusu ve denetimde önce/sonra (ADR-055)
- Toplu yönetime alma — okuma şeridinde gözlem farkı hesabı, seçim ekranı, eşik ve onay (ADR-018/043/051)
- Altın oran paleti ve yuvarlak tema düğmesi
- Kişi sayfasında olaylar, işler ve ek roller operatör diline çevrildi
- Mutabakat sonrası AD'den boş alan dolumu
- Cam yüzeyli turkuaz tema ve daraltılabilir kenar menüsü
- Gövde fontu Inter, monospace yalnızca veri alanlarında
- Sayaç kartları kendi renginde, kıvılcım ve değişim rozetiyle
- Etkinlik akışı olay kategorisine göre renklenir
- Rol bölümlerinde kişi/atama sayısı, sayı kolonunda dolu değer kalın
- Panelde sütunlar eşitlendi, eğilim çubukları gün başına yan yana
- Raporlar kapağında özet kutuları ve satır rozetleri
- Uygulamalar sayfasında özel açılır liste oku ve hizalı yenileme satırı
- Ayarlarda alan genişliği sınırlandı, bölüm menüsü üst bara göre yapışıyor
- Personel listesinde departman/rol filtresi ve sıralanabilir kolonlar
- Halka grafiğinin dilimleri arasına boşluk
- Roller sayfası tür renkli bölümler ve kart ızgarası
- Departman ağacı seviye renkli tek parça satır şeridi
- Mutabakatta "AD'de farklı" listesi ve toplu "AD'dekini al"
- Katalog OU'ları kapsam köklerinin altından keşfedilir
- AD fark listesine departman girer, alım hedefe iş açar
- Kayıp hesabın bağlantısı kaldırılabilir, kimlik yeniden sahiplenilir
- AD fark listesine rol girer, yer tutucu rol unvandan dolar
- Mutabakat kendi menü maddesi olur, Raporlar'dan çıkar

