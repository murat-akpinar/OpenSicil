// --- START FEATURE: throttle ---
// Saatlik fren sayaclari ve acil kota (ADR-016/050). Sayac denetim kaydindaki
// NIYET satirlarindan hesaplanir — backend bu kolonlari yazamaz (ADR-015), yani
// ele gecirilmis backend sayaci ne doldurabilir ne dusurebilir; worker yeniden
// baslasa da sayac sifirlanmaz. Birim kimliktir: ayni kimligin ikinci islemi ya
// da yeniden denemesi sayiyi buyutmez (migration 0006 `hourly_counter_usage`
// goruntusu ayni kurali ekrana verir).
//
// Is sayaclara karsi butundur (ADR-050): motor isin uretecegi siniflari once
// toplar, biri doluysa is HICBIR islem uygulamadan pencerenin acilisina ertelenir.
// Yalnizca oznitelik yazan islemler sayilmaz.

use sqlx::PgPool;

use crate::writes::OperationClass;

// $1 sinif, $2 kimlik. Doner: penceredeki kimlik sayisi; bu kimligin pencerede
// satiri var mi (varsa sayac buyumez); en eski satirin dusmesine kalan saniye —
// sayac en erken o an degisir, is o ana ertelenir.
const CLASS_USAGE_SQL: &str =
    "SELECT COUNT(DISTINCT identity_id), COUNT(*) FILTER (WHERE identity_id = $2), \
     COALESCE(CEIL(EXTRACT(EPOCH FROM MIN(occurred_at) + interval '1 hour' - now())), 0)::bigint \
     FROM audit_log \
     WHERE intent_id IS NULL AND operation_class = $1 \
     AND occurred_at >= now() - interval '1 hour'";

// $1 kimlik. Acil ayrilis kotasi sinif ayirmaz (ADR-016).
const EMERGENCY_USAGE_SQL: &str =
    "SELECT COUNT(DISTINCT identity_id), COUNT(*) FILTER (WHERE identity_id = $1) \
     FROM audit_log \
     WHERE intent_id IS NULL AND operation_class IS NOT NULL AND emergency \
     AND occurred_at >= now() - interval '1 hour'";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub destructive: u32,
    pub grant: u32,
    pub first_password: u32,
    pub emergency_quota: u32,
}

impl Limits {
    // ADR-050: sayac kumesi yikici, verme ve ilk paroladir; oznitelik sayilmaz.
    fn of(&self, class: OperationClass) -> Option<u32> {
        match class {
            OperationClass::Destructive => Some(self.destructive),
            OperationClass::Grant => Some(self.grant),
            OperationClass::FirstPassword => Some(self.first_password),
            OperationClass::Attribute => None,
        }
    }
}

pub fn label(class: OperationClass) -> &'static str {
    match class {
        OperationClass::Destructive => "yıkıcı işlem",
        OperationClass::Grant => "verme",
        OperationClass::FirstPassword => "ilk parola",
        OperationClass::Attribute => "öznitelik",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub class: OperationClass,
    pub used: i64,
    pub limit: u32,
    pub retry_after_seconds: i64,
}

// Kisi sayfasi bu metni "sebep / teknik ayrinti" sozlesmesiyle gosterir (F-12).
impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} sınırı dolu ({}/{}): iş hiçbir işlem uygulamadan bekliyor, \
             pencere en erken {} dk sonra açılıyor (ADR-050)",
            label(self.class),
            self.used,
            self.limit,
            (self.retry_after_seconds + 59) / 60
        )
    }
}

impl Blocked {
    /// Is satirina yazilan makine okunur neden: backend bunu ayristirip operatorun
    /// dilinde "bekleme sebebi" olarak gosterir (F-12, ADR-089 — worker metni
    /// cevirmez). Bicim: `throttle:<sinif>:<kullanilan>/<sinir>`.
    pub fn job_error(&self) -> String {
        format!(
            "throttle:{}:{}/{}",
            self.class.as_str(),
            self.used,
            self.limit
        )
    }
}

/// Isin uretecegi sayac siniflari; oznitelik islemleri listeye girmez.
pub fn needed_classes(grant: bool, destructive: bool, first_password: bool) -> Vec<OperationClass> {
    let mut classes = Vec::new();
    if destructive {
        classes.push(OperationClass::Destructive);
    }
    if grant {
        classes.push(OperationClass::Grant);
    }
    if first_password {
        classes.push(OperationClass::FirstPassword);
    }
    classes
}

/// ADR-050: siniflardan biri doluysa isin tamami bekler. Dolu sayaci acil ayrilis
/// kendi kotasiyla asar (ADR-016); kota da doluysa acil ayrilis da bekler.
pub async fn blocked(
    pool: &PgPool,
    classes: &[OperationClass],
    identity_id: i64,
    emergency: bool,
    limits: &Limits,
) -> Result<Option<Blocked>, String> {
    for &class in classes {
        let Some(limit) = limits.of(class) else {
            continue;
        };
        let (used, mine, opens_in): (i64, i64, i64) = sqlx::query_as(CLASS_USAGE_SQL)
            .bind(class.as_str())
            .bind(identity_id)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("saatlik sayaç okunamadı: {e}"))?;
        if mine > 0 || used < i64::from(limit) {
            continue;
        }
        if emergency && emergency_allows(pool, identity_id, limits.emergency_quota).await? {
            continue;
        }
        return Ok(Some(Blocked {
            class,
            used,
            limit,
            // 0 sn sonsuz donguye sokar: en az bir saniye beklenir
            retry_after_seconds: opens_in.max(1),
        }));
    }
    Ok(None)
}

async fn emergency_allows(pool: &PgPool, identity_id: i64, quota: u32) -> Result<bool, String> {
    if quota == 0 {
        return Ok(false);
    }
    let (used, mine): (i64, i64) = sqlx::query_as(EMERGENCY_USAGE_SQL)
        .bind(identity_id)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("acil kota okunamadı: {e}"))?;
    Ok(mine > 0 || used < i64::from(quota))
}
// --- END FEATURE: throttle ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    const LIMITS: Limits = Limits {
        destructive: 2,
        grant: 2,
        first_password: 2,
        emergency_quota: 1,
    };
    // Pencerede hic gorulmemis kimlikler; sorgu bunlari yalnizca kiyaslamada kullanir.
    const UNSEEN: i64 = -1;
    const UNSEEN_OTHER: i64 = -2;

    #[test]
    fn needed_classes_lists_only_counted_kinds() {
        assert!(needed_classes(false, false, false).is_empty());
        assert_eq!(
            needed_classes(true, true, true),
            vec![
                OperationClass::Destructive,
                OperationClass::Grant,
                OperationClass::FirstPassword
            ]
        );
        assert_eq!(LIMITS.of(OperationClass::Attribute), None);
    }

    #[test]
    fn blocked_message_names_counter_and_window() {
        let text = Blocked {
            class: OperationClass::Grant,
            used: 50,
            limit: 50,
            retry_after_seconds: 601,
        }
        .to_string();
        assert!(text.contains("verme sınırı dolu (50/50)"), "{text}");
        assert!(text.contains("11 dk"), "{text}");
    }

    // Is satirina makine okunur neden yazilir; backend operatorun dilinde gosterir (F-12).
    #[test]
    fn job_error_is_machine_readable() {
        assert_eq!(
            Blocked {
                class: OperationClass::Destructive,
                used: 12,
                limit: 10,
                retry_after_seconds: 60,
            }
            .job_error(),
            "throttle:destructive:12/10"
        );
    }

    async fn check(pool: &PgPool, identity: i64, emergency: bool) -> Option<Blocked> {
        blocked(pool, &[OperationClass::Grant], identity, emergency, &LIMITS)
            .await
            .unwrap()
    }

    async fn intent(pool: &PgPool, identity: i64, target: i64, class: &str, emergency: bool) {
        sqlx::query(
            "INSERT INTO audit_log (event_type, identity_id, target_system_id, \
             operation_class, emergency, detail) VALUES ('t', $1, $2, $3, $4, '{}'::jsonb)",
        )
        .bind(identity)
        .bind(target)
        .bind(class)
        .bind(emergency)
        .execute(pool)
        .await
        .unwrap();
    }

    // ADR-050: birim kimlik (ayni kimlik sayiyi buyutmez), dolu sayac yeni kimligi
    // bekletir, oznitelik sinifi hic sayilmaz; ADR-016: acil ayrilis kotayla gecer,
    // kota dolunca acil ayrilis da bekler.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn counts_identities_blocks_when_full_and_lets_emergency_through() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        let (a, b) = (seed.identity, seed.other_identity);

        assert_eq!(check(&pool, a, false).await, None, "sayaç boş");

        // ayni kimligin iki niyeti tek sayilir: sayac 1/2
        intent(&pool, a, seed.ad, "grant", false).await;
        intent(&pool, a, seed.ad, "grant", false).await;
        assert_eq!(check(&pool, b, false).await, None, "sayaç 1/2");

        intent(&pool, b, seed.ad, "grant", false).await;
        let stop = check(&pool, UNSEEN, false).await.expect("sayaç dolu");
        assert_eq!(
            (stop.class, stop.used, stop.limit),
            (OperationClass::Grant, 2, 2)
        );
        assert!((1..=3600).contains(&stop.retry_after_seconds));
        assert!(stop.to_string().contains("verme sınırı dolu (2/2)"));
        assert_eq!(
            check(&pool, a, false).await,
            None,
            "pencerede zaten sayılı kimlik"
        );
        assert_eq!(
            blocked(&pool, &[OperationClass::Attribute], UNSEEN, false, &LIMITS)
                .await
                .unwrap(),
            None,
            "öznitelik sınıfı sayılmaz"
        );

        // acil ayrilis dolu sayaci kendi kotasiyla asar; kota dolunca o da bekler
        assert_eq!(check(&pool, UNSEEN, true).await, None, "acil kota boş");
        intent(&pool, a, seed.ad, "destructive", true).await;
        assert_eq!(
            check(&pool, UNSEEN_OTHER, true).await.map(|b| b.class),
            Some(OperationClass::Grant),
            "acil kota da dolu"
        );

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
