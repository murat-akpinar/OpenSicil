// --- START FEATURE: dashboard ---
// Gosterge paneli (ADR-076 v1 kapsami, ADR-096 madde 3 ile ana sayfaya alindi).
// Butun sayilar Faz 2-3'te kurulan tablolardan okunur: yeni migration, yeni
// bagimlilik ve grafik kutuphanesi yok. Grafikler native CSS (ADR-076): yuzdeler
// burada en yakin bese yuvarlanir, sablon `.v-NN` sinifini secer -- nginx CSP'si
// satir ici `style`e izin vermiyor (ADR-088 madde 4).

use sqlx::PgPool;

use crate::audit;

/// Panelin pencereleri; "son 30 gun" ADR-076'nin varsayilani.
const WINDOW_DAYS: i32 = 30;
const TREND_DAYS: i32 = 7;
const ACTIVITY_LIMIT: i64 = 8;
const DISTRIBUTION_LIMIT: i64 = 5;

pub struct Dashboard {
    /// Hos geldiniz kartindaki tarih, kurulum saat diliminde (ADR-039)
    pub today: String,
    pub totals: Totals,
    pub activity: Vec<Event>,
    pub trend: Vec<TrendDay>,
    pub departments: Vec<DistRow>,
}

pub struct Totals {
    pub identities: i64,
    pub joined: i64,
    pub departed: i64,
    pub changed: i64,
    /// Hedefe uygulanamamis is: ADR-076'nin "ayrilmis ama kapatilamamis" kutusu
    pub needs_intervention: i64,
    /// Onay bekleyen rol/departman taslagi (ADR-031)
    pub pending_approvals: i64,
}

pub struct Event {
    /// i18n anahtari: `lang.key("event", …)`
    pub event_type: String,
    pub actor: String,
    pub person: String,
    pub time: String,
    pub icon: &'static str,
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

pub async fn load(pool: &PgPool, time_zone: &str) -> Result<Dashboard, sqlx::Error> {
    let (totals, today) = totals(pool, time_zone).await?;
    Ok(Dashboard {
        today,
        totals,
        activity: activity(pool, time_zone).await?,
        trend: trend(pool, time_zone).await?,
        departments: departments(pool).await?,
    })
}

/// Alti sayi ve bugunun tarihi tek sorguda: panel acilirken yedi ayri gidis
/// donus olmasin.
async fn totals(pool: &PgPool, time_zone: &str) -> Result<(Totals, String), sqlx::Error> {
    let row: (i64, i64, i64, i64, i64, i64, String) = sqlx::query_as(
        "SELECT \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL), \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL \
              AND start_date > (now() AT TIME ZONE $1)::date - $2), \
           (SELECT count(*) FROM identities WHERE deleted_at IS NULL \
              AND end_at IS NOT NULL AND end_at <= now() \
              AND end_at > now() - make_interval(days => $2)), \
           (SELECT count(DISTINCT identity_id) FROM audit_log \
              WHERE event_type = $3 AND occurred_at > now() - make_interval(days => $2)), \
           (SELECT count(*) FROM jobs WHERE status = 'needs_intervention'), \
           (SELECT count(*) FROM roles WHERE pending_definition IS NOT NULL) \
           + (SELECT count(*) FROM departments WHERE pending_definition IS NOT NULL), \
           to_char(now() AT TIME ZONE $1, 'DD.MM.YYYY')",
    )
    .bind(time_zone)
    .bind(WINDOW_DAYS)
    .bind(audit::IDENTITY_CHANGED)
    .fetch_one(pool)
    .await?;
    Ok((
        Totals {
            identities: row.0,
            joined: row.1,
            departed: row.2,
            changed: row.3,
            needs_intervention: row.4,
            pending_approvals: row.5,
        },
        row.6,
    ))
}

/// Son etkinlikler: yalnizca operatorun yaptiklari (`actor_username` dolu).
/// Worker'in niyet/sonuc satirlari akisa girmez; onlarin yeri kisi sayfasi.
async fn activity(pool: &PgPool, time_zone: &str) -> Result<Vec<Event>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT a.event_type, a.actor_username, i.given_name || ' ' || i.surname, \
                to_char(a.occurred_at AT TIME ZONE $1, 'DD.MM HH24:MI') \
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
        .map(|(event_type, actor, person, time)| Event {
            icon: icon_for(&event_type),
            event_type,
            actor: actor.unwrap_or_default(),
            person: person.unwrap_or_default(),
            time,
        })
        .collect())
}

/// Eğilim: son yedi gun, gun basina kayit ve ayrilis sayisi. Gunler
/// `generate_series` ile uretilir, boylece olaysiz gun de sutun olarak cizilir.
async fn trend(pool: &PgPool, time_zone: &str) -> Result<Vec<TrendDay>, sqlx::Error> {
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
    let peak = rows
        .iter()
        .map(|(_, joined, departed)| joined.max(departed))
        .max()
        .copied()
        .unwrap_or(0);
    Ok(rows
        .into_iter()
        .map(|(label, joined, departed)| TrendDay {
            label,
            joined,
            departed,
            joined_pct: percent(joined, peak),
            departed_pct: percent(departed, peak),
        })
        .collect())
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

/// Zirveye gore yuzde, en yakin bese yuvarlanmis: sablon `.v-NN` sinifini
/// secebilsin diye (satir ici `style` CSP'de yasak). Sifir olmayan deger hic
/// gorunmeyecek kadar kisa cizilmesin diye en az 5 doner.
fn percent(value: i64, peak: i64) -> i64 {
    if value <= 0 || peak <= 0 {
        return 0;
    }
    // Once yuzdeyi hesaplayip sonra yuvarlamak iki kez kirpardi (7/9 -> %77 -> 75);
    // dogrudan "en yakin beste bir dilim" sayisina yuvarlanir.
    let fifths = (value * 20 + peak / 2) / peak;
    (fifths * 5).clamp(5, 100)
}

/// Olay turunun ikonu; bilinmeyen tur notr ikon alir (ekran bozulmaz).
fn icon_for(event_type: &str) -> &'static str {
    match event_type {
        audit::IDENTITY_CREATED => "ico-user-plus",
        audit::IDENTITY_CHANGED | audit::TARGET_CHANGED | audit::MAPPING_CHANGED => "ico-refresh",
        audit::IDENTITY_ROLE_ASSIGNED | audit::IDENTITY_ROLE_REMOVED | audit::ROLE_CHANGED => {
            "ico-key"
        }
        audit::DEPARTMENT_CHANGED => "ico-sitemap",
        audit::IDENTITY_DEPARTURE_SET
        | audit::IDENTITY_EMERGENCY_DEPARTURE
        | audit::IDENTITY_DEPARTURE_REVERTED
        | audit::IDENTITY_CANCELLED => "ico-logout",
        audit::IDENTITY_SUSPENDED | audit::IDENTITY_SUSPENSION_LIFTED => "ico-clock",
        audit::FIRST_PASSWORD_REQUESTED | audit::FIRST_PASSWORD_SHOWN => "ico-lock",
        audit::OPERATOR_LOGIN | audit::OPERATOR_REJECTED => "ico-sign-in",
        audit::SETTINGS_CHANGED | audit::BOOTSTRAP_PASSWORD_CHANGED => "ico-cog",
        audit::USED_NAME_RELEASED | audit::IDENTITY_NAME_REQUESTED => "ico-tag",
        _ => "ico-inbox",
    }
}
// --- END FEATURE: dashboard ---

#[cfg(test)]
mod tests {
    use super::*;

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

        let dash = load(&pool, "Europe/Istanbul").await.unwrap();

        assert_eq!(dash.totals.identities, 2, "silinmemis kimlik");
        assert_eq!(
            dash.totals.joined, 1,
            "40 gun onceki giris pencereye girmez"
        );
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

        drop(pool);
        crate::test_support::drop_temp_db(&admin_pool, &db_name).await;
    }

    #[test]
    fn known_events_get_their_own_icon_and_unknown_ones_fall_back() {
        assert_eq!(icon_for(audit::IDENTITY_CREATED), "ico-user-plus");
        assert_eq!(icon_for(audit::IDENTITY_EMERGENCY_DEPARTURE), "ico-logout");
        assert_eq!(icon_for("bilinmeyen.olay"), "ico-inbox");
    }
}
