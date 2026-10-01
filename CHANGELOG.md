# Değişiklik Günlüğü

## Yayınlanmamış

### Bakım

- Proje dokümantasyonu ve ilk yapılandırma dosyaları
- Docker iskeleti ve compose yapilandirmasini olustur
- Faz 3a kapanışı — güvenlik ve test
- Faz 3b kapanışı — güvenlik ve test
- Faz 3c kapanışı — güvenlik ve test
- Faz 3d kapanışı — güvenlik ve test

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

### Düzeltmeler

- Arayüz kökü 404 veriyordu, kabuğun / bağlantısı rotasızdı
- Değişen statik varlıklar artık ETag ile doğrulanıyor
- Mutabakat sayaç kutuları panel yapısına alındı; iki AD ortamı belgelendi
- .env yedekleri artık git'e girmiyor
- Toplu sahiplenmede departman artık AD'den gelir, form alanı isteğe bağlı

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

