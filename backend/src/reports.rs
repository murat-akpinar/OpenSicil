// --- START FEATURE: reports ---
// Raporlar girisi: mutabakat (hedef sistem basina), yaklasan bitisler ve
// kullanilmis adlar tek sayfada toplanir. Kendi verisi yok — var olan ekranlara
// giden bir kapak; menudeki uc dagınık madde yerine mockup'taki tek "Raporlar"
// satirini karsilar. Okuma her operatorde (auditor dahil).

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::i18n::Lang;
use crate::identity_web::{internal, OperatorSession};
use crate::shell::Shell;
use crate::web::{render, AppState};

/// Mutabakat satiri: hedef sistem adi ve tarama ekraninin adresi.
pub struct TargetLink {
    pub id: i64,
    pub name: String,
}

#[derive(Template)]
#[template(path = "reports.html")]
struct ReportsTemplate {
    lang: Lang,
    shell: Shell,
    targets: Vec<TargetLink>,
    /// ADR-024: silinmesi onay bekleyen hesap sayisi, listeye giden satirin rozeti
    awaiting_deletions: i64,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/reports", get(page))
}

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    let loaded = tokio::try_join!(
        crate::org::list_targets(&state.pool),
        crate::deletions::awaiting_count(&state.pool),
    );
    match loaded {
        Ok((rows, awaiting_deletions)) => render(&ReportsTemplate {
            lang: op.lang,
            shell: Shell::of(&op),
            targets: rows
                .into_iter()
                .map(|t| TargetLink {
                    id: t.id,
                    name: t.name,
                })
                .collect(),
            awaiting_deletions,
        }),
        Err(e) => internal("raporlar sayfası okunamadı", e),
    }
}
// --- END FEATURE: reports ---

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_hub_lists_every_target_system_for_reconciliation() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        // AD ve Zimbra satirlari migration'da seed ediliyor (0005_catalog.sql)
        let targets = crate::org::list_targets(&pool).await.unwrap();
        assert!(
            targets.len() >= 2,
            "hedef sistemler seed'den gelmeli: {}",
            targets.len()
        );
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
