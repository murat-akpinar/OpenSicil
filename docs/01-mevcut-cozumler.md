# 01 — Mevcut Çözümler

Sıfırdan yazmadan önce: bu işi yapan olgun ürünler var. Bu dosya neyi yeniden icat ettiğimizi açıkça ortaya koyar. Neden yine de yazıldığı: ADR-002.

> **Not:** Bu alanda lisanslar sık değişiyor; midPoint'in lisansı da 2025'te değişti. Aşağıdaki lisanslar **2026-09-17** tarihinde projelerin kendi depolarından kontrol edildi. Bir karar vermeden önce tekrar kontrol edin.

## Açık kaynak

| Ürün | Kategori | Lisans (2026-09-17) | Güçlü yanı | Zayıf yanı |
|---|---|---|---|---|
| **Evolveum midPoint** | IGA | **EUPL-1.2-or-later**. Apache-2.0/EUPL ikili lisansı 2025-10-13'te kaldırıldı. Dokümanlar CC BY-NC-ND 4.0 | Sektörün en kapsamlı açık kaynak IGA'sı: rol modeli, JML, mutabakat, onay akışları, yetki gözden geçirme. **Asıl referans** | Ağır (Java), öğrenme eğrisi dik, eşlemeler betik (Groovy) ile yazılır. Resmi Zimbra rehberi 2021 tarihli ve Zimbra'nın kendi LDAP'ına doğrudan yazıyor |
| **Apache Syncope** | IGA | Apache-2.0 | ConnId connector'ları, bakımı süren bir AD connector'ı, REST API | Java; midPoint'e göre daha dar bir topluluk |
| **ConnId Zimbra Bundle** (Tirasa) | Zimbra connector'ı | Apache-2.0 | SOAP üzerinden çalışır; Syncope ve midPoint'te kullanılabilir | Neredeyse bakımsız: 2021'den beri tek bir küçük insan commit'i, sürüm etiketi yok, Zimbra 8.8.12'ye göre derleniyor |
| **OpenIAM Community Edition** | IAM/IGA | Sitede belirtilmiyor. Tek açık depo LGPL-3.0 ve son push'u Temmuz 2018 | Ticari bir IGA'nın ücretsiz sürümü | Açık kaynak durumu belirsiz. Adı bu projenin eski çalışma adıyla çakışıyordu; ürün bu yüzden **OpenSicil** oldu (ADR-063) |
| **Keycloak** | IdP | Apache-2.0 (OpenBerat `docs/01`, 2026-09-07) | AD federasyonu, OIDC/SAML. OpenSicil'in yönetim girişi için kullanılabilir. **Rakip değil** | IGA değil: AD'de hesap açmaz, yaşam döngüsü yönetmez |

## Ticari (referans ve rakip analizi)

| Ürün | Not |
|---|---|
| **SailPoint** (IdentityIQ, Identity Security Cloud) | IGA pazar lideri. Rol modeli ve yetki gözden geçirme ekranları için UX referansı |
| **Saviynt** | Bulut IGA; SailPoint'in doğrudan rakibi |
| **One Identity Manager** | Şirket içi AD ve Exchange odaklı kurumsal IGA |
| **Microsoft Entra ID Governance** | Lifecycle Workflows ve İK sistemi kaynaklı provisioning. AD'si olan ve Microsoft 365 kullanan bir kurumda **"bu zaten var" itirazının kaynağı**. Ciddiye alınmalı |
| **Okta** (Lifecycle Management, Workflows) | SaaS uygulamalar için JML; şirket içi AD'ye ajanla bağlanır |
| **ManageEngine ADManager Plus** | Şablonla AD kullanıcısı açma, toplu işlem, raporlama. **Hedef kullanıcı açısından en yakın rakip** |

## Neyi yeniden icat etmiyoruz

- **Kimlik doğrulama:** Keycloak veya başka bir OIDC sağlayıcı. OpenSicil parola doğrulamaz (ADR-005).
- **Erişim kararı:** Uygulamanın kendisi veya OpenBerat gibi bir IAP (ADR-008).
- **Dizin:** AD'nin kendisi. OpenSicil ayrı bir kullanıcı dizini tutmaz; kaynak kaydı ve hesap bağlantılarını tutar.

## Okumaya değer kaynaklar

Yazmaya başlamadan önce, sırayla:

1. **midPoint dokümantasyonu:** rol modeli (business/application role), "inbound/outbound mapping" ve "synchronization situations" (sahipsiz hesap, eşleşmemiş hesap). Kavramlar doğrudan buradan geliyor.
2. **ConnId Zimbra Bundle:** Zimbra SOAP işlemlerinin bir connector'da nasıl kullanıldığını gösteren somut örnek.
3. **Microsoft: Appendix C — Protected Accounts and Groups:** Yasaklı grup listesinin kaynağı ([docs/05](05-active-directory.md)).

## Kaynaklar

- midPoint lisansı: https://github.com/Evolveum/midpoint/blob/master/LICENSE
- midPoint Zimbra rehberi: https://docs.evolveum.com/connectors/resources/zimbra/
- Apache Syncope: https://syncope.apache.org/docs/reference-guide.html · AD connector'ı: https://github.com/Tirasa/ConnIdADBundle
- ConnId Zimbra Bundle: https://github.com/Tirasa/ConnIdZimbraBundle
- OpenSicil CE: https://www.openiam.com/ce-vs-ee · https://github.com/openiam-community/openiam-community · https://compare.evolveum.com/details-openiam.html
