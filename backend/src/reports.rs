// --- START FEATURE: reports ---
// Raporlar girisi: yaklasan bitisler, kullanilmis adlar, silinmeyi bekleyenler
// ve mudahale bekleyen isler tek sayfada toplanir. Kendi verisi yok — var olan
// ekranlara giden bir kapak; menudeki dagınık maddeler yerine mockup'taki tek
// "Raporlar" satirini karsilar. Okuma her operatorde (auditor dahil).
//
// Mutabakat burada degil: ADR-123 ile kendi menu maddesine ve `/reconcile`
// sayfasina tasindi. "Bekleyen is" kutusu sahiplenmeyi bekleyen hesabi yine
// sayar — kutu "bugun yapacak ne var" diye sorar, "Raporlar'da ne var" diye degil.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use sqlx::PgPool;

use crate::i18n::Lang;
use crate::identity_web::{internal, OperatorSession};
use crate::shell::Shell;
use crate::web::{render, AppState};

/// Kapak sayfasinin ozet kutulari ve satir rozetleri. Hepsi var olan
/// tablolardan okunur, yeni migration yok.
pub struct Summary {
    /// Operatorun kuyrugu: mudahale bekleyen is + onay bekleyen silme +
    /// sahiplenmeyi bekleyen hesap. Uc ayri ekranin isi ama tek bir soru:
    /// "bugun yapacak ne var".
    pub pending: i64,
    /// Kuyrugun kirilimi, hazir cumle: sablon uc sayiyi tek tek dizmesin
    pub pending_foot: String,
    pub interventions: i64,
    pub deletions: i64,
    /// Yaklasan bitisler ve pencerenin gun sayisi
    pub upcoming: i64,
    pub upcoming_days: i32,
    /// Serbest birakilmamis kullanilmis ad
    pub used_names: i64,
}

#[derive(Template)]
#[template(path = "reports.html")]
struct ReportsTemplate {
    lang: Lang,
    shell: Shell,
    summary: Summary,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/reports", get(page))
}

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    // Sayilar gidilecek ekranlarin kendi yardimcilarindan okunur, kopya SQL yok:
    // rozet "3" diyorsa liste uc satir gostermek zorunda.
    let loaded = tokio::try_join!(
        counts(&state.pool),
        crate::reconcile::unadopted(&state.pool),
        crate::deletions::awaiting_count(&state.pool),
        crate::upcoming::list(&state.pool, &state.time_zone, crate::upcoming::DEFAULT_DAYS),
    );
    let (counts, unadopted, deletions, upcoming) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => return internal("raporlar sayfası okunamadı", e),
    };
    let unadopted: i64 = unadopted.iter().map(|u| u.count).sum();
    render(&ReportsTemplate {
        lang: op.lang,
        shell: Shell::of(&op),
        summary: Summary {
            pending: counts.interventions + deletions + unadopted,
            pending_foot: op.lang.tn(
                "reports.pending_foot",
                &[
                    &counts.interventions.to_string(),
                    &deletions.to_string(),
                    &unadopted.to_string(),
                ],
            ),
            interventions: counts.interventions,
            deletions,
            upcoming: upcoming.len() as i64,
            upcoming_days: crate::upcoming::DEFAULT_DAYS,
            used_names: counts.used_names,
        },
    })
}

struct Counts {
    interventions: i64,
    used_names: i64,
}

/// Kendi yardimcisi olmayan iki sayi tek sorguda. Silme ve yaklasan bitis
/// kendi modullerinden gelir (`deletions::awaiting_count`, `upcoming::list`):
/// kosullari kopyalamak, ekranla rozetin sessizce ayrisma yolu olurdu.
async fn counts(pool: &PgPool) -> Result<Counts, sqlx::Error> {
    let row: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM jobs WHERE status = $1), \
                (SELECT count(*) FROM used_names WHERE released_at IS NULL)",
    )
    .bind(crate::identity::INTERVENTION_STATUS)
    .fetch_one(pool)
    .await?;
    Ok(Counts {
        interventions: row.0,
        used_names: row.1,
    })
}
// --- END FEATURE: reports ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Ozet kutusundaki sayi uc ayri listenin toplami; tek bir sayfasi yok, bu
    /// yuzden kutu baglanti olmamali. Eskiden `/interventions`e gidiyordu:
    /// "26 bekleyen is" diyen kutu bir mudahale varken bos liste aciyordu.
    /// Her bilesen kendi satirinda, kendi sayisi ve kendi adresiyle durur.
    #[test]
    fn the_pending_box_is_not_a_link_and_each_list_carries_its_own_count() {
        let page = ReportsTemplate {
            lang: crate::i18n::DEFAULT,
            shell: Shell::from_parts("admin", &["admin".to_string()]),
            summary: Summary {
                pending: 26,
                pending_foot: String::new(),
                interventions: 1,
                deletions: 2,
                upcoming: 0,
                upcoming_days: 30,
                used_names: 0,
            },
        }
        .render()
        .unwrap();
        assert!(
            !page.contains(r#"<a class="stat stat--left""#),
            "toplam kutusu baglanti olmamali: {page}"
        );
        let row = page
            .split_once(r#"href="/interventions""#)
            .expect("mudahale satiri yok")
            .1;
        let row = row.split_once("</a>").expect("satir kapanmamis").0;
        assert!(
            row.contains(">1<") && !row.contains(">26<"),
            "mudahale satiri kendi sayisini gostermeli: {row}"
        );
    }
}
