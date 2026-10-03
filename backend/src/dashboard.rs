// --- START FEATURE: dashboard ---
// Gosterge paneli (ADR-076 v1 kapsami, ADR-096 madde 3 ile ana sayfaya alindi).
// Butun sayilar Faz 2-3'te kurulan tablolardan okunur: yeni migration, yeni
// bagimlilik ve grafik kutuphanesi yok. Grafikler native CSS (ADR-076): yuzdeler
// burada en yakin bese yuvarlanir, sablon `.v-NN` sinifini secer -- nginx CSP'si
// satir ici `style`e izin vermiyor (ADR-088 madde 4).

use sqlx::PgPool;

use crate::audit;

/// Panelin pencereleri; "son 30 gun" ADR-076'nin varsayilani.
pub const WINDOW_DAYS: i32 = 30;
/// Secilebilir pencereler (ADR-076 tarih araligi filtresi); listede olmayan
/// deger varsayilana duser — sorguya keyfi sayi girmez.
pub const WINDOWS: [i32; 4] = [7, 30, 90, 365];
const TREND_DAYS: i32 = 7;
/// Akis karti yanindaki sutun kadar uzuyor; sekiz satir altinda genis bir
/// bosluk birakiyordu (ADR-117 C).
const ACTIVITY_LIMIT: i64 = 12;
const DISTRIBUTION_LIMIT: i64 = 5;
/// Rol halkasinda ayri dilim olan en kalabalik rol sayisi; kalani "diger"
const ROLE_SLICES: usize = 4;

/// `?days=` degeri: yalnizca `WINDOWS`'tan biri, aksi halde varsayilan.
pub fn window(requested: Option<i32>) -> i32 {
    requested
        .filter(|d| WINDOWS.contains(d))
        .unwrap_or(WINDOW_DAYS)
}

pub struct WindowChip {
    pub days: i32,
    pub active: bool,
}

pub struct Dashboard {
    /// Hos geldiniz kartindaki tarih, kurulum saat diliminde (ADR-039)
    pub today: String,
    /// Sayac penceresi (gun) ve ekrandaki secenekler; sablon karsilastirma yapmaz
    pub window_days: i32,
    pub windows: Vec<WindowChip>,
    pub totals: Totals,
    /// Dort sayac karti (ADR-117): sayi + onceki doneme gore degisim + pencerenin
    /// gunluk serisi. Sablon dort bloku tekrarlamaz, listeyi gezer.
    pub cards: Vec<StatCard>,
    /// Birincil role gore dagilim (halka): en kalabalik dort rol + "diger"
    pub roles: Donut,
    pub activity: Vec<Event>,
    pub trend: Vec<TrendDay>,
    /// Grafigin y ekseninin tepesi: en kalabalik gunun toplami. Sablon ekseni
    /// bundan ve yarisindan yazar — cubugun yaninda sayi olmadan yukseklik
    /// "iki mi iki yuz mu" sorusunu cevaplamiyordu.
    pub trend_peak: i64,
    /// Ortadaki eksen yazısı; zirve tek sayıysa boş. Tam sayı bölmesi tek
    /// zirvede yalan söylüyordu: çizgi 1,5'ta dururken yazı "1" oluyordu.
    pub trend_mid: String,
    pub departments: Vec<DistRow>,
    /// Calisma tipine gore personel dagilimi (halka grafik)
    pub employment: Donut,
    /// Sahiplenmeyi bekleyen AD hesaplari (ADR-103 madde 3): panel sirada ne
    /// yapilacagini soyler, operator mutabakat ekranini aramaz. Bos liste =
    /// serit hic basilmaz.
    pub unadopted: Vec<crate::reconcile::Unadopted>,
    /// Devreye alma karti (ADR-103 madde 2)
    pub setup: Setup,
    /// ADR-054: worker kuru calistirmada — panelde kalici serit; worker hic
    /// yazmadiysa (ilk acilis) serit yok
    pub dry_run: bool,
}

/// Devreye alma karti: kurulumun dort adimi mevcut tablolardan **tek sorguyla**
/// turetilir (ADR-103 madde 2). Yeni tablo ve "kurulum bitti" bayragi yok;
/// dordu tamamsa kart hic basilmaz.
pub struct Setup {
    pub steps: Vec<SetupStep>,
    pub done: bool,
}

pub struct SetupStep {
    /// Baslik anahtari (`setup.ad` …); metin i18n'de
    pub key: &'static str,
    /// Aciklama anahtari
    pub hint: &'static str,
    pub done: bool,
    pub icon: &'static str,
    /// Adimin kendi ekran(lar)i; son adim iki yol gosterir (ADR-103 madde 2)
    pub links: Vec<SetupLink>,
    /// Yalnizca `admin`in acabildigi ekran: yetkisiz operatore baglanti
    /// gosterilmez, olmayan bir kapiyi isaret etmek olurdu (bkz. `Shell::is_admin`)
    pub admin_only: bool,
    /// Henuz ekrani olmayan ikinci yol icin not anahtari; bos = not yok
    pub note: &'static str,
}

pub struct SetupLink {
    pub key: &'static str,
    pub href: String,
}

const SETUP_STEPS: usize = 4;

/// Halka grafik: dilimler SVG `stroke-dasharray` ile cizilir. `conic-gradient`
/// dilim acisini ancak satir ici `style` ya da yuzde basina ayri bir sinifla
/// alabilirdi (CSP satir ici stili yasakliyor, ADR-088); SVG sunum oznitelikleri
/// `style-src`e takilmaz ve tam deger tasir.
pub struct Donut {
    pub total: i64,
    pub slices: Vec<Slice>,
}

pub struct Slice {
    /// Veritabani anahtari; ekran karsiligi `lang.key("employment", …)`
    pub key: String,
    pub count: i64,
    pub pct: i64,
    /// `stroke-dasharray`: dilim uzunlugu + kalani (cember 100 birime ayarli)
    pub dash: String,
    /// `stroke-dashoffset`: dilim 12 yonunden baslasin diye 25'ten geri sayilir
    pub offset: i64,
    /// Dilim rengi sinifi (`.seg-<ton>`)
    pub tone: &'static str,
}

/// Dilim renkleri, paletin grafik sirasi (mavi, turkuaz, mor, turuncu).
/// `employment_type` dort degerle sinirli (0004_identity_model.sql), liste yeter.
const SLICE_TONES: [&str; 4] = ["accent", "cyan", "info", "warn"];

/// Sayac karti (ADR-117). `key` hem kartin rengini (`.stat--<key>`) hem
/// kivilcim gradyaninin id'sini verir; dort anahtar sabit ve benzersizdir.
pub struct StatCard {
    pub key: &'static str,
    /// Baslik anahtari (`lang.t`)
    pub label: &'static str,
    pub icon: &'static str,
    /// Kartin tamami bu adrese giden bir baglanti
    pub href: String,
    pub value: i64,
    /// Alt satirin metin anahtari; bos ise sablon pencereyi yazar
    pub foot: &'static str,
    pub delta: Delta,
    pub spark: Spark,
}

/// Onceki esit uzunluktaki doneme gore degisim rozeti.
pub struct Delta {
    /// Mutlak yuzde; `kind` yonu ayri tasir ki sablon isaret hesaplamasin
    pub pct: i64,
    /// `up` | `down` | `flat` | `new`
    pub kind: &'static str,
    /// Rozet tonu: artis her kartta iyi degil — ayrilis artarsa kotu
    pub tone: &'static str,
    /// Onceki donemin sayisi; rozetin `title`inda gorunur
    pub before: i64,
}

/// Kartin tabanindaki kivilcim: `<polyline>` ve `<polygon>` nokta dizileri.
/// viewBox 0 0 100 100 ve `preserveAspectRatio="none"` — kart ne kadar genis
/// olursa olsun cizgi tabanı doldurur, kalinligi `vector-effect` sabitler.
pub struct Spark {
    pub line: String,
    pub area: String,
}

/// Pencerenin gunluk serisi; her alan gun sayisi kadar uzun.
struct Series {
    joined: Vec<i64>,
    departed: Vec<i64>,
    changed: Vec<i64>,
}

/// Onceki esit uzunluktaki pencerenin sayilari (degisim rozetinin paydasi).
struct Previous {
    joined: i64,
    departed: i64,
    changed: i64,
}

pub struct Totals {
    pub identities: i64,
    pub joined: i64,
    pub departed: i64,
    pub changed: i64,
    /// Hedefe uygulanamamis is: ADR-076'nin "ayrilmis ama kapatilamamis" kutusu
    pub needs_intervention: i64,
    /// Mudahaledeki islerden birinin kimligi; serit tek isi dogrudan acar
    intervention_identity: Option<i64>,
    /// Onay bekleyen rol/departman taslagi (ADR-031)
    pub pending_approvals: i64,
    /// Rolu yer tutucu (`Tanimsiz`) olan kisi — operatorun is listesi (ADR-103 madde 4)
    pub role_unassigned: i64,
}

impl Totals {
    /// Seridin kisa yolu: tek is varsa dogrudan o kisinin sayfasi, birden
    /// fazlaysa mudahale listesi. Operator "1 is bekliyor" deyip neyin
    /// beklediğini aramak zorunda kalmasin.
    pub fn intervention_href(&self) -> String {
        match (self.needs_intervention, self.intervention_identity) {
            (1, Some(id)) => format!("/identities/{id}"),
            _ => "/interventions".to_string(),
        }
    }
}

pub struct Event {
    /// i18n anahtari: `lang.key("event", …)`
    pub event_type: String,
    pub actor: String,
    pub person: String,
    /// "HH:MM" — bugun ve dunun satirlari saati yazar
    pub time: String,
    /// "DD.MM" — daha eskiler gunu yazar
    pub date: String,
    /// `today` | `yesterday` | `older`; "dun" cevrilecek bir dize oldugu icin
    /// metni sablon secer, Rust saati ve gunu verir (ADR-089)
    pub when: &'static str,
    pub icon: &'static str,
    /// Olay kategorisi (`.feed-ico--<kategori>`): akis tek renk kare dizisiyken
    /// olay turleri birbirinden ayirt edilemiyordu (ADR-117 B)
    pub category: &'static str,
    /// Satirin gittigi kayit; bos = satir tiklanamaz
    pub href: String,
}

pub struct TrendDay {
    pub label: String,
    pub joined: i64,
    pub departed: i64,
    /// Beser adimli yukseklik yuzdesi (`.v-NN`)
    pub joined_pct: i64,
    pub departed_pct: i64,
}

pub struct DistRow {
    pub name: String,
    pub count: i64,
    /// Beser adimli genislik yuzdesi (`.v-NN`)
    pub pct: i64,
}

pub async fn load(pool: &PgPool, time_zone: &str, days: i32) -> Result<Dashboard, sqlx::Error> {
    let (totals, today) = totals(pool, time_zone, days).await?;
    let (trend, trend_peak) = trend(pool, time_zone).await?;
    let cards = cards(
        &totals,
        days,
        &series(pool, time_zone, days).await?,
        previous(pool, time_zone, days).await?,
    );
    Ok(Dashboard {
        today,
        window_days: days,
        cards,
        windows: WINDOWS
            .iter()
            .map(|&d| WindowChip {
                days: d,
                active: d == days,
            })
            .collect(),
        totals,
        roles: roles(pool).await?,
        activity: activity(pool, time_zone).await?,
        trend,
        trend_mid: if trend_peak >= 2 && trend_peak % 2 == 0 {
            (trend_peak / 2).to_string()
        } else {
            String::new()
        },
        trend_peak,
        departments: departments(pool).await?,
        employment: employment(pool).await?,
        unadopted: crate::reconcile::unadopted(pool).await?,
        setup: setup(pool).await?,
        dry_run: sqlx::query_scalar("SELECT dry_run FROM worker_status")
            .fetch_optional(pool)
            .await?
            .unwrap_or(false),
    })
}

/// Devreye alma adimlari: dort kosul ve AD hedefinin id'si tek sorguda.
/// "Katalog tarandi mi" kayip isaretli ogeyi saymaz — eski ortamdan kalan
/// kayip satirlar adimi tamam gostermesin (ADR-014: katalog oge silmez).
/// "Rol tanimli mi" seed'li yer tutucu rolu saymaz (ADR-103 madde 4).
async fn setup(pool: &PgPool) -> Result<Setup, sqlx::Error> {
    let row: (bool, bool, bool, bool, Option<i64>) = sqlx::query_as(
        "SELECT (SELECT ad_host <> '' FROM app_settings), \
                EXISTS (SELECT 1 FROM catalog_items WHERE missing_since IS NULL), \
                EXISTS (SELECT 1 FROM departments) \
                  AND EXISTS (SELECT 1 FROM roles WHERE NOT placeholder), \
                EXISTS (SELECT 1 FROM identities WHERE deleted_at IS NULL), \
                (SELECT id FROM target_systems WHERE kind = 'ad')",
    )
    .fetch_one(pool)
    .await?;
    Ok(setup_steps([row.0, row.1, row.2, row.3], row.4))
}

/// Aciklama anahtari her zaman `<key>_hint`; derleme aninda birlestirilir ki
/// iki metin anahtari ayri ayri yazilip birbirinden sapmasin.
macro_rules! setup_step {
    ($key:literal, $icon:literal, $done:expr, $links:expr) => {
        SetupStep {
            key: $key,
            hint: concat!($key, "_hint"),
            done: $done,
            icon: $icon,
            links: $links,
            admin_only: false,
            note: "",
        }
    };
}

/// Adim listesi; saf — DB olmadan sinanir.
fn setup_steps(done: [bool; SETUP_STEPS], ad_target: Option<i64>) -> Setup {
    // Toplu sahiplenme hedefin mutabakat ekraninda. AD satiri migration'da
    // seed edilir; yoksa (teorik) rapor kapagina duser, sablon bos href basmaz.
    let adopt = match ad_target {
        Some(id) => format!("/targets/{id}/reconcile"),
        None => "/reports".to_string(),
    };
    let mut ad = setup_step!(
        "setup.ad",
        "ico-server",
        done[0],
        vec![link("nav.settings", "/config")]
    );
    ad.admin_only = true;
    let mut staff = setup_step!(
        "setup.staff",
        "ico-users",
        done[3],
        vec![link("adopt.title", &adopt)]
    );
    // Ikinci yol CSV (ADR-103 madde 1 B1); ekrani Faz 5'te geliyor
    staff.note = "setup.staff_csv";
    Setup {
        done: done.iter().all(|step| *step),
        steps: vec![
            ad,
            setup_step!(
                "setup.catalog",
                "ico-sitemap",
                done[1],
                vec![link("nav.targets", "/targets")]
            ),
            setup_step!(
                "setup.model",
                "ico-key",
                done[2],
                vec![
                    link("nav.departments", "/departments"),
                    link("nav.roles", "/roles"),
                ]
            ),
            staff,
        ],
    }
}

fn link(key: &'static str, href: &str) -> SetupLink {
    SetupLink {
        key,
        href: href.to_string(),
    }
}

/// Alti sayi, serit kisa yolunun kimligi ve bugunun tarihi tek sorguda: panel
/// acilirken sekiz ayri gidis donus olmasin. `days`: giris/ayrilis/degisiklik penceresi.
async fn totals(
    pool: &PgPool,
    time_zone: &str,
    days: i32,
) -> Result<(Totals, String), sqlx::Error> {
    let row: (i64, i64, i64, i64, i64, Option<i64>, i64, String) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL), \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL \
              AND start_date > (now() AT TIME ZONE $1)::date - $2), \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL \
              AND end_at IS NOT NULL AND end_at <= now() \
              AND end_at > now() - make_interval(days => $2)), \
           (SELECT count(DISTINCT identity_id) FROM audit_log \
              WHERE event_type = $3 AND occurred_at > now() - make_interval(days => $2)), \
           (SELECT count(*) FROM jobs WHERE status = $4), \
           (SELECT min(identity_id) FROM jobs WHERE status = $4), \
           (SELECT count(*) FROM roles WHERE pending_definition IS NOT NULL) \
           + (SELECT count(*) FROM departments WHERE pending_definition IS NOT NULL), \
           to_char(now() AT TIME ZONE $1, 'DD.MM.YYYY')",
    )
    .bind(time_zone)
    .bind(days)
    .bind(audit::IDENTITY_CHANGED)
    .bind(crate::identity::INTERVENTION_STATUS)
    .fetch_one(pool)
    .await?;
    Ok((
        Totals {
            identities: row.0,
            joined: row.1,
            departed: row.2,
            changed: row.3,
            needs_intervention: row.4,
            intervention_identity: row.5,
            pending_approvals: row.6,
            // Personel listesiyle ayni yardimci, ayni sayi
            role_unassigned: crate::identity::unassigned_role_count(pool).await?,
        },
        row.7,
    ))
}

/// Pencerenin gunluk serisi: her gun icin kayit / ayrilis / gorev degisikligi.
/// Gunler `generate_series` ile uretilir, olaysiz gun de dizide 0 olarak durur —
/// kivilcim cizgisinin x ekseni esit araliklidir.
///
/// Esikler `totals`takilerle ayni kurallari yazar ama **gune hizalanmistir**
/// (`totals` yuvarlanan bir zaman damgasi penceresi kullanir). Sinir gununun
/// yarisi iki tarafta ayni kalmayabilir: seri bir sekildir, sayinin kendisi
/// karttaki rakamdan okunur.
async fn series(pool: &PgPool, time_zone: &str, days: i32) -> Result<Series, sqlx::Error> {
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH gun AS ( \
           SELECT generate_series((now() AT TIME ZONE $1)::date - ($3::int - 1), \
                                  (now() AT TIME ZONE $1)::date, \
                                  interval '1 day')::date AS d), \
         giren AS ( \
           SELECT start_date AS d, count(*) AS n FROM identities \
            WHERE deleted_at IS NULL \
              AND start_date > (now() AT TIME ZONE $1)::date - $3::int \
            GROUP BY 1), \
         ayrilan AS ( \
           SELECT (end_at AT TIME ZONE $1)::date AS d, count(*) AS n FROM identities \
            WHERE deleted_at IS NULL AND end_at IS NOT NULL AND end_at <= now() \
              AND (end_at AT TIME ZONE $1)::date > (now() AT TIME ZONE $1)::date - $3::int \
            GROUP BY 1), \
         degisen AS ( \
           SELECT (occurred_at AT TIME ZONE $1)::date AS d, \
                  count(DISTINCT identity_id) AS n FROM audit_log \
            WHERE event_type = $2 \
              AND (occurred_at AT TIME ZONE $1)::date > (now() AT TIME ZONE $1)::date - $3::int \
            GROUP BY 1) \
         SELECT coalesce(giren.n, 0), coalesce(ayrilan.n, 0), coalesce(degisen.n, 0) \
           FROM gun \
           LEFT JOIN giren ON giren.d = gun.d \
           LEFT JOIN ayrilan ON ayrilan.d = gun.d \
           LEFT JOIN degisen ON degisen.d = gun.d \
          ORDER BY gun.d",
    )
    .bind(time_zone)
    .bind(audit::IDENTITY_CHANGED)
    .bind(days)
    .fetch_all(pool)
    .await?;
    Ok(Series {
        joined: rows.iter().map(|r| r.0).collect(),
        departed: rows.iter().map(|r| r.1).collect(),
        changed: rows.iter().map(|r| r.2).collect(),
    })
}

/// Onceki esit uzunluktaki pencerenin uc sayisi: degisim rozetinin paydasi.
/// Kosullar `totals`takilerle birebir ayni, yalnizca pencere bir boy geriye kayar.
async fn previous(pool: &PgPool, time_zone: &str, days: i32) -> Result<Previous, sqlx::Error> {
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL \
              AND start_date > (now() AT TIME ZONE $1)::date - 2 * $2::int \
              AND start_date <= (now() AT TIME ZONE $1)::date - $2::int), \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL \
              AND end_at IS NOT NULL \
              AND end_at <= now() - make_interval(days => $2::int) \
              AND end_at > now() - make_interval(days => 2 * $2::int)), \
           (SELECT count(DISTINCT identity_id) FROM audit_log \
             WHERE event_type = $3 \
               AND occurred_at <= now() - make_interval(days => $2::int) \
               AND occurred_at > now() - make_interval(days => 2 * $2::int))",
    )
    .bind(time_zone)
    .bind(days)
    .bind(audit::IDENTITY_CHANGED)
    .fetch_one(pool)
    .await?;
    Ok(Previous {
        joined: row.0,
        departed: row.1,
        changed: row.2,
    })
}

/// Son etkinlikler: yalnizca operatorun yaptiklari (`actor_username` dolu).
/// Worker'in niyet/sonuc satirlari akisa girmez; onlarin yeri kisi sayfasi.
async fn activity(pool: &PgPool, time_zone: &str) -> Result<Vec<Event>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        i32,
        Option<i64>,
    )> = sqlx::query_as(
        "SELECT a.event_type, a.actor_username, i.given_name || ' ' || i.surname, \
                    to_char(a.occurred_at AT TIME ZONE $1, 'HH24:MI'), \
                    to_char(a.occurred_at AT TIME ZONE $1, 'DD.MM'), \
                    ((now() AT TIME ZONE $1)::date \
                     - (a.occurred_at AT TIME ZONE $1)::date)::int, \
                    a.identity_id \
             FROM audit_log a LEFT JOIN identities i ON i.id = a.identity_id \
             WHERE a.actor_username IS NOT NULL \
             ORDER BY a.id DESC LIMIT $2",
    )
    .bind(time_zone)
    .bind(ACTIVITY_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(event_type, actor, person, time, date, age, identity)| {
            let (icon, category) = glyph_for(&event_type);
            Event {
                icon,
                category,
                href: event_href(&event_type, identity),
                when: match age {
                    0 => "today",
                    1 => "yesterday",
                    _ => "older",
                },
                event_type,
                actor: actor.unwrap_or_default(),
                person: person.unwrap_or_default(),
                time,
                date,
            }
        })
        .collect())
}

/// Akis satirinin gittigi kayit. Kimlige bagli olay kisi sayfasina gider; hedef
/// sistem ayari ve oznitelik eslemesi hedef listesine (eslemenin hangi hedefe ait
/// oldugu yalnizca `detail` JSONB'sinde, kolon degil). Gerisi tiklanmaz: olmayan
/// bir kapiyi isaret etmektense satir sabit kalsin.
fn event_href(event_type: &str, identity: Option<i64>) -> String {
    match (identity, event_type) {
        (Some(id), _) => format!("/identities/{id}"),
        (None, audit::TARGET_CHANGED | audit::MAPPING_CHANGED) => "/targets".to_string(),
        _ => String::new(),
    }
}

/// Eğilim: son yedi gun, gun basina kayit ve ayrilis sayisi. Gunler
/// `generate_series` ile uretilir, boylece olaysiz gun de sutun olarak cizilir.
/// Doner: gunler + y ekseninin tepesi.
async fn trend(pool: &PgPool, time_zone: &str) -> Result<(Vec<TrendDay>, i64), sqlx::Error> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT to_char(d.day, 'DD.MM'), \
                count(a.id) FILTER (WHERE a.event_type = $2), \
                count(a.id) FILTER (WHERE a.event_type IN ($3, $4)) \
         FROM generate_series( \
                ((now() AT TIME ZONE $1)::date - ($5 - 1))::timestamp, \
                ((now() AT TIME ZONE $1)::date)::timestamp, \
                interval '1 day') AS d(day) \
         LEFT JOIN audit_log a ON (a.occurred_at AT TIME ZONE $1)::date = d.day::date \
         GROUP BY d.day ORDER BY d.day",
    )
    .bind(time_zone)
    .bind(audit::IDENTITY_CREATED)
    .bind(audit::IDENTITY_DEPARTURE_SET)
    .bind(audit::IDENTITY_EMERGENCY_DEPARTURE)
    .bind(TREND_DAYS)
    .fetch_all(pool)
    .await?;
    // Iki seri gun basina yan yana iki cubuk (ADR-117 C): olcek gunluk toplamin
    // degil serilerin **kendi** zirvesidir. Toplama gore olcekleyince tek serinin
    // dolu oldugu gun yarim yukseklikte cikiyor ve "yarisi kadar" diye okunuyordu.
    let peak = rows
        .iter()
        .flat_map(|(_, joined, departed)| [*joined, *departed])
        .max()
        .unwrap_or(0);
    let days = rows
        .into_iter()
        .map(|(label, joined, departed)| TrendDay {
            label,
            joined,
            departed,
            joined_pct: percent(joined, peak),
            departed_pct: percent(departed, peak),
        })
        .collect();
    Ok((days, peak))
}

/// Departman kirilimi: en kalabalik bes departman (ADR-076 madde 3).
async fn departments(pool: &PgPool) -> Result<Vec<DistRow>, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT d.name, count(i.id) FROM departments d \
         LEFT JOIN identities i ON i.department_id = d.id AND i.deleted_at IS NULL \
         GROUP BY d.id, d.name HAVING count(i.id) > 0 \
         ORDER BY count(i.id) DESC, d.name LIMIT $1",
    )
    .bind(DISTRIBUTION_LIMIT)
    .fetch_all(pool)
    .await?;
    let peak = rows.iter().map(|(_, count)| *count).max().unwrap_or(0);
    Ok(rows
        .into_iter()
        .map(|(name, count)| DistRow {
            name,
            count,
            pct: percent(count, peak),
        })
        .collect())
}

/// Personel dagilimi: calisma tipine gore. Dilim yuzdeleri burada hesaplanir ve
/// **son dilim kalani yutar** — yuvarlama artigi halkada bosluk birakmasin.
async fn employment(pool: &PgPool) -> Result<Donut, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT employment_type, count(*) FROM identities WHERE deleted_at IS NULL \
         GROUP BY employment_type ORDER BY count(*) DESC, employment_type",
    )
    .fetch_all(pool)
    .await?;
    let total: i64 = rows.iter().map(|(_, count)| *count).sum();
    Ok(Donut {
        slices: slices(rows, total),
        total,
    })
}

/// Rol dagilimi (ADR-076 madde 3): birincil role gore, en kalabalik dort rol
/// ayri dilim, kalani "diger" (bos anahtar; sablon `dash.other` yazar).
/// Rol adi veridir, i18n'e girmez.
async fn roles(pool: &PgPool) -> Result<Donut, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT r.name, count(i.id) FROM roles r \
         JOIN identities i ON i.primary_role_id = r.id AND i.deleted_at IS NULL \
         GROUP BY r.id, r.name ORDER BY count(i.id) DESC, r.name",
    )
    .fetch_all(pool)
    .await?;
    let total: i64 = rows.iter().map(|(_, count)| *count).sum();
    Ok(Donut {
        slices: slices(fold_others(rows, ROLE_SLICES), total),
        total,
    })
}

/// Ilk `keep` satir kalir, gerisi tek "diger" satirinda (bos anahtar) toplanir.
/// Saf — DB olmadan sinanir. Dort ton var (`SLICE_TONES`), bes dilim bes renk eder.
fn fold_others(rows: Vec<(String, i64)>, keep: usize) -> Vec<(String, i64)> {
    if rows.len() <= keep + 1 {
        return rows;
    }
    let mut kept: Vec<(String, i64)> = rows.iter().take(keep).cloned().collect();
    let rest: i64 = rows.iter().skip(keep).map(|(_, n)| *n).sum();
    kept.push((String::new(), rest));
    kept
}

/// Dilimler arasi bosluk, cemberin 100 biriminden (seridin yarisi kadar).
/// Dilimin kuyrugundan kesilir, baslangici degismez: komsu renkler birbirine
/// degmesin diye. Tek dilimde 0 — halka kapali kalir.
const SLICE_GAP: i64 = 2;

/// Halkanin dilimleri: yuzde, `stroke-dasharray` ve `stroke-dashoffset`.
/// Saf — DB olmadan sinanir.
fn slices(rows: Vec<(String, i64)>, total: i64) -> Vec<Slice> {
    let last = rows.len().saturating_sub(1);
    let gap = match last {
        0 => 0,
        _ => SLICE_GAP,
    };
    let mut cumulative = 0_i64;
    rows.into_iter()
        .enumerate()
        .map(|(i, (key, count))| {
            // Son dilim kalani yutar: yuvarlama artigi halkada bosluk birakmasin
            let pct = if i == last {
                100 - cumulative
            } else {
                exact_percent(count, total)
            };
            // 25: cember 100 birim ve 3 yonunden basliyor; dilimleri 12 yonune
            // kaydirmak icin baslangictan ceyrek tur geri alinir
            let offset = 25 - cumulative;
            cumulative += pct;
            // Yuzdesi sifire yuvarlanan dilim hic cizilmez; bosluk onu
            // gorunur yapmaz, efsane satiri gerceği soyler
            let drawn = match pct {
                0 => 0,
                _ => (pct - gap).max(1),
            };
            Slice {
                key,
                count,
                pct,
                dash: format!("{drawn} {}", 100 - drawn),
                offset,
                tone: SLICE_TONES[i % SLICE_TONES.len()],
            }
        })
        .collect()
}

/// Yuvarlanmis yuzde (halka dilimi); `.v-NN` sinifina baglanmadigi icin bese
/// yuvarlanmaz, SVG `stroke-dasharray` tam degeri tasir.
fn exact_percent(value: i64, total: i64) -> i64 {
    if value <= 0 || total <= 0 {
        return 0;
    }
    (value * 100 + total / 2) / total
}

/// Zirveye gore yuzde, en yakin bese yuvarlanmis: sablon `.v-NN` sinifini
/// secebilsin diye (satir ici `style` CSP'de yasak). Sifir olmayan deger hic
/// gorunmeyecek kadar kisa cizilmesin diye en az 5 doner.
pub(crate) fn percent(value: i64, peak: i64) -> i64 {
    if value <= 0 || peak <= 0 {
        return 0;
    }
    // Once yuzdeyi hesaplayip sonra yuvarlamak iki kez kirpardi (7/9 -> %77 -> 75);
    // dogrudan "en yakin beste bir dilim" sayisina yuvarlanir.
    let fifths = (value * 20 + peak / 2) / peak;
    (fifths * 5).clamp(5, 100)
}

/// Kivilcim cizgisinin cizim alani: tepe ve taban, 0-100'luk viewBox icinde.
/// Taban 100 degil 96 — 1,6px cizgi tam kenarda yarisi kirpilirdi.
const SPARK_TOP: f64 = 8.0;
const SPARK_BOTTOM: f64 = 96.0;

/// Kart tanimlari ekrandaki sirayla: anahtar, metin anahtari, ikon, pencere
/// filtresi. Bos filtre = "Toplam" karti, pencereyi okumaz.
const CARD_DEFS: [(&str, &str, &str, &str); 4] = [
    ("total", "dash.total_identities", "ico-users", ""),
    ("joined", "dash.joined", "ico-user-plus", "joined"),
    ("left", "dash.departed", "ico-logout", "departed"),
    ("moved", "dash.changed", "ico-refresh", "changed"),
];

/// Dort sayac karti. Saf — DB olmadan sinanir.
///
/// "Toplam" kartinin kendi sorgusu yok: pencerenin basindaki kadro
/// `identities - joined`tir (silinmemis kayitlarin ne kadari pencerede girdi),
/// seri de gunluk kayitlarin kumulatifi. Ayrilanlar silinmedigi icin toplamdan
/// dusmez: kart "kac kisi kayitli"yi sayar, "kac kisi calisiyor"u degil.
fn cards(totals: &Totals, days: i32, series: &Series, previous: Previous) -> Vec<StatCard> {
    let base = totals.identities - totals.joined;
    let total_series = cumulative(base, &series.joined);
    // Kart basina: sayi, onceki donem, gunluk seri, artis iyi mi
    let data: [(i64, i64, &[i64], bool); 4] = [
        (totals.identities, base, &total_series, true),
        (totals.joined, previous.joined, &series.joined, true),
        // Ayrilis artarsa kotu: rozetin tonu oburlerinin tersi
        (totals.departed, previous.departed, &series.departed, false),
        (totals.changed, previous.changed, &series.changed, true),
    ];
    CARD_DEFS
        .iter()
        .zip(data)
        .map(
            |((key, label, icon, filter), (value, before, daily, up_is_good))| StatCard {
                key,
                label,
                icon,
                href: match filter.is_empty() {
                    true => "/identities".to_string(),
                    false => format!("/identities?window={filter}&days={days}"),
                },
                value,
                foot: if filter.is_empty() {
                    "dash.all_time"
                } else {
                    ""
                },
                delta: delta(value, before, up_is_good),
                spark: spark(daily),
            },
        )
        .collect()
}

/// Kumulatif kadro: pencerenin basindaki sayidan baslar, her gun o gunun
/// kayitlarini ekler. Saf — DB olmadan sinanir.
fn cumulative(base: i64, daily: &[i64]) -> Vec<i64> {
    let mut running = base;
    daily
        .iter()
        .map(|n| {
            running += n;
            running
        })
        .collect()
}

/// Degisim rozeti. `up_is_good`: artis iyi mi (ayrilis kartinda degil).
/// Onceki donem sifirken yuzde tanimsizdir — rozet "yeni" der, bolme yapilmaz.
/// Saf — DB olmadan sinanir.
fn delta(now: i64, before: i64, up_is_good: bool) -> Delta {
    let tone = |up: bool| if up == up_is_good { "ok" } else { "err" };
    if now == before {
        return Delta {
            pct: 0,
            kind: "flat",
            tone: "muted",
            before,
        };
    }
    if before == 0 {
        return Delta {
            pct: 0,
            kind: "new",
            tone: tone(true),
            before,
        };
    }
    let up = now > before;
    Delta {
        pct: ((now - before).abs() * 100 + before / 2) / before,
        kind: if up { "up" } else { "down" },
        tone: tone(up),
        before,
    }
}

/// Kivilcim cizgisi: `<polyline>` ve altindaki dolgunun `<polygon>` noktalari.
/// Olcek dizinin kendi en kucuk-en buyuk araligi — kumulatif kadro gibi sifirdan
/// uzak serilerde 0 tabanli olcek cizgiyi duz yapardi. Hepsi ayni degerse cizgi
/// duz: sifirsa tabanda, degilse ortada. Saf — DB olmadan sinanir.
fn spark(values: &[i64]) -> Spark {
    use std::fmt::Write;
    let (lo, hi) = match (values.iter().min(), values.iter().max()) {
        (Some(lo), Some(hi)) => (*lo, *hi),
        _ => {
            return Spark {
                line: String::new(),
                area: String::new(),
            }
        }
    };
    let step = match values.len() {
        0 | 1 => 0.0,
        n => 100.0 / (n - 1) as f64,
    };
    let mut line = String::new();
    for (i, value) in values.iter().enumerate() {
        let y = match (hi > lo, hi > 0) {
            (true, _) => {
                SPARK_BOTTOM - (value - lo) as f64 / (hi - lo) as f64 * (SPARK_BOTTOM - SPARK_TOP)
            }
            (false, true) => (SPARK_TOP + SPARK_BOTTOM) / 2.0,
            (false, false) => SPARK_BOTTOM,
        };
        if i > 0 {
            line.push(' ');
        }
        let _ = write!(line, "{:.1},{y:.1}", i as f64 * step);
    }
    Spark {
        area: format!("{line} 100.0,100.0 0.0,100.0"),
        line,
    }
}

/// Olay turunun ikonu ve kategorisi (ADR-117 B). Bes kategori var ve kategori
/// hem rengi (`.feed-ico--<kategori>`) hem anlami tasir:
///
///   `auth`    operator girisi
///   `config`  hedef sistem ayari, oznitelik eslemesi, tanim ve ayar degisikligi
///   `account` hesap ve kimlik isleri, rol atama, parola
///   `danger`  silme, ayrilis, basarisiz giris, reddedilen is
///   `other`   gerisi — bilinmeyen tur de buraya duser, ekran bozulmaz
///
/// Ikon olay turune ozel kalir: kategori besli, ikon daha ince ayrim yapar.
/// Kisi sayfasinin olay akisi da ayni tablodan okur (`identity::load_events`),
/// boylece iki ekran birbirinden sapamaz.
pub(crate) fn glyph_for(event_type: &str) -> (&'static str, &'static str) {
    match event_type {
        // --- auth ---
        audit::OPERATOR_LOGIN => ("ico-sign-in", "auth"),
        audit::OPERATOR_REJECTED => ("ico-sign-in", "danger"),
        // --- config ---
        audit::TARGET_CHANGED => ("ico-cog", "config"),
        audit::MAPPING_CHANGED => ("ico-link", "config"),
        audit::SETTINGS_CHANGED | audit::BOOTSTRAP_PASSWORD_CHANGED => ("ico-cog", "config"),
        audit::ROLE_CHANGED => ("ico-key", "config"),
        audit::DEPARTMENT_CHANGED => ("ico-sitemap", "config"),
        // --- account ---
        audit::IDENTITY_CREATED => ("ico-user-plus", "account"),
        audit::IDENTITY_CHANGED => ("ico-refresh", "account"),
        audit::IDENTITY_ROLE_ASSIGNED | audit::IDENTITY_ROLE_REMOVED => ("ico-key", "account"),
        audit::IDENTITY_SUSPENDED | audit::IDENTITY_SUSPENSION_LIFTED => ("ico-clock", "account"),
        audit::FIRST_PASSWORD_REQUESTED | audit::FIRST_PASSWORD_SHOWN => ("ico-lock", "account"),
        audit::USED_NAME_RELEASED | audit::IDENTITY_NAME_REQUESTED => ("ico-tag", "account"),
        audit::JOB_RETRY_REQUESTED | audit::RECONCILE_REAPPLY => ("ico-refresh", "account"),
        audit::ACCOUNT_MANAGE_REQUESTED => ("ico-shield", "account"),
        // ADR-112: AD'den gelen alan degeri (dolum worker'da, alim operatorde)
        audit::IDENTITY_FIELD_TAKEN | "identity.fields_filled" => ("ico-link", "account"),
        audit::IDENTITY_IMPORTED | audit::IMPORT_APPLIED => ("ico-inbox", "account"),
        "identity.role_expired" => ("ico-key", "account"),
        // --- onay akisi: sahneleme, onay, red ---
        audit::IMPORT_STAGED | audit::MANAGE_STAGED => ("ico-inbox", "config"),
        audit::IMPORT_APPROVED | audit::MANAGE_APPROVED => ("ico-check", "config"),
        audit::IMPORT_REJECTED | audit::MANAGE_REJECTED => ("ico-logout", "config"),
        // --- danger ---
        audit::IDENTITY_DEPARTURE_SET
        | audit::IDENTITY_EMERGENCY_DEPARTURE
        | audit::IDENTITY_DEPARTURE_REVERTED
        | audit::IDENTITY_CANCELLED => ("ico-logout", "danger"),
        audit::ACCOUNT_DELETION_APPROVED => ("ico-trash", "danger"),
        // ADR-122: olu baglantinin kaldirilmasi istendi / worker kaldirdi
        audit::ACCOUNT_UNLINK_REQUESTED => ("ico-link", "account"),
        "ad.account.unlinked" => ("ico-link", "account"),
        // --- worker'in hedef islemleri: yalnizca kisi sayfasinda gorunur ---
        "ad.account.create" => ("ico-user-plus", "account"),
        "ad.account.adopted" => ("ico-link", "account"),
        "ad.account.managed" => ("ico-shield", "account"),
        "ad.account.enable" => ("ico-check", "account"),
        "ad.account.move" => ("ico-sitemap", "account"),
        "ad.account.attributes" => ("ico-refresh", "account"),
        "ad.group.add_member" | "ad.group.remove_member" => ("ico-key", "account"),
        "ad.account.first_password" | "ad.account.password_reset" => ("ico-lock", "account"),
        "ad.cancellation.verified" => ("ico-check", "account"),
        "ad.account.disable" => ("ico-logout", "danger"),
        "ad.account.delete" => ("ico-trash", "danger"),
        "ad.cancellation.rejected" => ("ico-bolt", "danger"),
        _ => ("ico-inbox", "other"),
    }
}
// --- END FEATURE: dashboard ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Etkinlik akisinin kategorileri (ADR-117 B). `glyph_for` bunlardan birini
    /// dondurur; her birinin app.css'te kendi rengi olmali.
    const FEED_CATEGORIES: [&str; 5] = ["auth", "config", "account", "danger", "other"];

    #[test]
    fn percent_rounds_to_five_and_keeps_small_values_visible() {
        assert_eq!(percent(0, 10), 0);
        assert_eq!(percent(10, 10), 100);
        assert_eq!(percent(5, 10), 50);
        // 1/10 = %10
        assert_eq!(percent(1, 10), 10);
        // 1/100 = %1 -> cizilebilsin diye tabana (5) yukselir
        assert_eq!(percent(1, 100), 5);
        // 7/9 = %77,7 -> en yakin bes
        assert_eq!(percent(7, 9), 80);
    }

    /// ADR-117: rozetin yonu ve tonu. Artis her kartta iyi degil.
    #[test]
    fn the_delta_badge_knows_when_a_rise_is_bad() {
        let up = delta(12, 10, true);
        assert_eq!((up.pct, up.kind, up.tone), (20, "up", "ok"));
        // Ayni artis ayrilis kartinda kotu
        let bad = delta(12, 10, false);
        assert_eq!((bad.pct, bad.kind, bad.tone), (20, "up", "err"));
        let down = delta(8, 10, true);
        assert_eq!((down.pct, down.kind, down.tone), (20, "down", "err"));
        // Azalan ayrilis iyi haber
        assert_eq!(delta(8, 10, false).tone, "ok");
    }

    /// Onceki donem sifirken yuzde tanimsiz: bolme yapilmaz, rozet "yeni" der.
    #[test]
    fn the_delta_badge_never_divides_by_zero() {
        let new = delta(5, 0, true);
        assert_eq!((new.kind, new.pct, new.tone), ("new", 0, "ok"));
        // Hic hareket yoksa rozet sessiz
        for before in [0, 7] {
            let flat = delta(before, before, true);
            assert_eq!((flat.kind, flat.tone), ("flat", "muted"), "{before}");
        }
        // Sifira dusus: yuzde yine hesaplanir
        assert_eq!(delta(0, 4, true).pct, 100);
    }

    #[test]
    fn the_cumulative_headcount_starts_at_the_window_floor() {
        assert_eq!(cumulative(10, &[1, 0, 2]), vec![11, 11, 13]);
        assert_eq!(cumulative(0, &[]), Vec::<i64>::new());
    }

    /// Kivilcim: nokta sayisi gun sayisi kadar, x 0'dan 100'e esit araliklarla,
    /// y hep cizim alaninin icinde. Dolgu cizgiyi tabanda kapatir.
    #[test]
    fn the_sparkline_fills_the_box_and_stays_inside_it() {
        let s = spark(&[0, 5, 10]);
        assert_eq!(s.line, "0.0,96.0 50.0,52.0 100.0,8.0");
        assert!(s.area.ends_with(" 100.0,100.0 0.0,100.0"), "{}", s.area);
        for point in s.line.split(' ') {
            let (x, y) = point.split_once(',').expect("x,y");
            let (x, y): (f64, f64) = (x.parse().unwrap(), y.parse().unwrap());
            assert!((0.0..=100.0).contains(&x), "{point}");
            assert!((SPARK_TOP..=SPARK_BOTTOM).contains(&y), "{point}");
        }
    }

    /// Hepsi ayni degerse cizgi duz: sifirsa tabanda (hicbir sey olmadi),
    /// degilse ortada (degismedi) — tabandaki duz cizgi "sifir" diye okunurdu.
    #[test]
    fn a_flat_series_draws_a_flat_line() {
        assert_eq!(spark(&[0, 0, 0]).line, "0.0,96.0 50.0,96.0 100.0,96.0");
        assert_eq!(spark(&[7, 7, 7]).line, "0.0,52.0 50.0,52.0 100.0,52.0");
        assert_eq!(spark(&[]).line, "");
        assert_eq!(spark(&[3]).line, "0.0,52.0");
    }

    /// Kart listesi: dort kart, dort ayri anahtar (gradyan id'si benzersiz olmali),
    /// "Toplam" disindakiler pencereli listeye gider.
    #[test]
    fn the_four_cards_have_distinct_keys_and_real_links() {
        let totals = Totals {
            identities: 12,
            joined: 2,
            departed: 1,
            changed: 3,
            needs_intervention: 0,
            intervention_identity: None,
            pending_approvals: 0,
            role_unassigned: 0,
        };
        let series = Series {
            joined: vec![1, 1],
            departed: vec![0, 1],
            changed: vec![2, 1],
        };
        let previous = Previous {
            joined: 1,
            departed: 2,
            changed: 3,
        };
        let cards = cards(&totals, 30, &series, previous);
        assert_eq!(cards.len(), 4);
        let keys: Vec<&str> = cards.iter().map(|c| c.key).collect();
        assert_eq!(keys, vec!["total", "joined", "left", "moved"]);
        assert_eq!(cards[0].href, "/identities");
        assert_eq!(cards[0].value, 12);
        // Toplamin tabani: pencerede girenler dusulur
        assert_eq!(cards[0].delta.before, 10);
        assert_eq!(cards[1].href, "/identities?window=joined&days=30");
        assert_eq!(cards[2].href, "/identities?window=departed&days=30");
        assert_eq!(cards[3].href, "/identities?window=changed&days=30");
        // Degismeyen sayac sessiz, azalan ayrilis iyi
        assert_eq!(cards[3].delta.kind, "flat");
        assert_eq!(cards[2].delta.tone, "ok");
        // Her kart filtresi listede izinli
        for card in &cards[1..] {
            let filter = card.href.split("window=").nth(1).unwrap();
            let filter = filter.split('&').next().unwrap();
            assert!(
                crate::identity::WINDOW_FILTERS.contains(&filter),
                "{filter} izinli listede yok"
            );
        }
    }

    #[test]
    fn percent_is_safe_without_data() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(-1, 10), 0);
    }

    #[test]
    fn every_percent_matches_a_defined_css_class() {
        // Sablon `v-{{ pct }}` yaziyor; app.css yalnizca beser adimli siniflari
        // tanimliyor. Araya baska bir deger sizarsa cubuk hic cizilmez.
        for peak in 1..40i64 {
            for value in 0..=peak {
                let pct = percent(value, peak);
                assert_eq!(pct % 5, 0, "{value}/{peak} -> {pct}");
                assert!((0..=100).contains(&pct), "{value}/{peak} -> {pct}");
            }
        }
    }

    /// ADR-076 tarih araligi: yalnizca listedeki pencereler; keyfi sayi varsayilana duser.
    #[test]
    fn the_window_accepts_only_listed_values() {
        assert_eq!(window(None), WINDOW_DAYS);
        assert_eq!(window(Some(7)), 7);
        assert_eq!(window(Some(365)), 365);
        assert_eq!(window(Some(5)), WINDOW_DAYS);
        assert_eq!(window(Some(-30)), WINDOW_DAYS);
    }

    /// Rol halkasi: ilk dort rol ayri, gerisi "diger"; bes ve altinda katlama yok.
    #[test]
    fn roles_beyond_the_fourth_fold_into_other() {
        let rows = |n: usize| {
            (1..=n)
                .map(|i| (format!("Rol {i}"), (10 - i) as i64))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            fold_others(rows(5), 4).len(),
            5,
            "beş rol beş dilim, 'diğer' gerekmez"
        );
        let folded = fold_others(rows(7), 4);
        assert_eq!(folded.len(), 5);
        assert_eq!(
            folded[4],
            (String::new(), 5 + 4 + 3),
            "5., 6., 7. rol toplandı"
        );
        assert_eq!(folded[0].0, "Rol 1");
    }

    /// Yan yana iki cubuk (ADR-117 C): olcek serilerin kendi zirvesi, bu yuzden
    /// hicbir cubuk cizim alanini asamaz — yigilmada iki %100 ust uste geliyordu.
    #[test]
    fn a_grouped_column_never_overflows_the_plot() {
        for peak in 1..60i64 {
            for value in 0..=peak {
                assert!(percent(value, peak) <= 100, "{value}/{peak}");
            }
        }
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_panel_counts_reads_and_charts_from_the_existing_tables() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        // Ali Kaya 40 gun once ise girdi (30 gunluk pencerenin disinda), bugun ayrildi.
        sqlx::query(
            "UPDATE identities SET start_date = current_date - 40, end_at = now() - interval '1 hour' \
             WHERE id = $1",
        )
        .bind(ids[1])
        .execute(&pool)
        .await
        .unwrap();
        // Iki operator olayi: biri kayit (bugunun sutunu), biri gorev degisikligi.
        for (event, identity) in [
            (audit::IDENTITY_CREATED, ids[0]),
            (audit::IDENTITY_CHANGED, ids[0]),
        ] {
            sqlx::query(
                "INSERT INTO audit_log (event_type, actor_username, identity_id) \
                 VALUES ($1, 'insan.kaynaklari', $2)",
            )
            .bind(event)
            .bind(identity)
            .execute(&pool)
            .await
            .unwrap();
        }
        // Bir is mudahalede bekliyor
        let target: i64 = sqlx::query_scalar("SELECT id FROM target_systems WHERE kind = 'ad'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO jobs (identity_id, target_system_id, priority, status) \
             VALUES ($1, $2, 2, 'needs_intervention')",
        )
        .bind(ids[0])
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();

        let dash = load(&pool, "Europe/Istanbul", WINDOW_DAYS).await.unwrap();

        assert_eq!(dash.totals.identities, 2, "silinmemis kimlik");
        assert_eq!(
            dash.totals.joined, 1,
            "40 gun onceki giris pencereye girmez"
        );
        // ADR-076 tarih araligi: pencere buyuyunce 40 gun onceki giris de sayilir
        let wide = load(&pool, "Europe/Istanbul", 365).await.unwrap();
        assert_eq!(wide.totals.joined, 2);
        assert_eq!(wide.window_days, 365);
        assert!(wide.windows.iter().any(|w| w.days == 365 && w.active));
        assert_eq!(wide.windows.iter().filter(|w| w.active).count(), 1);
        // Rol halkasi: iki kimlik ayni birincil rolde → tek dilim %100
        assert_eq!(dash.roles.total, 2);
        assert_eq!(dash.roles.slices.len(), 1);
        assert_eq!(dash.roles.slices[0].key, "Test Rolü");
        assert_eq!(dash.roles.slices[0].pct, 100);
        assert_eq!(dash.totals.departed, 1);
        assert_eq!(dash.totals.changed, 1);
        assert_eq!(dash.totals.needs_intervention, 1);
        assert_eq!(dash.totals.pending_approvals, 0);
        assert_eq!(dash.today.len(), 10, "GG.AA.YYYY: {}", dash.today);

        // Akis yalnizca operator satirlarini alir, en yenisi basta
        assert_eq!(dash.activity.len(), 2);
        assert_eq!(dash.activity[0].event_type, audit::IDENTITY_CHANGED);
        assert_eq!(dash.activity[0].actor, "insan.kaynaklari");
        assert_eq!(dash.activity[0].person, "Ayşe Yılmaz");

        // Olaysiz gunler de sutun olur; bugunun kaydi zirveyi tutar
        assert_eq!(dash.trend.len(), TREND_DAYS as usize);
        let today = dash.trend.last().unwrap();
        assert_eq!(today.joined, 1);
        assert_eq!(today.joined_pct, 100);
        assert!(dash.trend.iter().all(|d| d.joined_pct % 5 == 0));

        // Iki kimlik de ayni departmanda: tek satir, dolu olcek
        assert_eq!(dash.departments.len(), 1);
        assert_eq!(dash.departments[0].name, "Test Birimi");
        assert_eq!(dash.departments[0].count, 2);
        assert_eq!(dash.departments[0].pct, 100);

        // Mutabakat hic taranmadiysa yonlendirme seridi de yok (ADR-103)
        assert!(dash.unadopted.is_empty());

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[test]
    fn the_setup_card_only_disappears_when_all_four_steps_are_done() {
        let fresh = setup_steps([false; SETUP_STEPS], Some(7));
        assert!(!fresh.done);
        assert_eq!(fresh.steps.len(), SETUP_STEPS);
        // Ilk adim Yapilandirma sayfasinda: yalnizca `admin` acabiliyor
        assert!(fresh.steps[0].admin_only);
        assert_eq!(fresh.steps[0].links[0].href, "/config");
        // Departman + rol adimi iki ekrana baglanir
        assert_eq!(fresh.steps[2].links.len(), 2);
        // Son adim toplu sahiplenmeye, hedefin mutabakat ekranina
        assert_eq!(fresh.steps[3].links[0].href, "/targets/7/reconcile");
        assert_eq!(fresh.steps[3].note, "setup.staff_csv");

        // Uc adim tamam, biri eksik: kart durur
        let partial = setup_steps([true, true, true, false], None);
        assert!(!partial.done);
        // AD hedefi yoksa son adim rapor kapagina duser (bos href basilmaz)
        assert_eq!(partial.steps[3].links[0].href, "/reports");

        assert!(setup_steps([true; SETUP_STEPS], Some(7)).done);
    }

    #[test]
    fn every_setup_key_and_icon_exists() {
        // Anahtarlar struct alanindan geliyor; i18n taramasi sablondaki duz
        // metin cagrisini aradigi icin bunlari gormez, burada dogrulanir.
        let css = include_str!("../../frontend/assets/app.css");
        let setup = setup_steps([false; SETUP_STEPS], Some(1));
        for step in &setup.steps {
            assert!(
                css.contains(&format!(".{}::before {{", step.icon)),
                "app.css'te .{} yok",
                step.icon
            );
            let mut keys = vec![step.key, step.hint];
            if !step.note.is_empty() {
                keys.push(step.note);
            }
            keys.extend(step.links.iter().map(|l| l.key));
            for key in keys {
                for lang in [crate::i18n::Lang::Tr, crate::i18n::Lang::En] {
                    assert_ne!(lang.t(key), key, "{}: {key} eksik", lang.code());
                }
            }
        }
        assert!(
            css.contains(".qa-row-done {"),
            "app.css'te .qa-row-done yok"
        );
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_setup_steps_are_derived_from_the_existing_tables() {
        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;

        // Bos kurulum: dort adimin dordu de eksik
        let fresh = setup(&pool).await.unwrap();
        assert!(!fresh.done);
        assert!(fresh.steps.iter().all(|s| !s.done));

        // Departman + rol + personel tek seed'le gelir
        crate::test_support::seed_two_identities(&pool).await;
        // Baglanti ayari ve katalog taramasi
        sqlx::query("UPDATE app_settings SET ad_host = 'ldaps://dc1.example.org'")
            .execute(&pool)
            .await
            .unwrap();
        let catalog = crate::test_support::seed_example_catalog(&pool).await;
        let done = setup(&pool).await.unwrap();
        assert!(done.done, "dört adım da tamam olmalı");

        // Katalog ogesi silinmez, "kayip" isaretlenir (ADR-014): kayip oge
        // tarama adimini tamam gostermemeli
        sqlx::query("UPDATE catalog_items SET missing_since = now()")
            .execute(&pool)
            .await
            .unwrap();
        let lost = setup(&pool).await.unwrap();
        assert!(!lost.done);
        assert!(!lost.steps[1].done, "kayıp katalog adımı tamam saymaz");
        assert!(catalog.gg_vpn > 0, "seed katalog öğesi döndürür");

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// ADR-076 tarih araligi: `?days=` sayaclari ve "Son N gun" etiketini degistirir,
    /// listede olmayan deger varsayilana duser, segmentli kontrolde secili oge isaretli.
    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_home_window_filter_changes_the_counts_and_marks_the_segment() {
        use axum::body::Body;
        use axum::http::{header, Request};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let ids = crate::test_support::seed_two_identities(&pool).await;
        sqlx::query("UPDATE identities SET start_date = current_date - 40 WHERE id = $1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let operator = crate::operator_session::Operator {
            subject: "sub-x".to_string(),
            username: "ik.operatoru".to_string(),
            email: "ik@example.org".to_string(),
            authorities: vec!["hr".to_string()],
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(&pool, &operator)
            .await
            .unwrap();
        let cookie = format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME);
        let page = |path: &'static str| {
            let (app, cookie) = (app.clone(), cookie.clone());
            async move {
                let r = app
                    .oneshot(
                        Request::builder()
                            .uri(path)
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };
        let lang = crate::i18n::DEFAULT;
        let label = |days: i32| lang.t1("dash.window_n", days);

        // Varsayilan 30: 40 gun onceki giris sayilmaz; segmentte 30 secili
        let home = page("/").await;
        assert!(
            home.contains(&label(30)) && !home.contains(&label(5)),
            "{home}"
        );
        assert!(
            home.contains(r#"segment-item segment-item-active" href="/?days=30""#),
            "{home}"
        );
        // 365: giris sayilir; segmentte 365 secili, 30 degil
        let wide = page("/?days=365").await;
        assert!(
            wide.contains(r#"segment-item segment-item-active" href="/?days=365""#),
            "{wide}"
        );
        assert!(!wide.contains(r#"segment-item segment-item-active" href="/?days=30""#));
        // Listede olmayan deger varsayilana duser
        let odd = page("/?days=5").await;
        assert!(
            odd.contains(r#"segment-item segment-item-active" href="/?days=30""#),
            "{odd}"
        );
        // Rol halkasi ekranda: seed rolu ve toplam
        assert!(home.contains(lang.t("dash.roles")) && home.contains("Test Rolü"));

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[tokio::test]
    #[ignore = "gerçek Postgres gerektirir: DATABASE_URL ile çalıştır (--include-ignored)"]
    async fn the_home_page_prints_the_setup_card_until_the_install_is_complete() {
        use axum::body::Body;
        use axum::http::{header, Request};
        use tower::ServiceExt;

        let (admin_pool, pool, db_name) = crate::test_support::fresh_migrated_db().await;
        let app = crate::web::routes()
            .with_state(crate::web::test_state(pool.clone(), "https://localhost"));
        let operator = crate::operator_session::Operator {
            subject: "sub-setup".to_string(),
            username: "ik.operatoru".to_string(),
            email: "ik@example.org".to_string(),
            authorities: vec!["hr".to_string()],
            auth_source: crate::operator_session::AuthSource::Oidc,
            lang: crate::i18n::DEFAULT,
        };
        let token = crate::operator_session::create_session(&pool, &operator)
            .await
            .unwrap();
        let cookie = format!("{}={token}", crate::cookie::OPERATOR_SESSION_COOKIE_NAME);
        let home = || {
            let (app, cookie) = (app.clone(), cookie.clone());
            async move {
                let r = app
                    .oneshot(
                        Request::builder()
                            .uri("/")
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                    .await
                    .unwrap();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };

        let title = crate::i18n::DEFAULT.t("setup.title");
        let body = home().await;
        assert!(body.contains(title), "boş kurulumda kart basılmalı");
        // `hr` operatoru Yapilandirma sayfasini acamaz: baglanti gosterilmez
        assert!(
            !body.contains("href=\"/config\""),
            "yetkisiz operatöre bağlantı verilmemeli"
        );
        assert!(
            body.contains("/departments"),
            "eksik adımın ekranı bağlanmalı"
        );

        // Dort adim tamamlanir: kart hic basilmaz
        sqlx::query("UPDATE app_settings SET ad_host = 'ldaps://dc1.example.org'")
            .execute(&pool)
            .await
            .unwrap();
        crate::test_support::seed_example_catalog(&pool).await;
        crate::test_support::seed_two_identities(&pool).await;
        assert!(!home().await.contains(title), "kurulum bitince kart kalmaz");

        drop(app);
        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    /// ADR-117 B: her olay bir kategoriye duser, kategori rengi secer.
    #[test]
    fn known_events_get_their_own_icon_and_unknown_ones_fall_back() {
        assert_eq!(
            glyph_for(audit::IDENTITY_CREATED),
            ("ico-user-plus", "account")
        );
        assert_eq!(glyph_for(audit::OPERATOR_LOGIN), ("ico-sign-in", "auth"));
        // Basarisiz giris "auth" degil "danger": operatorun dikkatini ister
        assert_eq!(
            glyph_for(audit::OPERATOR_REJECTED),
            ("ico-sign-in", "danger")
        );
        assert_eq!(glyph_for(audit::TARGET_CHANGED), ("ico-cog", "config"));
        // Esleme ayri ikon alir: ayni kategoride ama farkli is
        assert_eq!(glyph_for(audit::MAPPING_CHANGED), ("ico-link", "config"));
        assert_eq!(
            glyph_for(audit::IDENTITY_EMERGENCY_DEPARTURE),
            ("ico-logout", "danger")
        );
        assert_eq!(glyph_for("bilinmeyen.olay"), ("ico-inbox", "other"));
    }

    /// Tabloda karsiligi olmayan olay sessizce gri "other" kutusuna duser ve
    /// akista ayirt edilemez. Liste elle tutulmaz: `audit.rs`in kendi kaynagi
    /// taranir, worker'in kendi olaylari `WORKER_EVENTS`te.
    #[test]
    fn no_known_event_falls_into_the_generic_bucket() {
        let source = include_str!("audit.rs");
        let declared = source
            .lines()
            .filter(|line| line.starts_with("pub const "))
            .filter_map(|line| line.split('"').nth(1));
        for event in declared.chain(audit::WORKER_EVENTS) {
            let (icon, category) = glyph_for(event);
            assert_ne!(category, "other", "{event} için kategori yok");
            assert!(
                FEED_CATEGORIES.contains(&category),
                "{event}: bilinmeyen kategori {category}"
            );
            assert!(icon.starts_with("ico-"), "{event}: ikon adı {icon}");
        }
    }

    /// Akisin tek renk olmamasi tabloya bagli: kategoriler gercekten dagilmali.
    #[test]
    fn the_feed_table_spreads_events_across_every_category() {
        let seen: std::collections::BTreeSet<&str> = [
            audit::OPERATOR_LOGIN,
            audit::TARGET_CHANGED,
            audit::IDENTITY_CREATED,
            audit::IDENTITY_CANCELLED,
            "bilinmeyen.olay",
        ]
        .iter()
        .map(|e| glyph_for(e).1)
        .collect();
        assert_eq!(seen.len(), FEED_CATEGORIES.len(), "{seen:?}");
    }

    /// Satir ancak gidilecek bir yer varsa tiklanir.
    #[test]
    fn a_feed_row_links_only_where_there_is_something_to_open() {
        assert_eq!(
            event_href(audit::IDENTITY_CREATED, Some(7)),
            "/identities/7"
        );
        // Kimlige bagli olmayan ayar olaylari hedef listesine gider
        assert_eq!(event_href(audit::TARGET_CHANGED, None), "/targets");
        assert_eq!(event_href(audit::MAPPING_CHANGED, None), "/targets");
        // Gidilecek yeri olmayan satir sabit kalir
        assert_eq!(event_href(audit::OPERATOR_LOGIN, None), "");
    }

    #[test]
    fn donut_slices_close_the_ring_and_start_where_the_previous_one_ended() {
        let rows = vec![
            ("permanent".to_string(), 312_i64),
            ("contract".to_string(), 128),
            ("intern".to_string(), 68),
            ("outsourced".to_string(), 39),
        ];
        let total: i64 = rows.iter().map(|(_, c)| *c).sum();
        let slices = slices(rows, total);

        // Yuzdeler tam 100 eder: son dilim yuvarlama artigini yutar
        assert_eq!(slices.iter().map(|s| s.pct).sum::<i64>(), 100);
        assert_eq!(slices[0].pct, 57);
        // Her dilim bir oncekinin bittigi yerden baslar (offset = 25 - onceki toplam)
        assert_eq!(slices[0].offset, 25);
        assert_eq!(slices[1].offset, 25 - 57);
        assert_eq!(slices[2].offset, 25 - 57 - 23);
        // dasharray cizilen dilimi + kalani; cember 100 birim, kuyruktan
        // `SLICE_GAP` kadar kesilir (komsu dilimler birbirine degmesin)
        assert_eq!(slices[0].dash, "55 45");
        // Bosluk yalnizca cizimden gider: siradaki dilim yine 57'de baslar
        assert_eq!(slices[1].offset, 25 - 57);
        // Renkler paletin grafik sirasinda
        assert_eq!(slices[0].tone, "accent");
        assert_eq!(slices[3].tone, "warn");
    }

    #[test]
    fn a_single_slice_fills_the_ring_and_no_data_draws_nothing() {
        let one = slices(vec![("permanent".to_string(), 7)], 7);
        assert_eq!(one[0].pct, 100);
        // Tek dilimde bosluk yok: kesilse halkada sebepsiz bir cizik olurdu
        assert_eq!(one[0].dash, "100 0");
        // Yuzdesi sifire yuvarlanan dilim cizilmez, bosluk onu 1 birime cikarmaz
        let tiny = slices(vec![("a".to_string(), 9999), ("b".to_string(), 1)], 10_000);
        assert_eq!(tiny[1].pct, 0);
        assert_eq!(tiny[1].dash, "0 100");
        assert!(slices(Vec::new(), 0).is_empty());
    }

    #[test]
    fn every_slice_tone_has_a_css_class() {
        // Sablon `seg-{{ tone }}` ve `dot-{{ tone }}` yaziyor; ikisi de app.css'te olmali
        let css = include_str!("../../frontend/assets/app.css");
        for tone in SLICE_TONES {
            assert!(
                css.contains(&format!(".seg-{tone} {{")),
                "eksik .seg-{tone}"
            );
            assert!(
                css.contains(&format!(".dot-{tone} {{")),
                "eksik .dot-{tone}"
            );
        }
    }

    #[test]
    fn every_tone_has_a_css_class() {
        // Sablon `ico-tile-{{ tone }}` yaziyor; app.css yalnizca bu bes tonu
        // taniyor. Araya baska bir ad sizarsa ikon karesi renksiz kalir.
        let css = include_str!("../../frontend/assets/app.css");
        for tone in ["accent", "ok", "warn", "err", "info"] {
            assert!(
                css.contains(&format!(".ico-tile-{tone} {{")),
                "app.css'te .ico-tile-{tone} yok"
            );
        }
        // ADR-117 B: akis satirinin rengi kategoriden gelir; kategori basina
        // hem ikon dairesi hem satir sinifi tanimli olmali, yoksa satir renksiz.
        for category in FEED_CATEGORIES {
            for class in [
                format!(".feed-ico--{category} {{"),
                format!(".feed-row--{category} {{"),
            ] {
                assert!(css.contains(&class), "app.css'te {class} yok");
            }
        }
    }
}
