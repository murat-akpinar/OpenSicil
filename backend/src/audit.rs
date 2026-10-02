// --- START FEATURE: audit-log ---
// Operator olaylarinin denetim kaydi (docs/07 "Denetim kaydi"). Satir yalnizca
// eklenir; kim (OIDC sub + kullanici adi), ne, hangi kimlik, once/sonra (hassas
// alanlar haric) yazilir. Worker'in niyet/sonuc satirlari worker crate'inde.

use sqlx::PgPool;

pub const SETTINGS_CHANGED: &str = "settings.changed";
pub const BOOTSTRAP_PASSWORD_CHANGED: &str = "bootstrap.password_changed";
pub const OPERATOR_LOGIN: &str = "operator.login";
pub const OPERATOR_REJECTED: &str = "operator.rejected";
pub const IDENTITY_CREATED: &str = "identity.created";
pub const JOB_RETRY_REQUESTED: &str = "job.retry_requested";
pub const ROLE_CHANGED: &str = "role.changed";
pub const DEPARTMENT_CHANGED: &str = "department.changed";
pub const TARGET_CHANGED: &str = "target.changed";
pub const IDENTITY_NAME_REQUESTED: &str = "identity.name_requested";
pub const USED_NAME_RELEASED: &str = "used_name.released";
pub const MAPPING_CHANGED: &str = "mapping.changed";
pub const IDENTITY_CHANGED: &str = "identity.changed";
pub const IDENTITY_ROLE_ASSIGNED: &str = "identity.role_assigned";
pub const IDENTITY_ROLE_REMOVED: &str = "identity.role_removed";
pub const IDENTITY_DEPARTURE_SET: &str = "identity.departure_set";
pub const IDENTITY_EMERGENCY_DEPARTURE: &str = "identity.emergency_departure";
pub const IDENTITY_DEPARTURE_REVERTED: &str = "identity.departure_reverted";
pub const IDENTITY_CANCELLED: &str = "identity.cancelled";
pub const IDENTITY_SUSPENDED: &str = "identity.suspended";
pub const IDENTITY_SUSPENSION_LIFTED: &str = "identity.suspension_lifted";
pub const ACCOUNT_MANAGE_REQUESTED: &str = "account.manage_requested";
pub const FIRST_PASSWORD_REQUESTED: &str = "first_password.requested";
pub const FIRST_PASSWORD_SHOWN: &str = "first_password.shown";
/// F-13: mutabakat bulgusundan "yeniden uygula" — kimlik icin is acildi
pub const RECONCILE_REAPPLY: &str = "reconcile.reapply";
/// ADR-112 madde 2: operator AD'deki degeri kimlige aldi (once/sonra detayda)
pub const IDENTITY_FIELD_TAKEN: &str = "identity.field_taken";
/// ADR-024: saklamasi dolan hesabin silinmesi operator tarafindan onaylandi
pub const ACCOUNT_DELETION_APPROVED: &str = "account.deletion_approved";
// F-17 CSV ice aktarma (ADR-018): parti olaylari kimliksiz (kim, ne zaman, kac satir),
// kimlik basina olay kisinin olay listesinde
pub const IMPORT_APPLIED: &str = "import.applied";
pub const IMPORT_STAGED: &str = "import.staged";
pub const IMPORT_APPROVED: &str = "import.approved";
pub const IMPORT_REJECTED: &str = "import.rejected";
pub const IDENTITY_IMPORTED: &str = "identity.imported";
// Toplu yonetime alma (ADR-018/043): esigi asan secim parti olaylariyla
pub const MANAGE_STAGED: &str = "manage.staged";
pub const MANAGE_APPROVED: &str = "manage.approved";
pub const MANAGE_REJECTED: &str = "manage.rejected";

pub struct Actor<'a> {
    pub subject: Option<&'a str>,
    pub username: &'a str,
}

pub async fn record(
    pool: &PgPool,
    actor: &Actor<'_>,
    event_type: &str,
    identity_id: Option<i64>,
    detail: serde_json::Value,
) -> Result<(), sqlx::Error> {
    // sqlx "json" ozelligi acilmadan metin olarak baglanip DB'de jsonb'ye cevrilir.
    sqlx::query(
        "INSERT INTO audit_log (event_type, actor_subject, actor_username, identity_id, detail) \
         VALUES ($1, $2, $3, $4, $5::jsonb)",
    )
    .bind(event_type)
    .bind(actor.subject)
    .bind(actor.username)
    .bind(identity_id)
    .bind(detail.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

// Yapilandirma degisikligi: sirlar hic yazilmaz, yalnizca hangi sirrin
// guncellendigi (docs/07 "hassas alanlar haric").
pub fn settings_change_detail(
    before: &crate::settings::AppSettings,
    after: &crate::settings::AppSettings,
    secrets_updated: &[&str],
) -> serde_json::Value {
    let public = |s: &crate::settings::AppSettings| {
        serde_json::json!({
            "ad_host": s.ad_host,
            "ad_bind_dn": s.ad_bind_dn,
            "ad_national_id_attribute": s.ad_national_id_attribute,
            "zimbra_url": s.zimbra_url,
            "oidc_issuer": s.oidc_issuer,
            "oidc_client_id": s.oidc_client_id,
        })
    };
    serde_json::json!({
        "before": public(before),
        "after": public(after),
        "secrets_updated": secrets_updated,
    })
}
// --- END FEATURE: audit-log ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Etiketi olmayan olay ekranda "?" basar (`Lang::key` yoksa "?" doner).
    /// Liste elle tutulmaz, bu dosyanin kendi kaynagi taranir; worker'in yazdigi
    /// ama buradan basilan olaylar `WORKER_EVENTS`'te durur.
    #[test]
    fn every_event_type_has_a_screen_label() {
        const WORKER_EVENTS: [&str; 2] = ["identity.fields_filled", "identity.role_expired"];
        let source = include_str!("audit.rs");
        let declared = source
            .lines()
            .filter(|line| line.starts_with("pub const "))
            .filter_map(|line| line.split('"').nth(1));
        for event in declared.chain(WORKER_EVENTS) {
            for lang in [crate::i18n::Lang::Tr, crate::i18n::Lang::En] {
                assert_ne!(lang.key("event", event), "?", "event.{event} eksik");
            }
        }
    }

    fn settings(host: &str, secret_set: bool) -> crate::settings::AppSettings {
        crate::settings::AppSettings {
            ad_host: host.to_string(),
            ad_bind_dn: "CN=svc,DC=example,DC=local".to_string(),
            ad_service_password_set: secret_set,
            ad_national_id_attribute: String::new(),
            zimbra_url: String::new(),
            zimbra_admin_password_set: false,
            oidc_issuer: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret_set: false,
            oidc_admin_verified: false,
        }
    }

    #[test]
    fn settings_detail_never_contains_secret_values_or_flags() {
        let detail = settings_change_detail(
            &settings("dc1", false),
            &settings("dc2", true),
            &["ad_service_password"],
        );
        let text = detail.to_string();
        assert!(
            !text.contains("password_set"),
            "sır bayrağı bile yazılmaz: {text}"
        );
        assert_eq!(detail["before"]["ad_host"], "dc1");
        assert_eq!(detail["after"]["ad_host"], "dc2");
        assert_eq!(detail["secrets_updated"][0], "ad_service_password");
    }

    #[tokio::test]
    async fn record_fails_when_db_unreachable() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .expect("lazy pool kurulamadı");
        let actor = Actor {
            subject: None,
            username: "admin",
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            record(&pool, &actor, SETTINGS_CHANGED, None, serde_json::json!({})),
        )
        .await;
        match result {
            Ok(inner) => assert!(inner.is_err()),
            Err(_elapsed) => {}
        }
    }

    // Gercek Postgres gerektirir (ADR-070).
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn record_writes_actor_and_view_counts_intents_per_identity() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let actor = Actor {
            subject: Some("sub-1"),
            username: "ayse",
        };
        record(&pool, &actor, OPERATOR_LOGIN, None, serde_json::json!({}))
            .await
            .expect("olay yazılamadı");
        let row: (Option<String>, String, String) = sqlx::query_as(
            "SELECT actor_subject, actor_username, performed_by::text FROM audit_log \
             WHERE event_type = $1",
        )
        .bind(OPERATOR_LOGIN)
        .fetch_one(&pool)
        .await
        .expect("olay okunamadı");
        assert_eq!(row.0.as_deref(), Some("sub-1"));
        assert_eq!(row.1, "ayse");
        assert!(!row.2.is_empty(), "performed_by current_user'dan dolmalı");

        assert_view_counts_identities_not_rows(&pool).await;

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    // ADR-050/062: sayac niyet satirlarindan kimlik sayar; yeniden deneme, sonuc
    // satiri, bir saatten eski niyet ve sinifsiz satir sayilmaz.
    async fn assert_view_counts_identities_not_rows(pool: &PgPool) {
        let catalog = crate::test_support::seed_example_catalog(pool).await;
        let ids = crate::test_support::seed_two_identities(pool).await;
        let intent = |identity: i64, emergency: bool, age: &'static str| async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO audit_log (event_type, identity_id, target_system_id, operation_class, emergency, occurred_at) \
                 VALUES ('ad.account.disable', $1, $2, 'destructive', $3, now() - $4::interval) RETURNING id",
            )
            .bind(identity)
            .bind(catalog.ad)
            .bind(emergency)
            .bind(age)
            .fetch_one(pool)
            .await
            .expect("niyet satırı yazılamadı")
        };
        let first = intent(ids[0], false, "0 minutes").await;
        intent(ids[0], false, "10 minutes").await;
        intent(ids[1], true, "20 minutes").await;
        intent(ids[1], false, "3 hours").await;
        sqlx::query(
            "INSERT INTO audit_log (event_type, identity_id, intent_id, outcome) \
             VALUES ('ad.account.disable', $1, $2, 'succeeded')",
        )
        .bind(ids[0])
        .bind(first)
        .execute(pool)
        .await
        .expect("sonuç satırı yazılamadı");

        let usage: (String, i64, i64) = sqlx::query_as(
            "SELECT operation_class, identities, emergency_identities FROM hourly_counter_usage",
        )
        .fetch_one(pool)
        .await
        .expect("sayaç görünümü okunamadı");
        assert_eq!(usage, ("destructive".to_string(), 2, 1));

        let open_intents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log i WHERE i.operation_class IS NOT NULL \
             AND NOT EXISTS (SELECT FROM audit_log r WHERE r.intent_id = i.id)",
        )
        .fetch_one(pool)
        .await
        .expect("açık niyetler sayılamadı");
        assert_eq!(
            open_intents, 3,
            "sonucu olmayan niyet 'sonucu bilinmiyor'dur"
        );
    }
}
