// --- START FEATURE: reports ---
// Raporlar girisi: hesap kapsami, yaklasan bitisler ve bekleyen is ozeti. Kendi
// verisi yok — var olan ekranlara giden bir kapak. Dort liste (yaklasan,
// kullanilmis adlar, silme, mudahale) Personel seridinde (ADR-134 madde 3).
// Okuma her operatorde (auditor dahil).
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

use crate::i18n::Lang;
use crate::identity_web::{internal, OperatorSession};
use crate::shell::{Shell, Tabs};
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
    /// Kutunun gittigi liste: dolu olan ilki (`pending_target`)
    pub pending_href: &'static str,
    /// Yaklasan bitisler ve pencerenin gun sayisi
    pub upcoming: i64,
    pub upcoming_days: i32,
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
    tabs: Tabs,
    summary: Summary,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/reports", get(page))
}

async fn page(OperatorSession(op): OperatorSession, State(state): State<AppState>) -> Response {
    let time_zone = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    // Sayilar gidilecek ekranlarin kendi yardimcilarindan okunur, kopya SQL yok:
    // rozet "3" diyorsa liste uc satir gostermek zorunda.
    let loaded = tokio::try_join!(
        crate::deletions::pending_counts(&state.pool),
        crate::reconcile::unadopted(&state.pool),
        crate::upcoming::list(&state.pool, &time_zone, crate::upcoming::DEFAULT_DAYS),
        crate::bulk_manage::coverage(&state.pool),
    );
    let ((interventions, deletions), unadopted, upcoming, coverage) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => return internal("raporlar sayfası okunamadı", e),
    };
    let unadopted: i64 = unadopted.iter().map(|u| u.count).sum();
    render(&ReportsTemplate {
        lang: op.lang,
        shell: Shell::of(&op),
        tabs: Tabs::new(
            op.lang,
            "nav.reports",
            &crate::activity::REPORT_TABS,
            "/reports",
        ),
        summary: Summary {
            pending: interventions + deletions + unadopted,
            pending_href: pending_target(interventions, deletions, unadopted),
            pending_foot: op.lang.tn(
                "reports.pending_foot",
                &[
                    &interventions.to_string(),
                    &deletions.to_string(),
                    &unadopted.to_string(),
                ],
            ),
            upcoming: upcoming.len() as i64,
            upcoming_days: crate::upcoming::DEFAULT_DAYS,
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

/// Toplamin tek bir sayfasi yok: kutu dolu olan ilk listeye gider, hepsi bossa
/// mudahale listesine. Eskiden hep `/interventions`e gidiyordu ve "26" diyen
/// kutu (26'si sahiplenme) bos liste aciyordu.
pub fn pending_target(interventions: i64, deletions: i64, unadopted: i64) -> &'static str {
    match (interventions, deletions, unadopted) {
        (0, d, _) if d > 0 => "/deletions",
        (0, 0, u) if u > 0 => "/reconcile",
        _ => "/interventions",
    }
}

// --- END FEATURE: reports ---

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            pending: 26,
            pending_foot: String::new(),
            pending_href: "/reconcile",
            upcoming: 0,
            upcoming_days: 30,
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
            tabs: Tabs::new(
                crate::i18n::DEFAULT,
                "nav.reports",
                &crate::activity::REPORT_TABS,
                "/reports",
            ),
            summary,
        }
        .render()
        .unwrap()
    }

    /// Ozet kutusu dolu olan ilk listeye gider; liste kartlari Personel
    /// seridinde kaldi (ADR-134 madde 3).
    #[test]
    fn the_pending_box_links_to_the_first_list_with_work() {
        assert_eq!(pending_target(2, 5, 19), "/interventions");
        assert_eq!(pending_target(0, 5, 19), "/deletions");
        assert_eq!(pending_target(0, 0, 19), "/reconcile");
        assert_eq!(pending_target(0, 0, 0), "/interventions");

        let page = page(summary());
        let before = page
            .split_once(crate::i18n::DEFAULT.t("reports.pending"))
            .expect("bekleyen is kutusu yok")
            .0;
        let open = before
            .rfind("<a class=\"sum-box")
            .expect("kutu bağlantı değil");
        assert!(before[open..].contains(r#"href="/reconcile""#));
        assert!(
            page.contains(r#"href="/reports/activity""#),
            "etkinlik geçmişi sekmesi yok"
        );
        for list in ["/deletions", "/used-names"] {
            let link = format!("href=\"{list}\"");
            assert!(!page.contains(&link), "{list} Personel seridinde olmali");
        }
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
