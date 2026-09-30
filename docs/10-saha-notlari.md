# 10 — Saha Notları

> Yaşayan dosyadır. Gerçek bir kurumda **bugün işlerin nasıl yürüdüğünü** olgu olarak tutar. Tasarım gözden geçirmesinin girdisidir: soyut eleştiri yeni kural üretir, gerçek olgu ise tasarımın bir varsayımını sınar.

Buradaki kurum bir **örnektir**, ürünün tek müşterisi değildir. Her olgu dört sonuçtan birine bağlanır ([docs/08](08-gereksinimler.md#kuruma-özel-sorular-ürüne-nasıl-yansıdı) ile aynı ilke):

| Sonuç | Anlamı |
|---|---|
| **karşılanıyor** | Tasarım bu olguda zaten doğru çalışıyor; yalnızca not düşülür |
| **ön koşul / doküman** | Ürün değişmez; kurulum dokümanı kontrolü ve adımı yazar |
| **ayar** | Kurumdan kuruma değişir; kurulum ayarı olur |
| **karar** | Tasarım bu olguda yanlış ya da sessiz; ADR yazılır |

Yeni bir olgu hatırlandığında önce buraya **olgu olarak** yazılır ("bizde şöyle yapılıyor"), çözüm olarak değil. Sonra tek soru sorulur: *tasarım bu olguda adım adım ne yapıyor?*

## Olgular

| # | Olgu (bugün nasıl yapılıyor) | Tasarımdaki karşılığı | Sonuç |
|---|---|---|---|
| 1 | Kişi işe gelir; İK ad, soyad, kimlik no, telefon ve birimi girer, "parolanız bu" der; kişi o dakikadan itibaren çalışabilmelidir. Ürünün hedef sahnesi budur | Tek adımlı "kaydet ve ilk parolayı ver", okunabilir parola, N-13 ([ADR-056](decisions/056-ise-baslama-gunu-akisi.md)). Replikasyon ve ilk giriş kanalı ürünün dışında ([docs/09](09-kurulum.md#aynı-dakika-giriş)) | karar (yazıldı) |
| 2 | BT, AD'de hesabı **elle** açar; sonra Zimbra admin panelinden **aynı adla** mailbox'ı elle açar | Ürünün var olma nedeni. Mevcut hesaplar CSV + sahiplenmeyle bağlanır; adlar aynı olduğu için ipucu `kullanıcıadı` ve `kullanıcıadı@alanadı` ([docs/09](09-kurulum.md#bugün-hesapları-elle-açan-kurum)) | karşılanıyor |
| 3 | Zimbra'da hesap açılırken **parola belirlenmez**; adlar aynı olduğu için giriş LDAP üzerinden AD'de doğrulanır | [ADR-009](decisions/009-parola-yonetimi.md) ön koşulu sağlanıyor. Bu pratik tasarımı düzeltti: OpenIAM de Zimbra hesabını **parolasız** açar; yerel parolası olmayan hesapta geri düşecek bir şey yoktur ([ADR-057](decisions/057-birincil-kaynak-dogrulamasi.md)) | karar (yazıldı) |

## Henüz bilinmeyenler

Bir sonraki gözden geçirmeden önce doldurulursa bulgular tahmine değil olguya dayanır. Bilinmeyen boş bırakılır; uydurulmaz.

- [ ] `zimbraAuthFallbackToLocal` değeri; webmail'de "parolayı değiştir" açık mı ([docs/06](06-zimbra.md#ön-koşullar-kurumun-işi))
- [ ] Zimbra sürümü ve edisyonu
- [ ] Kaç DC, kaç site; şube var mı (aynı dakika giriş — [docs/09](09-kurulum.md#aynı-dakika-giriş))
- [ ] Domain bilgisayarı olmayan personel var mı (saha, yalnızca webmail) — "ilk girişte değiştir" ayarı ([ADR-019](decisions/019-ilk-parola-teslimi.md))
- [ ] Kullanıcı adı biçimi ve aynı adlı ikinci kişide ne yapılıyor ([ADR-011](decisions/011-kullanici-adi-ve-eposta.md))
- [ ] Yeni gelene bugün hangi gruplar, hangi OU, hangi dağıtım listeleri veriliyor; kim karar veriyor (rol tasarımının ham verisi)
- [ ] Ev dizini, logon script, profil yolu kullanılıyor mu (`homeDirectory`, `scriptPath` izinli listede yok — [ADR-029](decisions/029-esleme-hedef-oznitelikleri-izinli-liste.md))
- [ ] Kimlik numarası ya da sicil no AD'de bir özniteliğe yazılıyor mu ([ADR-010](decisions/010-kisisel-veri-kimlik-no-telefon.md))
- [ ] Ayrılanın hesabı bugün ne zaman, kim tarafından kapatılıyor; mailbox'a ne oluyor; unutulan oluyor mu
- [ ] Görev ya da birim değiştirenin eski grupları kaldırılıyor mu
- [ ] AD grubundan okunmayan, elle hesap açılan uygulamalar (ERP, kartlı geçiş, muhasebe)
- [ ] Gruplar `CN=Users` içinde mi, bir OU'da mı ([docs/09](09-kurulum.md#active-directory))
- [ ] Soyadı değişen personelin hesabı ve adresi bugün elle yeniden adlandırılıyor mu, takma ad mı ekleniyor ([docs/09](09-kurulum.md#ayarlar-ve-öneriler) "soyad değişimi" notu; F-25 v2'de)
- [ ] AD hesabı olup mailbox'ı olmayan (ya da tersi) personel var mı; yönetime almada rolün "hesap açılsın" ayarı buna göre verilir ([docs/04](04-yasam-dongusu.md#mevcut-personel-içe-aktarma-ve-sahiplenme))
- [ ] Uzun izne (doğum, askerlik, ücretsiz izin) çıkanın hesabı bugün kapatılıyor mu; kapatılmıyorsa askı bu kurumda kullanılmaz ([ADR-053](decisions/053-tarihli-aski.md))
- [ ] OpenIAM'in çalışacağı sunucunun ve PostgreSQL'in yedeğini kim, ne sıklıkla alacak ([docs/09](09-kurulum.md#yedekten-dönüş))

## Sahne yürütme sonuçları (son tarama, 2026-09-19)

On sahne tasarımın içinden adım adım yürütüldü (form alanı → kayıt → iş → connector işlemi → sayaç → ekran → kişi → süre). Bir sonraki tarama buradakileri yeniden saymaz; yalnızca yeni olgu ya da yeni sahne ekler.

| # | Sahne | Takıldığı adım | Sonuç |
|---|---|---|---|
| 1 | Aynı gün işe başlama (kişi masada, şubede çalışacak) | Zimbra erişilemezken ad üretilemiyor, AD hesabı da açılamıyordu (9. sahneyle birlikte). Şube DC'sine replikasyon ve ilk giriş kanalı ürünün dışında | karar: [ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md) · ön koşul: [docs/09](09-kurulum.md#aynı-dakika-giriş) |
| 2 | Önceden kayıt, kişi başlamadan vazgeçti | Yok: kayıt iptali hedefte doğrulanır, hesaplar hemen silinir, ad yakılmaz | karşılanıyor |
| 3 | Stajyer kadroya geçti; birim ve sicil no değişti | Bitiş tarihi kaldırılınca `accountExpires`'a ne olduğu yazılı değildi; eski staj bitişinde sessiz kilitlenme | karar: [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md) 4. madde |
| 4 | Doğum iznine çıkış ve dönüş | "Dönüş tarihi" ile "askı bitişi (günün sonu)" aynı alan sanılıyordu; kişi döndüğü gün giremezdi | karar: [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md) 3. madde |
| 5 | Cuma akşamı acil çıkış; kişi İK operatörüydü | Ayrılmış operatör yalnızca yazmada durduruluyordu; 8 saat kişisel veri okuyabiliyor, IdP oturumu sürüyorsa yeniden girebiliyordu | karar: [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md) 1. madde · doküman: yönetim grubu üyeliği elle kaldırılır, geri almada yetki sessizce dönerdi ([docs/09](09-kurulum.md#kimlik-sağlayıcı-oidc)) |
| 6 | Ayrıldı, 2 ay sonra geri döndü | İleri tarihli dönüşte `ayrıldı → bekliyor → aktif` yolu yıkıcı sayacın yanından geçiyordu | karar: [ADR-059](decisions/059-netlestirmeler-operator-geri-alma-aski-bitisi-accountexpires.md) 2. madde |
| 7 | BT, AD'de elle değişiklik yaptı | Grup, taşıma, kapatma, silme karşılanıyor. Sessiz kalan: **elle yeniden adlandırma** (v1'de soyad değişimi yok; eşlenmiş `mail` eski adrese geri yazılır) | doküman: [docs/09](09-kurulum.md#ayarlar-ve-öneriler) "soyad değişimi" notu |
| 8 | İlk kurulum günü: 300 kişi, hesaplar iki tarafta elle ve aynı adla | Kuru modda delegasyon **sınanamaz** (yazma yok); doküman sınanır diyordu. İpucu kolonunu BT hazırlar | doküman: [docs/09](09-kurulum.md#ayarlar-ve-öneriler) kuru çalıştırma satırı |
| 9 | Zimbra 4 saat kapalıyken 5 kişi işe başladı | 1. sahneyle aynı bağ: posta sunucusu arızası Windows girişini kesiyordu | karar: [ADR-058](decisions/058-zimbra-erisilemezken-ad-uretimi.md) |
| 10 | OpenIAM sunucusu çöktü, dünkü yedekten dönüldü | 3. adım "sahiplenin" diyordu, kuru mod sahiplenmeyi reddeder. Yedeğin var olduğu varsayılıyordu, ön koşul yazılı değildi | doküman: [docs/09](09-kurulum.md#yedekten-dönüş), [docs/09](09-kurulum.md#sunucu) |

**Varsayım denetimi** (yanlışsa sessiz bozulanlar): `lastLogonTimestamp`'ın domain'de kapatılmış olması (`msDS-LogonTimeSyncInterval = 0`) → worker açılışta reddeder ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)); `employeeNumber`'ın confidential işaretlenmemiş olması → kontrol komutu [docs/09](09-kurulum.md#active-directory); hibritte pasif OU'nun Entra Connect kapsamı dışında olması, break-glass yöneticinin olmaması, `zimbraAuthFallbackToLocal = TRUE` → docs/09'da tek komut ya da tek soruyla kontrol, ürün denetlemez. Gürültülü bozulanlar (LDAPS, delegasyon, kapsam DN'leri, 7071, admin hesabı, groups claim) bulgu değildir.

### Bilerek yazılmayanlar (son tarama)

Görüldü, tartıldı, kural yapılmadı. Yeni bir olgu gerektirirse yeniden açılır.

- **Worker'ın açılışta Zimbra alan adı ayarlarını okuması** (`zimbraAuthMech`, geri düşme): tek komutla kontrol docs/09'da var; ayrılanı OpenIAM zaten `locked` yapar, kalan risk yalnızca AD'de elle pasifleştirilen hesaptır.
- **Break-glass yöneticinin worker tarafından denetlenmesi:** IdP'deki yerel hesap worker'dan görünmez, yanlış alarm üretir.
- **[ADR-048](decisions/048-kayit-iptali-hedefte-dogrulanir.md)'in "AD bağlantısı yoksa `zimbraLastLogonTimestamp`" dalı:** parola AD'de doğrulandığı için yalnızca Zimbra hesabı olan kimlik pratikte giriş yapamaz; dal neredeyse ölü koddur. Faz 3c'de silinmesi değerlendirilir, zararı yok.
- **Kayıt formu varsayılanları:** hedef sahne beş alan sayar (1. olgu), form sekiz ister (birincil rol, çalışma tipi, başlangıç tarihi). Başlangıç = bugün ve çalışma tipi = kadrolu varsayılanı Faz 3a arayüz işidir; birincil rolü İK seçer (unvan).
- **İpucu kolonunun üretimi:** İK dökümünde AD kullanıcı adı yoktur; BT birleştirir ya da mutabakattaki "yönetilmeyen hesap"tan ekranda eşleştirilir. Ürün otomatik eşleştirmez ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md)).
- **`lastLogonTimestamp` replikasyon penceresi:** kabul edildi, her DC'ye sorulmaz ([ADR-060](decisions/060-lastlogontimestamp-on-kosulu-acilista-dogrulanir.md)).
- **F-31 numara boşluğu:** atıf yok, numaralar yeniden dizilmez.

## Dağıtım sahneleri (2026-09-19)

Soru: plan küçük, orta ve büyük ölçekte, Docker'da ve Kubernetes'te rahat çalışır mı? Sahneler ortam → süreç → kuyruk → hedef → operatörün gördüğü şeklinde yürütüldü. Kimlik sayısına dair ölçek kuralları ([ADR-016](decisions/016-hedef-olcek-ve-olcekte-calisma.md), [ADR-047](decisions/047-worker-tek-sirada.md), [ADR-051](decisions/051-okuma-seridi.md)) yeniden sayılmadı.

| # | Sahne | Takıldığı adım | Sonuç |
|---|---|---|---|
| 11 | Küçük kurum, tek VM, compose; gece host yeniden başladı, worker bir ayrılışın ortasındaydı | İşin kime kalacağı yazılı değildi; "çalışıyor" kalan işi tekilleştirme açık sayar, ayrılan için bir daha iş açılmaz | karar: [ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md) 1. madde |
| 12 | Sürüm yükseltme (`docker compose up -d --build`, Kubernetes rollout) | Handler'sız PID 1 SIGTERM'i yok sayar, 10/30 sn sonra SIGKILL: her yükseltme bir işi keser. Kubernetes varsayılan stratejisi yeni worker'ı eskisi ölmeden başlatır; sayaçların yarışsızlığı tek sürece dayanıyordu | karar: [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md) 1. madde, [ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md) 3. madde |
| 13 | İki sunucu: backend DMZ host'unda, worker yönetim bölgesinde, PostgreSQL kurumun kümesi | Veritabanı bağlantısı ağdan geçiyor; `sqlx` varsayılanı `prefer` TLS yoksa sessizce düz metne düşer. `compose.yaml`'daki `db` bağımlılığı servis alt kümesi başlatmayı engelliyordu. `.env` iki kopya; [ADR-039](decisions/039-ortak-ayarlar-env.md)'un "aynı dosya, aynı komut" varsayımı yok. Anahtar farkı gürültülü, sayaç sınırı farkı sessiz | karar: [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md) 6.–7. madde · doküman: [docs/09](09-kurulum.md#dağıtım-biçimleri) |
| 14 | Büyük kurum, Kubernetes: backend iki kopya, worker pod'u node değiştiriyor | Worker'ın sağlığı neyle ölçülür (port yok)? Oturum bellekteyse giriş döngüsü. Migration ve metrik ucu sorularının birer seçeneği (elle komut, IP listesi) burada çalışmıyor. "7071 yalnızca worker'ın IP'sine" kuralındaki adres sabit değil (ağ eklentisine göre node ya da pod IP'si); Ingress 1 MB üstü CSV'yi reddediyor | karar: [ADR-061](decisions/061-dagitim-sozlesmesi-compose-ve-kubernetes.md) · ön koşul: [docs/09](09-kurulum.md#kubernetese-özel-ön-koşullar) |
| 15 | Kurumun PostgreSQL kümesinde planlı geçiş, worker iş ortasında | AD işlemi uygulandı, denetim satırı yazılamadı; ikinci çalışma fark bulmaz, işlem denetim kaydına ve sayaca hiç girmez. Eşzamansız replikada son bağlantı satırının kaybı ise karşılanıyor: ad çakışması müdahaleye düşer ([ADR-022](decisions/022-kullanici-adi-elle-giris-ve-cakisma.md)) | karar: [ADR-062](decisions/062-is-kirasi-ve-yarida-kalan-is.md) 2. madde |
| 16 | 50.000 kimlikli kurum temel rolü düzenledi; "pod sayısını artıralım" | Hızın sınırı tek sıra ve saatlik verme sayacıdır (500/saat ile 50.000 kimlik günler sürer); ikisi de bilerek konmuş frendir, altyapı değil. Yatay ölçek bir şey kazandırmaz | karşılanıyor; doküman: [docs/09](09-kurulum.md#dağıtım-biçimleri), ölçüm Faz 5 N-03 |

**Bilerek yazılmayanlar (dağıtım):** Helm chart ve operator (ilk gerçek Kubernetes kullanıcısına kadar); sırları dosyadan okuma (`*_FILE`; Secret ortam değişkenine verilebiliyor, [ADR-006](decisions/006-sirlar-env.md) tetikleyicisi duruyor); etkin/yedek worker (N-05 v1 hedefi değil, kira deseni hazır); ikinci worker'ı reddeden nabız tablosu (kazancı saatte bir işlik aşım); ortak ayar parmak izi karşılaştırması (tek ConfigMap ve doküman yeter).

