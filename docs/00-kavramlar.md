# 00 — Kavramlar

OpenSicil bir **IGA** ürünüdür: kişinin kurumdaki yaşam döngüsünü (işe giriş, görev değişikliği, ayrılış) yönetir ve role göre hedef sistemlerde hesap ve yetki açar, değiştirir, kapatır. Bu dosya ürünü anlatırken kullanılan sektör terimlerini toplar. Ürün, rakip veya iş ilanı ararken kullanılan kelimeler bunlardır.

## Çatı kategoriler

| Terim | Açılımı | Anlamı | OpenSicil'de |
|---|---|---|---|
| **IAM** | Identity and Access Management | Kimlik ve erişimle ilgili her şey: giriş, yetki, SSO, provisioning, denetim | Genel çatı. Doğru ama çok geniş |
| **IGA** | Identity Governance and Administration | "Kim neye erişmeli, neden, ne zamana kadar" sorusunun yönetimi | **Ürünün kategorisi** |
| **AM** | Access Management | Giriş anı: SSO, MFA, OIDC/SAML | Kapsam dışı. Keycloak gibi bir IdP'nin işi |
| **IAP / ZTNA** | Identity-Aware Proxy / Zero Trust Network Access | Her istekte "bu kimlik bu uygulamaya erişebilir mi" kararı | Kapsam dışı. OpenBerat bu kategoride |
| **PAM** | Privileged Access Management | Ayrıcalıklı hesapların (Domain Admin, root) kasalanması ve oturum kaydı | Kapsam dışı |

## Yaşam döngüsü

| Terim | Anlamı |
|---|---|
| **JML** (Joiner–Mover–Leaver) | İşe giriş, görev/departman değişikliği ve ayrılış süreçlerinin toplu adı |
| **Identity Lifecycle Management** | JML'in kimlik kaydı üzerinden, tarih ve duruma bağlı olarak yönetilmesi |
| **Authoritative source** | Kimlik verisinin doğru kabul edildiği kaynak. Büyük şirketlerde İK sistemi; OpenSicil v1'de kendi kişi kaydı, CSV ile beslenebilir ([ADR-018](decisions/018-ice-aktarma-ve-sahiplenme.md)) |
| **Provisioning** | Hedef sistemde hesap açmak ve yetki vermek |
| **Deprovisioning** | Hesabı kapatmak ve yetkiyi almak. En çok atlanan ve güvenlik açısından en kritik kısım |
| **Birthright access** | Kişinin sadece işe girdiği ve rolü nedeniyle otomatik aldığı yetkiler |
| **Privilege creep** (yetki birikmesi) | Görev değiştiren kişinin eski yetkilerinin kaldırılmaması |
| **Orphan account** (sahipsiz hesap) | Hedef sistemde olup kimlik kaydında karşılığı olmayan hesap |
| **Rehire** | Ayrılmış bir kişinin yeniden işe alınması |

## Yetki modeli

| Terim | Anlamı | OpenSicil'de |
|---|---|---|
| **RBAC** | Yetkilerin kişiye değil role bağlanması | Çekirdek model ([ADR-007](decisions/007-rol-modeli.md)) |
| **ABAC** | Departman, lokasyon, sözleşme tipi gibi özniteliklere göre yetki | Departman bazlı yetkiler bu fikrin sade hali |
| **Business role** (iş rolü) | İş dilindeki görev: "Sistem Uzmanı" | Birincil ve ek roller |
| **Entitlement** (yetki öğesi) | Hedef sistemdeki tek ve somut izin: bir AD grubu, bir mail listesi | Katalogdaki her kayıt |
| **Role explosion** (rol patlaması) | Her kombinasyon için ayrı rol açılması ("Sistem Uzmanı – Ankara – VPN'siz") | Katmanlı modelle önlenir |
| **SoD** (Segregation of Duties) | Çakışan yetkilerin aynı kişide toplanmasının engellenmesi | v1'de yönetim ekranındaki görev ayrılığıyla sınırlı |
| **Access certification / review** | Yetkilerin periyodik olarak sorumlusuna onaylatılması | Kapsam dışı |

## Senkronizasyon ve bağlantı

| Terim | Anlamı |
|---|---|
| **Connector** | OpenSicil'in bir hedef sistemle konuşan parçası (AD connector, Zimbra connector) |
| **Desired state** (olması gereken durum) | Kimlik kaydı ve rollerden hesaplanan "bu kişi bu sistemde nasıl görünmeli" cevabı |
| **Reconciliation** (mutabakat) | Hedef sistemdeki gerçek durumu olması gereken durumla karşılaştırma |
| **Drift** (sapma) | Hedef sistemde birinin elle yaptığı ve kayıtla çelişen değişiklik |
| **Correlation / Adoption** (sahiplenme) | Hedef sistemde zaten var olan bir hesabı bir kimlik kaydına bağlamak |
| **Simulation** (gözlem modu) | Sahiplenilen hesap için farkın hesaplanıp gösterilmesi ama uygulanmaması |
| **Delegated administration** (yetki devri) | Bir operatörün sadece kurumun bir bölümünde yetkili olması. v2 |
| **Role mining** | Mevcut grup üyeliklerini analiz ederek rol tanımı çıkarmak |
| **Idempotent** | Aynı işlemin iki kez çalışmasının ek bir değişiklik yaratmaması |
| **SCIM 2.0** | Uygulamalara hesap açıp kapatmak için REST standardı (RFC 7643/7644) |
| **ConnId** | midPoint ve Apache Syncope'un kullandığı Java connector çatısı |

## Kimlik doğrulama (bağlam için)

| Terim | Anlamı |
|---|---|
| **IdP** | Kimliği doğrulayan taraf (Keycloak, Entra ID) |
| **OIDC** | OAuth2 üzerine kurulu kimlik protokolü. OpenSicil'in yönetim ekranı bununla giriş yapar ([ADR-005](decisions/005-yonetim-girisi-oidc.md)) |
| **Groups claim** | IdP'nin token içine koyduğu grup listesi. OpenSicil yönetim yetkilerini buradan okur |
| **LDAPS** | TLS üzerinden LDAP (port 636). AD connector sadece bununla konuşur |

## Kişisel veri

| Terim | Anlamı |
|---|---|
| **KVKK** | 6698 sayılı Kişisel Verilerin Korunması Kanunu. T.C. Kimlik No ve telefon kişisel veridir |
| **PII** | Personally Identifiable Information, kişiyi tanımlayan veri |
| **Blind index** | Şifreli bir alanda arama yapabilmek için değerin anahtarlı özetini (HMAC) ayrıca tutmak |
| **E.164** | Uluslararası telefon numarası biçimi: `+905321234567` |
