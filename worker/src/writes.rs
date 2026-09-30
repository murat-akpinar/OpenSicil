// --- START FEATURE: write-point ---
// Connector yazma cagrilarinin tek gecis noktasi (ADR-054, ADR-062): kuru
// calistirma acikken hedefe HICBIR sey yazilmaz, niyet satiri da acilmaz
// (sayaclar degismez); acikken once niyet satiri + kira uzatmasi, sonra
// connector yazmasi, sonra sonuc satiri. Kira baskasina gecmisse yazma yapilmaz.
// Motor (3a: fark → islemler) her hedef yazmasini buradan gecirir; connector
// bu dosyayi bilmez, yalnizca `TargetWriter`'i uygular.

use sqlx::PgPool;

use crate::queue::{self, ClaimedJob, Intent, IntentError};

/// ADR-050 sayac sinifi; motor gecise gore secer (ornek: ayrilisi geri alan
/// etkinlestirme yikicidir, ADR-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationClass {
    Destructive,
    Grant,
    FirstPassword,
    Attribute,
}

impl OperationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationClass::Destructive => "destructive",
            OperationClass::Grant => "grant",
            OperationClass::FirstPassword => "first_password",
            OperationClass::Attribute => "attribute",
        }
    }
}

pub struct WriteRequest<'a> {
    /// Denetim olay adi, ornek `ad.account.disable`
    pub event_type: &'a str,
    pub class: OperationClass,
    pub emergency: bool,
    /// Denetim satirina yazilan JSON metni (sir icermez)
    pub detail_json: &'a str,
}

#[derive(Debug)]
pub enum WriteError {
    /// Baglanti duzeyi: deneme tuketmez, hedef bekletilir (ADR-052)
    Unreachable(String),
    /// Nesne duzeyi: deneme tuketir
    Failed(String),
}

#[derive(Debug)]
pub enum WriteFailure {
    LeaseLost,
    Unreachable(String),
    Failed(String),
    Db(sqlx::Error),
}

impl std::fmt::Display for WriteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteFailure::LeaseLost => write!(f, "iş kirası başka worker'a geçti"),
            WriteFailure::Unreachable(r) => write!(f, "hedefe ulaşılamıyor: {r}"),
            WriteFailure::Failed(r) => write!(f, "hedef yazması başarısız: {r}"),
            WriteFailure::Db(e) => write!(f, "veritabanı hatası: {e}"),
        }
    }
}

/// Connector'in yazma yuzu; AD ve Zimbra bunu uygular, kendi protokol hatasini
/// Unreachable/Failed'e cevirir.
pub trait TargetWriter {
    fn write(
        &mut self,
        event_type: &str,
        detail_json: &str,
    ) -> impl std::future::Future<Output = Result<(), WriteError>> + Send;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub dry_run: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    Applied,
    /// Kuru calistirma: hedefe ve denetim kaydina dokunulmadi ("uygulanacakti")
    DryRun,
}

pub async fn apply<W: TargetWriter>(
    pool: &PgPool,
    job: &ClaimedJob,
    worker_id: &str,
    mode: Mode,
    writer: &mut W,
    request: &WriteRequest<'_>,
) -> Result<Applied, WriteFailure> {
    if mode.dry_run {
        return Ok(Applied::DryRun);
    }
    let intent = Intent {
        event_type: request.event_type,
        operation_class: request.class.as_str(),
        emergency: request.emergency,
        detail_json: request.detail_json,
    };
    let intent_id = queue::record_intent(pool, job, worker_id, &intent)
        .await
        .map_err(|e| match e {
            IntentError::LeaseLost => WriteFailure::LeaseLost,
            IntentError::Db(e) => WriteFailure::Db(e),
        })?;
    let outcome = writer.write(request.event_type, request.detail_json).await;
    queue::record_outcome(pool, job, intent_id, outcome.is_ok(), request.event_type)
        .await
        .map_err(WriteFailure::Db)?;
    match outcome {
        Ok(()) => Ok(Applied::Applied),
        Err(WriteError::Unreachable(r)) => Err(WriteFailure::Unreachable(r)),
        Err(WriteError::Failed(r)) => Err(WriteFailure::Failed(r)),
    }
}
// --- END FEATURE: write-point ---

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    struct FakeWriter {
        calls: Vec<String>,
        fail_with: Option<WriteError>,
    }

    impl TargetWriter for FakeWriter {
        async fn write(&mut self, event_type: &str, _detail: &str) -> Result<(), WriteError> {
            self.calls.push(event_type.to_string());
            match self.fail_with.take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    async fn audit_rows(pool: &PgPool) -> (i64, i64) {
        sqlx::query_as(
            "SELECT COUNT(*) FILTER (WHERE intent_id IS NULL AND operation_class IS NOT NULL), \
             COUNT(*) FILTER (WHERE outcome = 'succeeded') FROM audit_log",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn dry_run_touches_nothing_live_run_records_intent_and_outcome() {
        let (admin_pool, pool, db_name) = test_support::fresh_migrated_db().await;
        let seed = test_support::seed_example_model(&pool).await;
        test_support::enqueue(&pool, seed.identity, seed.ad, 1).await;
        let job = queue::claim(&pool, "w1", &[]).await.unwrap().unwrap();
        let request = WriteRequest {
            event_type: "ad.account.disable",
            class: OperationClass::Destructive,
            emergency: false,
            detail_json: "{}",
        };
        let mut writer = FakeWriter {
            calls: vec![],
            fail_with: None,
        };

        let dry = apply(
            &pool,
            &job,
            "w1",
            Mode { dry_run: true },
            &mut writer,
            &request,
        )
        .await;
        assert_eq!(dry.unwrap(), Applied::DryRun);
        assert!(writer.calls.is_empty(), "kuru modda hedefe yazılmaz");
        assert_eq!(
            audit_rows(&pool).await,
            (0, 0),
            "kuru modda niyet de yazılmaz"
        );

        let live = apply(
            &pool,
            &job,
            "w1",
            Mode { dry_run: false },
            &mut writer,
            &request,
        )
        .await;
        assert_eq!(live.unwrap(), Applied::Applied);
        assert_eq!(writer.calls, vec!["ad.account.disable"]);
        assert_eq!(audit_rows(&pool).await, (1, 1), "niyet + başarılı sonuç");

        writer.fail_with = Some(WriteError::Unreachable("LDAP kapalı".into()));
        let down = apply(
            &pool,
            &job,
            "w1",
            Mode { dry_run: false },
            &mut writer,
            &request,
        )
        .await;
        assert!(matches!(down, Err(WriteFailure::Unreachable(_))));
        assert_eq!(
            audit_rows(&pool).await,
            (2, 1),
            "başarısız sonuç da niyetine bağlanır"
        );

        // kira baskasinda: yazma yapilmaz
        sqlx::query("UPDATE jobs SET locked_by = 'w2' WHERE id = $1")
            .bind(job.id)
            .execute(&pool)
            .await
            .unwrap();
        let lost = apply(
            &pool,
            &job,
            "w1",
            Mode { dry_run: false },
            &mut writer,
            &request,
        )
        .await;
        assert!(matches!(lost, Err(WriteFailure::LeaseLost)));
        assert_eq!(writer.calls.len(), 2, "kira gidince hedefe dokunulmaz");

        drop(pool);
        test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
