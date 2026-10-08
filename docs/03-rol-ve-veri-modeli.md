# 03 — Rol ve Veri Modeli

Bu dosya mantıksal modeli anlatır. Tablo ve kolon tasarımı kodla birlikte migration'da yapılır.

## Varlıklar

| Varlık | Ne tutar |
|---|---|
| **Kimlik** | Gerçek bir kişi. Hiçbir hedef sisteme bağlı olmayan kalıcı bir iç ID'si vardır |
| **Departman** | Ağaç: isteğe bağlı üst departman ve kod (ADR-017). Yetki öğeleri ve tek değerli varsayılanlar taşır; alt departmanlar miras alır. Şirket ve lokasyon ayrı varlık değil, ağacın seviyeleridir |
| **Rol** | Türü (temel, birincil, ek), unvanı, hedef sistem başına yetki öğeleri ve ayarlar |
| **Hedef sistem** | AD veya Zimbra bağlantısı; sistem varsayılanları ve öznitelik eşlemeleri. Tür değil kayıttır: model birden fazla AD'yi engellemez, v1 ayarı ve ekranı bir AD ve bir Zimbra ile sınırlıdır (F-33) |
| **Katalog öğesi** | Hedef sistemden içe alınan OU, grup, dağıtım listesi veya COS. Değişmeyen ID ile tutulur |
| **Hesap bağlantısı** | Kimliğin bir hedef sistemdeki hesabı: objectGUID veya zimbraId; modu (**yönetilen** veya **gözlem**, ADR-018); ilk parolanın verilip verilmediği; o sistemde en son uygulanan kimlik durumu (ADR-032); ayrılışta parolanın sıfırlandığı işareti (ADR-033, ADR-046); ayrılışta yazılan posta yönlendirmesi ve kullanıcının temizlenen kendi yönlendirmesi, filtresi ve otomatik yanıtı (ADR-045, ADR-049); **köken** (`açıldı` ya da `sahiplenildi`; yönetime almada değişmez, kayıt iptalinin koşuludur — ADR-048). Sadece worker yazar (ADR-015) |
| **Değişiklik seti** | Operatörün tek bir kayıt işlemi: kim, ne zaman, neyi, kaç kimliği etkiledi, onay durumu. Eşiği aşan rol, departman ve içe aktarma düzenlemeleri yayımlanana kadar **taslak** olarak bekler; motor taslağı okumaz (ADR-031) |
| **İş** | "Bu kimliği şu sistemde olması gereken duruma getir". Durum, deneme sayısı, son hata |
| **Denetim kaydı** | Değiştirilemez olay kaydı ([docs/07](07-guvenlik-ve-kvkk.md)) |
| **Kullanılmış ad kaydı** | Bir daha verilmeyecek kullanıcı adları ve e-posta adresleri; ad, tür, tarih ve gerekçeyle düz metin tutulur, Sistem yöneticisi serbest bırakabilir (ADR-035) |

Kimliği olan rol veya departman silinemez; alt departmanı olan departman da silinemez. Önce kimlikler ve alt departmanlar taşınır.

## Kimlik şeması (v1)

Alan kümesi sabittir. Kuruma özel ek alanlar v2'dedir. Bir alanın hedef sistemde hangi özniteliğe yazılacağını kurulum belirler (ADR-012).

| Alan | Zorunlu | Not |
|---|---|---|
| Ad | Evet | Birden fazla ad olabilir: "Mehmet Ali" |
| Soyad | Evet | |
| Ulusal kimlik numarası + ülke | Kuruluma göre, varsayılan hayır | Şifreli saklanır, ekranda maskeli görünür. TR için kontrol haneleri doğrulanır (ADR-010) |
| Sicil no | Hayır; CSV içe aktarmada evet | Kurumun kendi personel numarası; tekil olmalı. İçe aktarmanın eşleştirme anahtarıdır (ADR-018). İK ekrandan değiştirebilir; çalışma tipi dönüşümünde yeni kayıt açılmaz (ADR-042) |
| Mevcut hesap ipucu | Hayır | Sahiplenilecek AD kullanıcı adı ve Zimbra adresi. Doluysa motor hesap açmaz, sahiplenmeyi bekler. Sahiplenme kapalıyken formda gösterilmez |
| Cep telefonu | Hayır | E.164 olarak saklanır: `+905321234567` |
| Departman | Evet | Ağacın herhangi bir seviyesi |
| Birincil rol | Evet | Tam olarak bir tane |
| Ek roller | Hayır | Sıfır veya daha fazla; her birinin isteğe bağlı bitiş tarihi vardır (ADR-020) |
| Yönetici | Hayır | Başka bir kimlik; AD'de `manager` olarak yazılabilir. Yönetici de kayıtlı bir kimlik olmalıdır; mevcut personel içe aktarmayla kaydedilir (F-17). `ayrıldı` durumundaki kimlik seçilemez. Alan yönetici ayrılınca **değişmez**; astların **etkin yöneticisi** ayrılanın devir yöneticisinden türetilir (ADR-041); bu hedefte hesabı yoksa `manager` yazılmaz, temizlenmez (ADR-040) |
| Devir yöneticisi | Hayır | Ayrılış formunda girilir, ayrılanın kaydında durur; varsayılan ayrılanın yöneticisi. Astların etkin yöneticisi ve ayrılan postasının yönlendirme hedefi (ADR-041, ADR-045) |
| Çalışma tipi | Evet | Kadrolu, sözleşmeli, stajyer, dış kaynak |
| Başlangıç tarihi | Evet | |
| Bitiş tarihi | Kadrolu dışında evet | Kadroluda ayrılış kaydedilince girilir. Saklanan değer **bitiş anı**dır: planlıda bitiş gününün ertesi 00:00, acil ayrılış ve kayıt iptalinde şimdi (ADR-038) |
| Askı başlangıcı, askı bitişi | Hayır | İkisi de isteğe bağlı tarih; ileri tarihli askı girilebilir. Askı bitişi **iznin son günüdür**, hesaplar ertesi gün 00:00'da açılır ve form bu günü gösterir (ADR-059); tarihsiz askı elle kaldırılana kadar sürer (ADR-053) |
| Kullanıcı adı | Sistem üretir; isteğe bağlı elle girilir (ADR-022); sahiplenmede hedeften okunur | Oluştuktan sonra değişmez |
| E-posta adresi | Sistem üretir; sahiplenmede hedeften okunur | v1'de değişmez. Alan adı departman zincirinden gelir |
| UPN | Sistem üretir (kullanıcı adı + sonek); sahiplenmede hedeften okunur | Değişmez; eşlenemez (ADR-034) |
| Durum | Türetilir, saklanmaz | Başlangıç, bitiş anı, askı tarihleri, iptal işareti ve `silindi_anı`'ndan saf fonksiyonla (ADR-013, ADR-038, ADR-053) |

**Parola alanı yoktur** (ADR-009).

**Mükerrer kişi:** Kimlik numarası varsa tekillik kesindir. Yoksa, normalleştirilmiş ad ve soyadı silinmemiş bir kimlikle aynı olan kayıtta uyarı gösterilir; engel değildir.

### AD'den geri dolum

Kaynak OpenSicil'dir; AD'de zaten duran veriyi ise elle yeniden girmek gerekmez (ADR-112). Değer her zaman son mutabakat taramasının bulgusundan gelir — backend AD'ye bağlanmaz, dolum hedefe yazmaz ve fren sayacı harcamaz (ADR-051).

| Durum | Ne olur | Nerede |
|---|---|---|
| Alan OpenSicil'de **boş**, AD'de dolu | Tarama bitince kendiliğinden dolar: sicil, cep, e-posta, kullanıcı adı ve yönetici (ADR-129). Sicil başka bir kimlikte duruyorsa yazılmaz, geri kalanı yazılır. Silinmiş kimlik dolmaz (ADR-024'ün temizliği geri alınmaz) | Worker, `identity.fields_filled` denetim satırı |
| Rol yer tutucu `Tanımsız` | AD'deki `title` bir rolün unvanıyla tek anlamlı eşleşiyorsa o role geçer (ADR-120 madde 5) | Worker, gece |
| İki tarafta dolu, değişiklik **yalnızca AD'de** (ADR-138) | Tarama AD'nin şimdiki halini önceki taramadaki haliyle karşılaştırır. AD değişmiş ve OpenSicil'deki değer AD'nin eski halinde duruyorsa yeni değer kendiliğinden kimliğe yazılır. Alanlar: ad, soyad, sicil (yazım birebir: `9` ile `00000000009` farktır), cep (E.164), departman ve unvan (ağaçta tek düğüme çözülmeyen alınmaz). AD'de boşaltılan alan OpenSicil'den silinmez. Kimlik için tek iş açılır | Worker, her taramada (15 dakikada bir); `identity.field_taken`, `source: ad_auto`, önce/sonra |
| İki tarafta **dolu ve farklı**, kim değiştirdi bilinmiyor | Aynı alan iki tarama arasında iki tarafta da değiştiyse ya da fark eşitleme devreye girmeden önce de varsa otomatik yazma yok. Mutabakat ekranındaki "AD'de farklı" listesi kişi · alan · iki değer gösterir; operatör satır seçip AD'dekini alır. Alanlar: ad, soyad, sicil, cep, departman, rol unvanı. Ad, soyad, departman ya da rol alınırsa iş açılır (görünen ad, OU ve gruplar ondan türer) | Backend, `hr`/`admin`; `identity.field_taken` önce/sonra |
| TC kimlik no | Yapılandırma'da öznitelik adı verilmedikçe hiç okunmaz. Verildiyse tarama değeri şifreli saklar; dolum operatörün bastığı toplu eylemdir, çünkü yazma AEAD + blind index ister ve worker blind index üretmez. Kontrol hanesinden geçmeyen ve başka kişide kayıtlı numara atlanır; denetime değer girmez | Backend, mutabakat ekranı "TC kimlik no AD'den" |

Dolmayanlar: UPN (eşlenemez, ADR-034) ve kullanıcı adı/e-posta **farkı** — ikisini OpenSicil üretir ve silmede kullanılmış ad olarak yakar; AD'dekini "almak" o kaydı atlardı.

## Rol modeli

Ayrıntılı gerekçe: ADR-007.

### Katmanlar

Bir kimliğin olması gereken yetkileri dört kaynağın birleşimidir (v2'de beşincisi eklenir):

```
Temel rol ∪ Departman ve ataları ∪ Birincil rol ∪ Ek roller   (v2: ∪ süreli kişisel istisnalar)
```

| Katman | Kaç tane | Örnek |
|---|---|---|
| **Temel rol** | Kurulumda tek; ayrılmamış her kimliğe uygulanır | `GG-Internet`, `herkes@` |
| **Departman** | Kimlik başına bir; ataları miras yoluyla katılır | `GG-Muhasebe-Paylasim`, `muhasebe@`; üst departman "Ankara"dan `GG-Ankara-Yazici` |
| **Birincil rol** | Kimlik başına bir | Sistem Uzmanı |
| **Ek rol** | Kimlik başına sıfır veya daha fazla; isteğe bağlı bitiş tarihli | Nöbet Ekibi, İSG Temsilcisi, devir süresince eski rol |

"Sistem Uzmanı" rolü her departmanda aynı kalır; departmana özel şeyler departmandan, lokasyona veya şirkete özel şeyler üst departmandan gelir. Böylece "Sistem Uzmanı – Muhasebe" ya da "Muhasebe – Ankara" gibi roller açılmaz. Matris yapıda (her ilde ayrı bir Muhasebe servisi) bütün muhasebecilere ortak yetki departmandan değil rolden gelir.

### Çok değerli ve tek değerli ayarlar

| Tür | Örnek | Birden fazla kaynaktan gelirse |
|---|---|---|
| **Çok değerli** | AD grupları, Zimbra dağıtım listeleri | Birleşim alınır |
| **Tek değerli** | Hesap açılsın mı, AD OU, Zimbra COS, unvan, e-posta alan adı, UPN soneki | Öncelik sırası uygulanır |

Tek değerli ayarların öncelik sırası:

```
Birincil rol  →  Departman  →  Üst departmanlar (yakından köke)  →  Hedef sistem varsayılanı
```

İlk dolu olan kazanır. Temel rol ve ek roller tek değerli ayar **taşıyamaz**; ekran buna izin vermez. Çakışma bu yüzden tasarım gereği oluşamaz.

**"Hesap açılsın mı" yalnızca hesap yokken okunur.** Bağlı hesabı olan kimlikte `hayır` hesabı kapatmaz ve silmez; hesap yaşam döngüsüne göre yönetilmeye devam eder ve mutabakatta "rol hesap öngörmüyor" bilgi bulgusu görünür (ADR-040).

### Örnek

**Hedef sistem varsayılanları**
- AD: hesap açılsın = evet, OU = `OU=Personel,DC=example,DC=local`
- Zimbra: hesap açılsın = evet, COS = `default`

**Temel rol:** AD `GG-Internet`; Zimbra `herkes@example.com`

**Departman "Bilgi İşlem":** AD `GG-BT-Paylasim`; Zimbra `bt@example.com`

**Birincil rol "Sistem Uzmanı"**
- Unvan: Sistem Uzmanı
- AD OU: `OU=SistemUzmanlari,OU=Personel,DC=example,DC=local`
- AD grupları: `GG-Sistem-Uzmanlari`, `GG-VPN`, `OpenBerat-IT`
- Zimbra COS: `teknik`

**Ek rol "Nöbet Ekibi":** AD `GG-Nobet`; Zimbra `nobet@example.com`

**Bilgi İşlem'de çalışan, Nöbet Ekibi'nde de olan bir Sistem Uzmanı için sonuç:**

| Sistem | Olması gereken |
|---|---|
| AD | `OU=SistemUzmanlari,…` altında aktif hesap, unvan "Sistem Uzmanı". Gruplar: `GG-Internet`, `GG-BT-Paylasim`, `GG-Sistem-Uzmanlari`, `GG-VPN`, `OpenBerat-IT`, `GG-Nobet` |
| Zimbra | COS `teknik`. Listeler: `herkes@`, `bt@`, `nobet@` |

## Katalog

- Rol ve departman ekranında grup, OU, liste veya COS elle yazılmaz; **katalogdan seçilir**.
- Katalog, worker'ın hedef sistemi okumasıyla dolar. Sadece yönetilen kapsamın içindeki nesneler katalog olabilir (ADR-014).
- Her öğe değişmeyen ID'siyle tutulur (AD'de objectGUID). Ad ve DN sadece gösterim içindir ve katalog yenilendikçe güncellenir. Bir OU yeniden adlandırılsa veya grup taşınsa bile roller bozulmaz.
- Hedef sistemde artık bulunamayan öğe **kayıp** olarak işaretlenir. Onu kullanan roller ekranda uyarı gösterir, motor bu öğe için işlem üretmez. v1'de kayıp öğe her rolde elle değiştirilir; tek işlemde yenisiyle değiştirme v2'dedir (F-39).
- Ayrıcalıklı gruplar ve OpenSicil'in kendi yönetim grupları kataloğa **hiç alınamaz**.

## Motor neye dokunur

Yalnızca iki koşul birlikte sağlandığında işlem yapılır:

1. **Hesap bağlı ve yönetiliyor:** OpenSicil'in açtığı ya da sahiplenip yönetime aldığı hesap. **Gözlem** modundaki bağlantıda motor farkı hesaplar ve gösterir ama uygulamaz (ADR-018).
2. **Öğe katalogda:** Grup veya liste katalogda kayıtlı.

Sonuçları:
- Katalogdaki bir gruba elle eklenmiş, OpenSicil'e bağlı olmayan kişilere dokunulmaz. Mevcut personel bu sayede ilk gün kesinti yaşamaz.
- Bağlı bir hesaba katalog dışından elle eklenmiş gruplar kaldırılmaz, mutabakat raporunda görünür.
- Bağlı bir hesaba katalogdaki bir grup elle eklenmişse ve rolünde yoksa, o kimlik için worker bir sonraki kez çalıştığında kaldırılır. Katalogdaki öğeler için kaynak OpenSicil'dir. Kişi sayfası bu üyelikleri "elle eklenmiş, sonraki işte kaldırılacak" olarak gösterir; kalıcı olacaksa role ya da bitiş tarihli ek role alınır.
- Hedefte elle pasifleştirilmiş bağlı hesap geri açılmaz; etkinleştirme yalnızca kimlik durumu geçişinde yapılır, hesap mutabakatta "elle pasifleştirilmiş" olarak görünür (ADR-032).
- Hesaplanamayan bileşene dokunulmaz: hedefte bulunamayan bağlı hesap yeniden açılmaz ("kayıp hesap" bulgusu; Recycle Bin'den geri alınırsa GUID aynıdır ve bağlantı canlanır), bu hedefte hesabı olmayan etkin yöneticinin `manager` özniteliği temizlenmez (ADR-040).

## Değişiklik seti ve etki önizlemesi

- Operatörün her kaydı bir değişiklik setidir. Bir CSV içe aktarma, bir departmanın taşınması ve toplu yönetime alma da tek birer değişiklik setidir.
- Rol veya departman düzenlemesi ve içe aktarma önce **taslak** olarak yazılır; motor taslağı okumaz (ADR-031).
- Etki önizlemesi **model farkıdır**: etkilenen her kimlik için yayımlanmış tanımla ve taslakla olması gereken durum karşılaştırılır. Örnek: "42 kimlik etkilenecek: 42 kişiye `GG-X` eklenecek, 42 kişiden `GG-Y` kaldırılacak." Hedef sistem okunmaz; sayı tahmin değil kesindir. Hedefteki gerçek fark mutabakatın işidir.
- Yetki veya hesap durumu farkı (yetki öğesi ekleme ya da çıkarma; hesap açma, silme, etkinleştirme, pasifleştirme; OU taşıma; ayrılışı geri alma — ADR-030, ADR-037) üreten kimlik sayısı eşiği aşmıyorsa taslak kaydedilirken yayımlanır ve işler açılır. Yalnızca özniteliği değişen kimlikler sayılmaz. Bağlantısı gözlem modunda olan, ipucu bekleyen ya da o hedefte hesabı olmayacak kimlikler de sayılmaz: taslak onlarda işlem üretmez; önizleme bunları "gözlemde: M kimlik" diye ayrı gösterir (ADR-043). Kendisi onay olan işlemler (silinmeyi bekleyenlerin onayı, elle pasifleştirilmiş hesabın etkinleştirilmesi) eşiğe girmez, saatlik sayaca girer.
- Aşıyorsa set "onay bekliyor"dur (ADR-014): onay taslağı yayımlar, red taslağı atar; model değişmemiştir. Onay ekranı model farkını **onay anında yeniden hesaplar**; taslak beklerken role atanan kimlikler sayıya girer ve fark kaydedilendekinden farklıysa ekran bunu söyler (ADR-055). Onayı bir Sistem yöneticisi verir; başlatanın kendisi de olabilir (ADR-132). Kural backend'de uygulanır ve operatör hatasına karşıdır; ele geçirilmiş backend'e karşı fren worker'daki saatlik sayaçlardır (ADR-016).
- Toplu yönetime alma istisnadır: farkı worker gözlenen duruma göre hesaplar ve mod bayrağını worker çevirir (ADR-018).
- İçe aktarmanın bekleyen satırları onaya kadar taslak tablosunda durur ve kimlik alanlarıyla aynı korumaya tabidir: kimlik numarası şifreli (ADR-010), onay ya da red sonrası satırlar silinir. ADR-018'in "dosya saklanmaz" kuralı ham dosya içindir.
- Tarihlerden doğan zamanlanmış geçişler (başlangıç, bitiş, saklama sonu) yeni bir değişiklik seti değildir; eşiğe değil saatlik sınıra tabidir.
