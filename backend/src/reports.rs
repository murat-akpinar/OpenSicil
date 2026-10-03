// --- START FEATURE: reports ---
// Raporlar girisi: hesap kapsami, yaklasan bitisler, kullanilmis adlar,
// silinmeyi bekleyenler ve mudahale bekleyen isler tek sayfada toplanir. Kendi
// verisi yok — var olan ekranlara giden bir kapak; menudeki dagınık maddeler
// yerine mockup'taki tek "Raporlar" satirini karsilar. Okuma her operatorde
// (auditor dahil).
//
// Mutabakat burada degil: ADR-123 ile kendi menu maddesine ve `/reconcile`
// sayfasina tasindi. "Bekleyen is" kutusu sahiplenmeyi bekleyen hesabi yine
// sayar — kutu "bugun yapacak ne var" diye sorar, "Raporlar'da ne var" diye degil.
//
// ADR-130: hesap kapsami karti "kaci yonetiliyor, kaci gozlemde" sorusunu
// hedef basina cevaplar ve satir dogrudan o hedefin toplu yonetime alma
// ekranina gider; sayilar `bulk_manage::coverage`den, o ekranin listesiyle
// ayni kosullardan okunur.

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
    /// ADR-130: butun hedeflerin toplami ve hedef basina kirilim
    pub managed: i64,
    pub observed: i64,
    pub coverage: Vec<CoverageRow>,
}

/// Hesap kapsami satiri: hedefin adi, iki sayi ve "yonetilen" payinin cubugu.
pub struct CoverageRow {
    pub target_id: i64,
    pub name: String,
    pub observed: i64,
    /// Hazir cumle: "12 yonetiliyor · 3 gozlemde"
    pub foot: String,
    /// Yonetilen payi, bese yuvarlanmis — sablon `.v-NN` sinifini secer
    /// (satir ici `style` CSP'de yasak)
    pub pct: i64,
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
        crate::bulk_manage::coverage(&state.pool),
    );
    let (counts, unadopted, deletions, upcoming, coverage) = match loaded {
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
            managed: coverage.iter().map(|c| c.managed).sum(),
            observed: coverage.iter().map(|c| c.observed).sum(),
            coverage: coverage.into_iter().map(|c| row(&op.lang, c)).collect(),
        },
    })
}

fn row(lang: &Lang, c: crate::bulk_manage::Coverage) -> CoverageRow {
    CoverageRow {
        pct: crate::dashboard::percent(c.managed, c.total()),
        foot: lang.tn(
            "reports.coverage_foot",
            &[&c.managed.to_string(), &c.observed.to_string()],
        ),
        target_id: c.target_id,
        name: c.name,
        observed: c.observed,
    }
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

    fn summary() -> Summary {
        Summary {
            pending: 26,
            pending_foot: String::new(),
            interventions: 1,
            deletions: 2,
            upcoming: 0,
            upcoming_days: 30,
            used_names: 0,
            managed: 12,
            observed: 3,
            coverage: vec![CoverageRow {
                target_id: 7,
                name: "Active Directory".into(),
                observed: 3,
                foot: "12 yönetiliyor · 3 gözlemde".into(),
                pct: 80,
            }],
        }
    }

    fn page(summary: Summary) -> String {
        ReportsTemplate {
            lang: crate::i18n::DEFAULT,
            shell: Shell::from_parts("admin", &["admin".to_string()]),
            summary,
        }
        .render()
        .unwrap()
    }

    /// Ozet kutusundaki sayi uc ayri listenin toplami; tek bir sayfasi yok, bu
    /// yuzden kutu baglanti olmamali. Eskiden `/interventions`e gidiyordu:
    /// "26 bekleyen is" diyen kutu bir mudahale varken bos liste aciyordu.
    /// Her bilesen kendi satirinda, kendi sayisi ve kendi adresiyle durur.
    #[test]
    fn the_pending_box_is_not_a_link_and_each_list_carries_its_own_count() {
        let page = page(summary());
        let before = page
            .split_once(crate::i18n::DEFAULT.t("reports.pending"))
            .expect("bekleyen is kutusu yok")
            .0;
        assert!(
            before.rfind("<div class=\"sum-box") > before.rfind("<a class=\"sum-box"),
            "toplam kutusu baglanti olmamali"
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

    /// ADR-130: kapsam satiri "kaci yonetiliyor, kaci gozlemde" der ve
    /// gozlemdekileri toplu yonetime alma ekranina gider — rapor okunup
    /// kapatilmasin, isin yapildigi yere baglansin.
    #[test]
    fn the_coverage_row_links_to_bulk_manage_of_its_own_target() {
        let page = page(summary());
        let row = page
            .split_once(r#"href="/targets/7/manage""#)
            .expect("kapsam satiri toplu yonetime almaya gitmeli")
            .1;
        let row = row.split_once("</a>").expect("satir kapanmamis").0;
        assert!(row.contains("12 yönetiliyor · 3 gözlemde"), "{row}");
        assert!(row.contains("v-80"), "yonetilen payinin cubugu yok: {row}");
    }

    /// Hic baglanti yoksa kart bos kalmaz: yerine tek cumle gecer.
    #[test]
    fn coverage_card_falls_back_to_a_sentence_without_links() {
        let mut s = summary();
        s.coverage.clear();
        s.managed = 0;
        s.observed = 0;
        let page = page(s);
        assert!(!page.contains("/manage"), "{page}");
        assert!(page.contains(crate::i18n::DEFAULT.t("reports.coverage_empty")));
    }
}
