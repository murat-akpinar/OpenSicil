# Proje Haritası

Yer imleri. Bir dosya eklendiğinde, taşındığında, silindiğinde veya yeni feature marker açıldığında aynı commit içinde güncellenir. Kısa tutulur.

## Dizinler
- `README.md` → Türkçe giriş noktası: konum, mimari diyagramları, ne-neden-nasıl karar tabloları, yol haritası; mimariyi ya da faz sırasını değiştiren karar burayı da günceller
- `README_ENG.md` → aynı içeriğin İngilizcesi; `README.md` güncellenince birlikte güncellenir
- `docs/00-kavramlar.md` … `docs/08-gereksinimler.md` → tasarım: kavramlar, mevcut çözümler, mimari, rol modeli, yaşam döngüsü, AD, Zimbra, güvenlik, gereksinimler ve açık sorular
- `docs/09-kurulum.md` → kurulum ön koşulları, boyutlandırma, ayarlar; env veya ön koşul ekleyen her kutucuk günceller
- `docs/10-saha-notlari.md` → gerçek kurumda bugün işlerin nasıl yürüdüğü (olgu → tasarımdaki karşılığı → sonuç) henüz bilinmeyenler, son taramanın ve dağıtım gözden geçirmesinin sahne sonuçları ve bilerek yazılmayanlar; sahne yürütmeli gözden geçirmenin girdisi ve kaydı
- `docs/11-dogrulama-notlari.md` → teknik iddiaların birincil kaynakla sınanmış hali (Zimbra, AD, Samba, `ldap3`, Keycloak, dağıtım): sonuç, alıntı, kaynak; yeni iddia önce buraya soru olarak girer
- `docs/decisions/` → karar kayıtları (001–066; 025'in yerine 041 geçti)
- `nginx/nginx.conf` → yönlendirme kuralları, güvenlik header'ları
- `<backend/src/...>` → <ne işe yarar>

## Feature indeksi
Koddaki `--- START FEATURE: <ad> ---` markerlarının karşılığı. Aramak için:
`grep -rn "FEATURE: <ad>" --exclude-dir=.git --exclude-dir=tmp .`

| Feature | Nerede |
|---|---|
| <user-login> | <backend/src/auth/> |

## Ortak yardımcılar
Yeni bir şey yazmadan önce buraya bak. Aynı işi yapan varsa tekrar yazma.

| Ne yapar | Nerede |
|---|---|
| <istek doğrulama şeması> | <backend/src/common/validation> |
