# Değişiklik Günlüğü

## Yayınlanmamış

### Bakım

- Proje dokümantasyonu ve ilk yapılandırma dosyaları
- Docker iskeleti ve compose yapilandirmasini olustur

### Dokümantasyon

- Readme rozetlerini yenile, lisans kaydını sadeleştir
- Kurulum kararlarını tamamla, todo.md şemasını oluştur
- Faz 1a dagitim ve migration komutlarini doldur
- MidPoint denemesi yapıldı, ADR-002 kararı degismedi
- Gösterge panelini v1 kapsamına ekle (ADR-076)

### Testler

- Faz 1a güvenlik ve test kapanışını tamamla
- Faz 1b güvenlik ve test kapanışını tamamla
- Faz 1c güvenlik ve test kapanışını tamamla
- Faz 2 güvenlik ve test kapanışını tamamla

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

