// --- START FEATURE: write-point ---
// Connector yazma cagrilarinin tek gecis noktasi (ADR-054, ADR-062): kuru
// calistirma acikken hedefe HICBIR sey yazilmaz, niyet satiri da acilmaz
// (sayaclar degismez); acikken once niyet satiri + kira uzatmasi, sonra
// connector yazmasi, sonra sonuc satiri. Kira baskasina gecmisse yazma yapilmaz.
// Motor her hedef yazmasini buradan gecirir; connector yalnizca `TargetWriter`'i
// uygular ve niyet/sonuc satirlarini bilmez.

use sqlx::PgPool;

use crate::queue::{self, ClaimedJob, Intent, IntentError};

/// ADR-050 sayac sinifi; motor gecise gore secer (ornek: ayrilisi geri alan
/// etkinlestirme yikicidir, ADR-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationClass {
    Destructive,
    Grant,
    /// Ilk parola teslimi (3d) uretir
    #[allow(dead_code)]
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

/// Hedefe yazilacak islem; parola JSON detaya asla girmez.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Tek `add`: oznitelikler + parola + UAC 514 + pwdLastSet 0 (+ accountExpires)
    CreateAccount {
        dn: String,
        attributes: Vec<(String, String)>,
        password: String,
        account_expires: Option<i64>,
    },
    SetEnabled {
        dn: String,
        enabled: bool,
    },
    AddMember {
        group_dn: String,
        member_dn: String,
    },
    /// Uyelik farki: rolde olmayan katalog grubu cikarilir (docs/03, ADR-050)
    RemoveMember {
        group_dn: String,
        member_dn: String,
    },
    /// Eslenen oznitelikler: Some = replace, None = sil (ADR-012/034/082)
    SetAttributes {
        dn: String,
        changes: Vec<(String, Option<String>)>,
    },
    /// OU tasima (modifyDN; docs/05 "OU tasima", ADR-050 sirasi: ekleme ve cikarma arasinda)
    MoveAccount {
        dn: String,
        new_rdn: String,
        new_parent: String,
    },
    /// Saklama sonu ya da dogrulanmis iptal (ADR-024/048/084)
    DeleteAccount {
        dn: String,
    },
    /// Ayrilista gecikmeli parola rastgelelestirme + pwdLastSet 0 (ADR-033); parola detaya girmez
    ResetPassword {
        dn: String,
    },
}

impl WriteOp {
    pub fn event_type(&self) -> &'static str {
        match self {
            WriteOp::CreateAccount { .. } => "ad.account.create",
            WriteOp::SetEnabled { enabled: true, .. } => "ad.account.enable",
            WriteOp::SetEnabled { enabled: false, .. } => "ad.account.disable",
            WriteOp::AddMember { .. } => "ad.group.add_member",
            WriteOp::RemoveMember { .. } => "ad.group.remove_member",
            WriteOp::SetAttributes { .. } => "ad.account.attributes",
            WriteOp::MoveAccount { .. } => "ad.account.move",
            WriteOp::DeleteAccount { .. } => "ad.account.delete",
            WriteOp::ResetPassword { .. } => "ad.account.password_reset",
        }
    }

    // Kucuk sabit sekilli JSON; worker'da serde yok. DN'ler tirnak icerebilir → kacis.
    pub fn detail_json(&self) -> String {
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        match self {
            WriteOp::CreateAccount { dn, attributes, .. } => format!(
                "{{\"dn\":\"{}\",\"attributes\":[{}]}}",
                q(dn),
                attribute_names_json(attributes)
            ),
            WriteOp::SetEnabled { dn, enabled } => {
                format!("{{\"dn\":\"{}\",\"enabled\":{enabled}}}", q(dn))
            }
            WriteOp::DeleteAccount { dn } | WriteOp::ResetPassword { dn } => {
                format!("{{\"dn\":\"{}\"}}", q(dn))
            }
            // Degerler yazilmaz: hassas kaynak (kimlik no) denetim kaydina girmez (docs/07)
            WriteOp::SetAttributes { dn, changes } => format!(
                "{{\"dn\":\"{}\",\"attributes\":[{}]}}",
                q(dn),
                attribute_changes_json(changes)
            ),
            WriteOp::AddMember {
                group_dn,
                member_dn,
            }
            | WriteOp::RemoveMember {
                group_dn,
                member_dn,
            } => format!(
                "{{\"group\":\"{}\",\"member\":\"{}\"}}",
                q(group_dn),
                q(member_dn)
            ),
            WriteOp::MoveAccount {
                dn,
                new_rdn,
                new_parent,
            } => format!(
                "{{\"dn\":\"{}\",\"new_rdn\":\"{}\",\"new_parent\":\"{}\"}}",
                q(dn),
                q(new_rdn),
                q(new_parent)
            ),
        }
    }
}

fn json_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn attribute_names_json(attributes: &[(String, String)]) -> String {
    attributes
        .iter()
        .map(|(name, _)| format!("\"{}\"", json_quote(name)))
        .collect::<Vec<_>>()
        .join(",")
}

fn attribute_changes_json(changes: &[(String, Option<String>)]) -> String {
    changes
        .iter()
        .map(|(name, value)| {
            format!(
                "{{\"name\":\"{}\",\"cleared\":{}}}",
                json_quote(name),
                value.is_none()
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub struct WriteRequest<'a> {
    pub op: &'a WriteOp,
    pub class: OperationClass,
    pub emergency: bool,
}

#[derive(Debug)]
pub enum WriteError {
    /// Baglanti duzeyi: deneme tuketmez, hedef bekletilir (ADR-052)
    Unreachable(String),
    /// Nesne duzeyi: deneme tuketir
    Failed(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Unreachable(r) => write!(f, "hedefe ulaşılamıyor: {r}"),
            WriteError::Failed(r) => write!(f, "{r}"),
        }
    }
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
        op: &WriteOp,
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
    let detail = request.op.detail_json();
    let intent = Intent {
        event_type: request.op.event_type(),
        operation_class: request.class.as_str(),
        emergency: request.emergency,
        detail_json: &detail,
    };
    let intent_id = queue::record_intent(pool, job, worker_id, &intent)
        .await
        .map_err(|e| match e {
            IntentError::LeaseLost => WriteFailure::LeaseLost,
            IntentError::Db(e) => WriteFailure::Db(e),
        })?;
    let outcome = writer.write(request.op).await;
    queue::record_outcome(
        pool,
        job,
        intent_id,
        outcome.is_ok(),
        request.op.event_type(),
    )
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
        async fn write(&mut self, op: &WriteOp) -> Result<(), WriteError> {
            self.calls.push(op.event_type().to_string());
            match self.fail_with.take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn detail_json_never_contains_password_and_escapes_quotes() {
        let op = WriteOp::CreateAccount {
            dn: "CN=Yılmaz \"Ayşe\",OU=Personel,DC=x".to_string(),
            attributes: vec![("givenName".to_string(), "Ayşe".to_string())],
            password: "Gizli-Parola-1".to_string(),
            account_expires: None,
        };
        let detail = op.detail_json();
        assert!(!detail.contains("Gizli"), "{detail}");
        assert!(detail.contains("\\\"Ayşe\\\""), "{detail}");
        assert!(detail.contains("\"givenName\""), "{detail}");
        assert_eq!(op.event_type(), "ad.account.create");
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
        let op = WriteOp::SetEnabled {
            dn: "CN=x,OU=Personel,DC=x".to_string(),
            enabled: false,
        };
        let request = WriteRequest {
            op: &op,
            class: OperationClass::Destructive,
            emergency: false,
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
