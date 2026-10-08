// --- START FEATURE: activity-history ---
// Etkinlik gecmisi (ADR-134 madde 4–5): `audit_log` uzerinde filtreli, sayfali
// okuma ekrani. Filtreler GET parametresi, adres paylasilabilir. Yazma eylemi yok;
// okuma denetim kaydini bugun okuyan her yetkide (auditor dahil).
//
// Worker'in niyet ve sonuc satirlari (ADR-062) tek satirdir: sonuc niyete
// `intent_id` ile baglanir, ciplak sonuc listeye girmez — kisi sayfasinin akisiyla
// ayni kural. Sonucu olmayan niyet "sonucu bilinmiyor"dur.
//
// Olcum (2026-10-08, 1M satir, 90 gunluk pencerede 400k): sayim 0,7 sn + sayfa
// 0,8 sn; 5 sn sinirinin altinda, indeks acilmadi. Sayfa sorgusu once 50 satiri
// kesip sonra kisi ve hedefi birlestirir — pencere sayimiyla tek sorgu 4,2 sn'ydi.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use sqlx::postgres::PgArguments;
use sqlx::query::QueryAs;
use sqlx::{PgPool, Postgres};

use crate::dashboard::{glyph_for, EVENT_TYPES};
use crate::i18n::Lang;
use crate::identity_web::{
    allowed, empty_as_none, forbidden, internal, pagination, urlencode, Choice, OperatorSession,
    PAGE_SIZE,
};
use crate::shell::{Shell, Tabs};
use crate::web::{render, AppState};

/// `glyph_for`un bes kategorisi (ADR-117 B); islem filtresi bunlardan biri ya da
/// tek bir olay turu olabilir. Kategori adlarinda nokta yok, olay turlerinde var.
const CATEGORIES: [&str; 5] = ["auth", "config", "account", "danger", "other"];
const OUTCOMES: [&str; 3] = ["succeeded", "failed", "unknown"];
/// `actor=system`: worker'in satirlari (operator adi tasimaz)
const SYSTEM: &str = "system";

/// Raporlar calisma alaninin sekmeleri (ADR-134 madde 2); yetki dokumu ve
/// ayrilmis ama acik raporlari kendi kutucuklarinda eklenir.
pub(crate) const REPORT_TABS: [(&str, &str); 2] = [
    ("/reports/activity", "nav.activity"),
    ("/reports", "reports.coverage"),
];

#[derive(Deserialize, Default)]
pub struct Params {
    #[serde(default, deserialize_with = "empty_as_none")]
    identity: Option<i64>,
    /// Kisi arama metni; tek kisiye cozulurse `identity` olur
    person: Option<String>,
    actor: Option<String>,
    /// Yok = varsayilan (son 7 gun); bos = sinir yok
    from: Option<String>,
    to: Option<String>,
    event: Option<String>,
    #[serde(default, deserialize_with = "empty_as_none")]
    target: Option<i64>,
    outcome: Option<String>,
    #[serde(default, deserialize_with = "empty_as_none")]
    offset: Option<i64>,
}

/// Dogrulanmis filtreler: SQL'e giden ve forma geri basilan degerler. Bos metin
/// "filtre yok" demek.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub identity: Option<i64>,
    pub actor: String,
    pub from: String,
    pub to: String,
    pub event: String,
    pub target: Option<i64>,
    pub outcome: String,
}

impl Filters {
    /// Paylasilabilir adresin sorgu dizesi. `from`/`to` bos da olsa yazilir:
    /// parametrenin yoklugu "son 7 gun", bos degeri "sinir yok" demek.
    pub fn query(&self) -> String {
        let mut out = format!("from={}&to={}", urlencode(&self.from), urlencode(&self.to));
        let mut push = |key: &str, value: &str| {
            if !value.is_empty() {
                out.push_str(&format!("&{key}={}", urlencode(value)));
            }
        };
        push(
            "identity",
            &self.identity.map(|i| i.to_string()).unwrap_or_default(),
        );
        push("actor", &self.actor);
        push("event", &self.event);
        push(
            "target",
            &self.target.map(|i| i.to_string()).unwrap_or_default(),
        );
        push("outcome", &self.outcome);
        out
    }

    fn href(&self) -> String {
        format!("/reports/activity?{}", self.query())
    }
}

/// `YYYY-AA-GG` ve takvimde var olan bir gun mu. Postgres'e gecersiz tarih
/// giderse sorgu 500 ile duserdi; kural burada, sayfa uyariyla devam eder.
fn valid_day(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !digits(0..4) || !digits(5..7) {
        return false;
    }
    if !digits(8..10) {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(0);
    let (y, m, d) = (num(0..4), num(5..7), num(8..10));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let last = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    y > 0 && (1..=last).contains(&d)
}

/// Islem filtresinin SQL karsiligi: (turler, listede olmayan turler de mi).
/// `None` = tanınmayan deger. `other` kategorisi `glyph_for`un tanimadigi
/// turleri de kapsar — kategori nasil basiliyorsa oyle suzulur.
fn event_types(event: &str) -> Option<(Vec<&'static str>, bool)> {
    if CATEGORIES.contains(&event) {
        let types = EVENT_TYPES
            .iter()
            .copied()
            .filter(|t| glyph_for(t).1 == event)
            .collect();
        return Some((types, event == "other"));
    }
    EVENT_TYPES
        .iter()
        .find(|t| **t == event)
        .map(|t| (vec![*t], false))
}

/// Ham parametreleri dogrular. Gecersiz deger 400 degil "filtre yok" + uyari
/// (uyarinin alan etiketi anahtari doner). `default` = (bugun − 6, bugun).
fn validate(p: &Params, default: (&str, &str)) -> (Filters, Vec<&'static str>) {
    let mut bad = Vec::new();
    let mut day = |raw: &Option<String>, fallback: &str, label: &'static str| match raw {
        None => fallback.to_string(),
        Some(v) if v.trim().is_empty() => String::new(),
        Some(v) if valid_day(v.trim()) => v.trim().to_string(),
        Some(_) => {
            bad.push(label);
            String::new()
        }
    };
    let from = day(&p.from, default.0, "activity.from");
    let to = day(&p.to, default.1, "activity.to");
    let event = p.event.as_deref().unwrap_or("").trim().to_string();
    let event = match event.is_empty() || event_types(&event).is_some() {
        true => event,
        false => {
            bad.push("activity.event");
            String::new()
        }
    };
    let outcome = p.outcome.as_deref().unwrap_or("").trim().to_string();
    let outcome = match outcome.is_empty() || OUTCOMES.contains(&outcome.as_str()) {
        true => outcome,
        false => {
            bad.push("activity.outcome");
            String::new()
        }
    };
    let filters = Filters {
        identity: p.identity.filter(|id| *id > 0),
        actor: p.actor.as_deref().unwrap_or("").trim().to_string(),
        from,
        to,
        event,
        target: p.target.filter(|id| *id > 0),
        outcome,
    };
    (filters, bad)
}

macro_rules! outcome_expr {
    () => {
        "CASE WHEN o.outcome IS NOT NULL THEN o.outcome \
         WHEN a.operation_class IS NOT NULL THEN 'unknown' ELSE '' END"
    };
}

// $1 saat dilimi; $2..$10 filtreler (bos = yok). Tarih sinirlari kurulumun saat
// diliminde gun basidir. Sayim ve sayfa ayni kosulu buradan alir.
macro_rules! activity_where {
    () => {
        concat!(
            " FROM audit_log a LEFT JOIN audit_log o ON o.intent_id = a.id",
            " WHERE a.intent_id IS NULL",
            " AND ($2::date IS NULL OR a.occurred_at >= $2::date::timestamp AT TIME ZONE $1)",
            " AND ($3::date IS NULL OR a.occurred_at < ($3::date + 1)::timestamp AT TIME ZONE $1)",
            " AND ($4::bigint IS NULL OR a.identity_id = $4)",
            " AND ($5::text IS NULL OR CASE WHEN $5 = 'system' THEN a.actor_username IS NULL",
            " ELSE lower(a.actor_username) = lower($5) END)",
            " AND ($6::text[] IS NULL OR a.event_type = ANY($6) OR ($7 AND a.event_type <> ALL($8)))",
            " AND ($9::bigint IS NULL OR a.target_system_id = $9)",
            " AND ($10::text IS NULL OR ",
            outcome_expr!(),
            " = $10)"
        )
    };
}

const COUNT_SQL: &str = concat!("SELECT count(*)", activity_where!());

const PAGE_SQL: &str = concat!(
    "SELECT p.at, p.event_type, p.actor_username, p.identity_id, \
     nullif(btrim(i.given_name || ' ' || i.surname), ''), t.name, p.outcome, p.detail \
     FROM (SELECT a.id, a.occurred_at, \
     to_char(a.occurred_at AT TIME ZONE $1, 'YYYY-MM-DD HH24:MI') AS at, \
     a.event_type, a.actor_username, a.identity_id, a.target_system_id, ",
    outcome_expr!(),
    " AS outcome, a.detail::text AS detail",
    activity_where!(),
    " ORDER BY a.occurred_at DESC, a.id DESC LIMIT $11 OFFSET $12) p \
     LEFT JOIN identities i ON i.id = p.identity_id \
     LEFT JOIN target_systems t ON t.id = p.target_system_id \
     ORDER BY p.occurred_at DESC, p.id DESC"
);

fn nonempty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn bind<'q, O>(
    q: QueryAs<'q, Postgres, O, PgArguments>,
    tz: &str,
    f: &Filters,
) -> QueryAs<'q, Postgres, O, PgArguments> {
    let (types, unknown) = match f.event.is_empty() {
        true => (None, false),
        false => event_types(&f.event).map_or((None, false), |(t, u)| (Some(t), u)),
    };
    q.bind(tz.to_string())
        .bind(nonempty(&f.from))
        .bind(nonempty(&f.to))
        .bind(f.identity)
        .bind(nonempty(&f.actor))
        .bind(types)
        .bind(unknown)
        .bind(EVENT_TYPES)
        .bind(f.target)
        .bind(nonempty(&f.outcome))
}

type PageRow = (
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    String,
    String,
);

pub struct Row {
    pub at: String,
    /// i18n anahtari: `lang.key("event", …)`
    pub event_type: String,
    pub icon: &'static str,
    pub category: &'static str,
    /// Bos = worker; ekran "otomatik" der
    pub actor: String,
    /// Ayni filtrelerle yalnizca bu yapanin satirlari
    pub actor_href: String,
    pub identity_id: Option<i64>,
    pub person: String,
    pub target: String,
    pub outcome: String,
    pub outcome_kind: &'static str,
    /// Girintili JSON; bos nesne ise bos
    pub detail: String,
}

/// Toplam satir ve istenen sayfa.
pub async fn load(
    pool: &PgPool,
    tz: &str,
    f: &Filters,
    offset: i64,
) -> Result<(i64, Vec<Row>), sqlx::Error> {
    let count = bind(sqlx::query_as::<_, (i64,)>(COUNT_SQL), tz, f).fetch_one(pool);
    let page = bind(sqlx::query_as::<_, PageRow>(PAGE_SQL), tz, f)
        .bind(PAGE_SIZE)
        .bind(offset)
        .fetch_all(pool);
    let ((total,), rows) = tokio::try_join!(count, page)?;
    let rows = rows
        .into_iter()
        .map(
            |(at, event_type, actor, identity_id, person, target, outcome, detail)| {
                let (icon, category) = glyph_for(&event_type);
                let actor = actor.unwrap_or_default();
                let actor_href = Filters {
                    actor: match actor.is_empty() {
                        true => SYSTEM.to_string(),
                        false => actor.clone(),
                    },
                    ..f.clone()
                }
                .href();
                Row {
                    at,
                    icon,
                    category,
                    event_type,
                    actor,
                    actor_href,
                    identity_id,
                    person: match (person, identity_id) {
                        (Some(name), _) => name,
                        // ADR-024: kisisel verisi temizlenmis kimlik adsizdir
                        (None, Some(id)) => format!("#{id}"),
                        (None, None) => String::new(),
                    },
                    target: target.unwrap_or_default(),
                    outcome_kind: crate::identity::outcome_kind(&outcome),
                    outcome,
                    detail: pretty(&detail),
                }
            },
        )
        .collect();
    Ok((total, rows))
}

fn pretty(detail: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(detail) {
        Ok(serde_json::Value::Object(map)) if map.is_empty() => String::new(),
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| detail.to_string()),
        Err(_) => detail.to_string(),
    }
}

/// Islem secicisinin bir grubu: kategori secenegi + o kategorinin turleri.
pub struct EventGroup {
    pub category: &'static str,
    pub selected: bool,
    pub events: Vec<(&'static str, bool)>,
}

fn event_groups(current: &str) -> Vec<EventGroup> {
    CATEGORIES
        .iter()
        .map(|category| EventGroup {
            category,
            selected: current == *category,
            events: EVENT_TYPES
                .iter()
                .filter(|t| glyph_for(t).1 == *category)
                .map(|t| (*t, current == *t))
                .collect(),
        })
        .collect()
}

#[derive(Template)]
#[template(path = "activity.html")]
struct ActivityTemplate {
    lang: Lang,
    shell: Shell,
    tabs: Tabs,
    f: Filters,
    /// Kisi filtresi aciksa adi ve filtreyi kaldiran adres
    person_name: String,
    remove_person: String,
    /// Kisi filtresi yokken arama kutusunun metni
    person_text: String,
    /// Arama birden cok kisiye uydu: (adres, ad)
    matches: Vec<(String, String)>,
    warnings: Vec<String>,
    groups: Vec<EventGroup>,
    targets: Vec<Choice>,
    /// (sonuc, secili mi)
    outcomes: Vec<(&'static str, bool)>,
    rows: Vec<Row>,
    range: String,
    prev: Option<String>,
    next: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/reports/activity", get(page))
}

/// Kisi filtresinin cozumu: (ad, birden cok eslesme, uyarilar). Yeni arama
/// yazilmaz — ust bardaki aramanin sorgusu kisiyi bulur (ADR-134 madde 4),
/// filtre `identity_id` tasir. Bilinmeyen kimlik filtresi duser.
async fn resolve_person(
    pool: &PgPool,
    tz: &str,
    lang: Lang,
    f: &mut Filters,
    text: &str,
) -> Result<(String, Vec<(String, String)>, Vec<String>), sqlx::Error> {
    let mut matches = Vec::new();
    let mut warnings = Vec::new();
    if f.identity.is_none() && !text.is_empty() {
        match crate::identity::search(pool, tz, text).await?.as_slice() {
            [one] => f.identity = Some(one.id),
            [] => warnings.push(lang.t1("activity.person_none", text)),
            many => {
                warnings.push(lang.t1("activity.person_many", text));
                let pick = |id| {
                    Filters {
                        identity: Some(id),
                        ..f.clone()
                    }
                    .href()
                };
                matches = many.iter().map(|l| (pick(l.id), l.name.clone())).collect();
            }
        }
    }
    let Some(id) = f.identity else {
        return Ok((String::new(), matches, warnings));
    };
    let name: Option<String> = sqlx::query_scalar(
        "SELECT coalesce(nullif(btrim(given_name || ' ' || surname), ''), '#' || id) \
         FROM identities WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if name.is_none() {
        warnings.push(lang.t1("activity.bad_filter", lang.t("activity.person")));
        f.identity = None;
    }
    Ok((name.unwrap_or_default(), matches, warnings))
}

/// Varsayilan pencere: kurulumun saat diliminde (bugun − 6, bugun).
async fn default_window(pool: &PgPool, tz: &str) -> Result<(String, String), sqlx::Error> {
    sqlx::query_as(
        "SELECT to_char(d - 6, 'YYYY-MM-DD'), to_char(d, 'YYYY-MM-DD') \
         FROM (SELECT (now() AT TIME ZONE $1)::date AS d) x",
    )
    .bind(tz)
    .fetch_one(pool)
    .await
}

async fn page(
    OperatorSession(op): OperatorSession,
    State(state): State<AppState>,
    Query(p): Query<Params>,
) -> Response {
    // Denetim kaydi bugun her yetkiye okunuyor (panel akisi, kisi sayfasi);
    // yetkisi olmayan oturum (ADR-095: gruba uye olmayan AD kullanicisi) okuyamaz.
    if !allowed(&op, &crate::shell::AUTHORITY_ORDER) {
        return forbidden(op.lang);
    }
    let tz = match state.time_zone().await {
        Ok(tz) => tz,
        Err(response) => return *response,
    };
    let lang = op.lang;
    let window = match default_window(&state.pool, &tz).await {
        Ok(w) => w,
        Err(e) => return internal("etkinlik geçmişi tarihi okunamadı", e),
    };
    let (mut f, bad) = validate(&p, (&window.0, &window.1));
    let person_text = p.person.as_deref().unwrap_or("").trim().to_string();
    let (person_name, matches, person_warnings) =
        match resolve_person(&state.pool, &tz, lang, &mut f, &person_text).await {
            Ok(resolved) => resolved,
            Err(e) => return internal("etkinlik geçmişi kişi filtresi", e),
        };
    let warnings = bad
        .iter()
        .map(|label| lang.t1("activity.bad_filter", lang.t(label)))
        .chain(person_warnings)
        .collect();
    let offset = p.offset.unwrap_or(0).max(0);
    let loaded = tokio::try_join!(
        load(&state.pool, &tz, &f, offset),
        crate::org::list_targets(&state.pool),
    );
    let ((total, rows), targets) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => return internal("etkinlik geçmişi okunamadı", e),
    };
    let (range, prev, next) = pagination(lang, offset, rows.len() as i64, total);
    let page_href = |o: i64| format!("{}&offset={o}", f.href());
    render(&ActivityTemplate {
        lang,
        shell: Shell::of(&op),
        tabs: Tabs::new(lang, "nav.reports", &REPORT_TABS, "/reports/activity"),
        remove_person: Filters {
            identity: None,
            ..f.clone()
        }
        .href(),
        person_name,
        person_text,
        matches,
        warnings,
        groups: event_groups(&f.event),
        targets: targets
            .into_iter()
            .map(|t| Choice {
                selected: f.target == Some(t.id),
                id: t.id,
                label: t.name,
            })
            .collect(),
        outcomes: OUTCOMES.iter().map(|o| (*o, f.outcome == *o)).collect(),
        prev: prev.map(page_href),
        next: next.map(page_href),
        rows,
        range,
        f,
    })
}
// --- END FEATURE: activity-history ---

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: (&str, &str) = ("2026-10-02", "2026-10-08");

    #[test]
    fn days_must_exist_on_the_calendar() {
        for ok in ["2026-10-08", "2024-02-29", "2000-02-29", "2026-12-31"] {
            assert!(valid_day(ok), "{ok}");
        }
        for bad in [
            "2026-02-29",
            "1900-02-29",
            "2026-13-01",
            "2026-04-31",
            "2026-1-01",
            "26-10-08",
            "2026/10/08",
            "+026-10-08",
            "2026-10-0x",
            "0000-01-01",
            "",
        ] {
            assert!(!valid_day(bad), "{bad}");
        }
    }

    /// Yok = son yedi gun, bos = sinir yok, bozuk = sinir yok + uyari (400 degil).
    #[test]
    fn missing_empty_and_broken_dates_are_three_different_things() {
        let (f, bad) = validate(&Params::default(), DEFAULT);
        assert_eq!((f.from.as_str(), f.to.as_str()), DEFAULT);
        assert!(bad.is_empty());
        let p = Params {
            from: Some(String::new()),
            to: Some("2026-02-30".into()),
            ..Default::default()
        };
        let (f, bad) = validate(&p, DEFAULT);
        assert_eq!((f.from.as_str(), f.to.as_str()), ("", ""));
        assert_eq!(bad, vec!["activity.to"]);
    }

    #[test]
    fn unknown_event_and_outcome_are_dropped_with_a_warning() {
        let p = Params {
            event: Some("'; DROP TABLE audit_log; --".into()),
            outcome: Some("maybe".into()),
            identity: Some(-3),
            ..Default::default()
        };
        let (f, bad) = validate(&p, DEFAULT);
        assert_eq!(
            (f.event.as_str(), f.outcome.as_str(), f.identity),
            ("", "", None)
        );
        assert_eq!(bad, vec!["activity.event", "activity.outcome"]);
    }

    /// Kategori `glyph_for`un kendisinden: "danger" ayrilisi kapsar, oturum
    /// acmayi kapsamaz; "other" listede olmayan turleri de alir.
    #[test]
    fn a_category_expands_to_the_types_glyph_for_paints_that_way() {
        let (danger, unknown) = event_types("danger").unwrap();
        assert!(danger.contains(&crate::audit::IDENTITY_DEPARTURE_SET));
        assert!(danger.contains(&"ad.account.delete"));
        assert!(!danger.contains(&crate::audit::OPERATOR_LOGIN));
        assert!(!unknown);
        let (other, unknown) = event_types("other").unwrap();
        assert!(other.is_empty() && unknown);
        assert_eq!(
            event_types(crate::audit::OPERATOR_LOGIN),
            Some((vec![crate::audit::OPERATOR_LOGIN], false))
        );
        assert_eq!(event_types("identity"), None);
    }

    /// Paylasilan adres filtreyi geri kurar: `from`/`to` bos da olsa yazilir,
    /// metin kacirilir.
    #[test]
    fn the_query_string_round_trips_and_escapes() {
        let f = Filters {
            identity: Some(7),
            actor: "ayşe k&x".into(),
            from: String::new(),
            to: "2026-10-08".into(),
            event: "danger".into(),
            target: None,
            outcome: "unknown".into(),
        };
        assert_eq!(
            f.query(),
            "from=&to=2026-10-08&identity=7&actor=ay%C5%9Fe+k%26x&event=danger&outcome=unknown"
        );
    }

    #[test]
    fn empty_detail_is_hidden_and_objects_are_indented() {
        assert_eq!(pretty("{}"), "");
        assert_eq!(pretty(r#"{"a":1}"#), "{\n  \"a\": 1\n}");
    }

    async fn cookie(pool: &PgPool, authorities: &[&str]) -> String {
        let operator = crate::operator_session::Operator {
            subject: "sub-a".to_string(),
            username: "denetci".to_string(),
            email: "d@example.org".to_string(),
            authorities: authorities.iter().map(|a| a.to_string()).collect(),
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(pool, &operator)
            .await
            .unwrap();
        format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME)
    }

    // Gercek Postgres gerektirir (ADR-070).
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn filters_alone_and_together_merge_intent_with_outcome_and_page() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let [ayse, ali] = crate::test_support::seed_two_identities(&pool).await;
        let ad: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        // Sahip baglantisi: niyet/sonuc kolonlari servis rolune kapali (0006)
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO audit_log (event_type, actor_username, identity_id, occurred_at) VALUES \
               ('operator.login', 'ik.uzmani', NULL, now() - interval '1 hour'), \
               ('identity.changed', 'ik.uzmani', {ayse}, now() - interval '2 hours'), \
               ('identity.departure_set', 'Mudur', {ali}, now() - interval '3 days'), \
               ('identity.created', 'ik.uzmani', {ali}, now() - interval '20 days'), \
               ('legacy.event', NULL, NULL, now() - interval '4 hours'); \
             WITH i AS (INSERT INTO audit_log (event_type, identity_id, target_system_id, \
                   operation_class, occurred_at) VALUES \
                   ('ad.account.disable', {ali}, {ad}, 'destructive', now() - interval '3 days 1 minute') \
                   RETURNING id) \
             INSERT INTO audit_log (event_type, identity_id, target_system_id, intent_id, outcome) \
             SELECT 'ad.account.disable', {ali}, {ad}, id, 'failed' FROM i; \
             INSERT INTO audit_log (event_type, identity_id, target_system_id, operation_class, \
                   occurred_at) VALUES \
                   ('ad.group.add_member', {ayse}, {ad}, 'grant', now() - interval '5 hours');"
        )))
        .execute(&pool)
        .await
        .unwrap();
        let tz = "Europe/Istanbul";
        let open = Filters::default();
        let events = |rows: Vec<Row>| rows.into_iter().map(|r| r.event_type).collect::<Vec<_>>();
        let only = |f: Filters| {
            let pool = pool.clone();
            async move { events(load(&pool, tz, &f, 0).await.unwrap().1) }
        };

        // Filtresiz: 8 satir yazildi, ciplak sonuc satiri listeye girmez → 7
        let (total, rows) = load(&pool, tz, &open, 0).await.unwrap();
        assert_eq!(total, 7);
        let disable = rows
            .iter()
            .find(|r| r.event_type == "ad.account.disable")
            .unwrap();
        assert_eq!(
            (disable.outcome.as_str(), disable.outcome_kind),
            ("failed", "err")
        );
        assert_eq!(disable.target, "Active Directory");
        assert!(disable.actor.is_empty());
        let grant = rows
            .iter()
            .find(|r| r.event_type == "ad.group.add_member")
            .unwrap();
        assert_eq!(
            (grant.outcome.as_str(), grant.outcome_kind),
            ("unknown", "warn")
        );
        assert_eq!(rows[0].event_type, "operator.login", "yeniden eskiye");

        // Tek tek
        assert_eq!(
            only(Filters {
                identity: Some(ali),
                ..open.clone()
            })
            .await,
            [
                "identity.departure_set",
                "ad.account.disable",
                "identity.created"
            ]
        );
        assert_eq!(
            only(Filters {
                actor: "mudur".into(),
                ..open.clone()
            })
            .await,
            ["identity.departure_set"],
            "kullanıcı adı büyük-küçük harf duyarsız"
        );
        assert_eq!(
            only(Filters {
                actor: SYSTEM.into(),
                ..open.clone()
            })
            .await,
            ["legacy.event", "ad.group.add_member", "ad.account.disable"]
        );
        assert_eq!(
            only(Filters {
                event: "danger".into(),
                ..open.clone()
            })
            .await,
            ["identity.departure_set", "ad.account.disable"]
        );
        assert_eq!(
            only(Filters {
                event: "other".into(),
                ..open.clone()
            })
            .await,
            ["legacy.event"],
            "listede olmayan tür 'diğer'"
        );
        assert_eq!(
            only(Filters {
                event: "operator.login".into(),
                ..open.clone()
            })
            .await,
            ["operator.login"]
        );
        assert_eq!(
            only(Filters {
                target: Some(ad),
                ..open.clone()
            })
            .await,
            ["ad.group.add_member", "ad.account.disable"]
        );
        assert_eq!(
            only(Filters {
                outcome: "unknown".into(),
                ..open.clone()
            })
            .await,
            ["ad.group.add_member"]
        );
        assert_eq!(
            only(Filters {
                outcome: "failed".into(),
                ..open.clone()
            })
            .await,
            ["ad.account.disable"]
        );
        let (week_ago, today) = default_window(&pool, tz).await.unwrap();
        let week = Filters {
            from: week_ago,
            to: today,
            ..open.clone()
        };
        assert_eq!(
            load(&pool, tz, &week, 0).await.unwrap().0,
            6,
            "20 gün önceki dışarıda"
        );
        // Birlikte
        assert_eq!(
            only(Filters {
                identity: Some(ali),
                event: "danger".into(),
                actor: SYSTEM.into(),
                ..week.clone()
            })
            .await,
            ["ad.account.disable"]
        );

        // Sayfa siniri: PAGE_SIZE + 3 satirda ikinci sayfa 3 + 7 = 10 satir
        sqlx::query(
            "INSERT INTO audit_log (event_type, actor_username, occurred_at) \
             SELECT 'operator.login', 'yuk', now() - interval '10 days' - g * interval '1 second' \
             FROM generate_series(1, $1) g",
        )
        .bind(PAGE_SIZE + 3)
        .execute(&pool)
        .await
        .unwrap();
        let (total, first) = load(&pool, tz, &open, 0).await.unwrap();
        assert_eq!((total, first.len() as i64), (PAGE_SIZE + 10, PAGE_SIZE));
        let (_, second) = load(&pool, tz, &open, PAGE_SIZE).await.unwrap();
        assert_eq!(second.len(), 10);

        // HTTP: auditor okur, yetkisiz oturum 403; bozuk tarih 400 degil uyari
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let get = |uri: String, cookie: String| {
            let app = app.clone();
            async move {
                let res = app
                    .oneshot(
                        Request::builder()
                            .uri(uri)
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = res.status();
                let body = axum::body::to_bytes(res.into_body(), usize::MAX)
                    .await
                    .unwrap();
                (status, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let auditor = cookie(&pool, &["auditor"]).await;
        let (status, page) = get(
            format!("/reports/activity?identity={ali}&from=&to=2026-02-30"),
            auditor.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("Ali Kaya"), "kişi filtresi adıyla basılır");
        let lang = crate::i18n::DEFAULT;
        // Sablon tirnaklari kacirir; uyari etiketiyle ve kuyruguyla aranir
        let warning = lang.t1("activity.bad_filter", lang.t("activity.to"));
        let tail = warning.rsplit('"').next().unwrap();
        let alert = page.split_once("alert-warn").expect("uyarı yok").1;
        assert!(
            alert.contains(lang.t("activity.to")) && alert.contains(tail),
            "bozuk tarih uyarısı: {warning}"
        );
        let content = page.split_once("<main").expect("main yok").1;
        assert!(
            !content.contains("method=\"post\""),
            "sayfada yazma eylemi yok"
        );
        let (_, page) = get(
            "/reports/activity?person=Kaya&from=".into(),
            auditor.clone(),
        )
        .await;
        assert!(page.contains("Ali Kaya"), "tek eşleşme kişiye çözülür");
        assert!(page.contains(r#"aria-current="page""#));
        let none = cookie(&pool, &[]).await;
        assert_eq!(
            get("/reports/activity".into(), none).await.0,
            StatusCode::FORBIDDEN
        );

        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }
}
