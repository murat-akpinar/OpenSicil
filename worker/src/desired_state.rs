// --- START FEATURE: desired-state ---
// Olmasi gereken durum: saf fonksiyon, yan etkisi yok (docs/02 motor; ADR-007,
// 017, 020, 024, 033, 038, 040, 041, 048, 053, 059). 3a motoru, 3f etki
// onizlemesi ve Faz 5 toplu yonetime alma ayni fonksiyonu cagirir; ikinci bir
// fark hesabi yazilmaz. Zaman girdi olarak gelir: `now` Unix saniyesi ve ayni
// anin kurulum saat dilimindeki gunu; tz kutuphanesi cagiranin isidir.
// backend/src/desired_state.rs ve worker/src/desired_state.rs birebir aynidir
// (ADR-070: bagimsiz crate'ler) — birini degistiren digerini de degistirir.

use std::collections::BTreeSet;

pub type CatalogItemId = i64;
pub type IdentityId = i64;

const SECONDS_PER_DAY: i64 = 86_400;

/// Kurulum saat dilimindeki takvim gunu; alan sirasi karsilastirmayi dogru kilar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Date {
    /// "YYYY-MM-DD" (Postgres `to_char` ciktisi); baska bicim kabul edilmez.
    pub fn from_iso(text: &str) -> Option<Date> {
        let mut parts = text.split('-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        let in_range = (1..=12).contains(&month) && (1..=31).contains(&day);
        (parts.next().is_none() && in_range).then_some(Date { year, month, day })
    }
}

/// `now` Unix saniyesi, `today` ayni anin kurulum dilimindeki gunu.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub now: i64,
    pub today: Date,
}

/// ADR-038 sirasi: ilk tutan kazanir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Deleted,
    Departed,
    Suspended,
    Pending,
    Active,
}

/// Kimligin sakladigi zaman bilgisi ve isaretleri (docs/03; ADR-038, 053).
#[derive(Debug, Clone)]
pub struct Timeline {
    pub start_date: Date,
    /// Bitis ANI: planlida ertesi gun 00:00, acil ve iptalde simdi.
    pub end_at: Option<i64>,
    pub suspension_start: Option<Date>,
    /// Iznin son gunu; ertesi gun 00:00'da acilir (ADR-059).
    pub suspension_end: Option<Date>,
    pub cancelled: bool,
    pub emergency_departure: bool,
    pub deleted_at: Option<i64>,
}

pub fn lifecycle_state(t: &Timeline, clock: &Clock) -> LifecycleState {
    if t.deleted_at.is_some() {
        return LifecycleState::Deleted;
    }
    if t.end_at.is_some_and(|end| end <= clock.now) {
        return LifecycleState::Departed;
    }
    let suspended = t.suspension_start.is_some_and(|s| s <= clock.today)
        && t.suspension_end.is_none_or(|e| clock.today <= e);
    if suspended {
        return LifecycleState::Suspended;
    }
    if t.start_date > clock.today {
        return LifecycleState::Pending;
    }
    LifecycleState::Active
}

/// Tek degerli ayarlar; None = "bu kaynak soylemiyor" (ADR-007, 017).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SingleValued {
    pub provision_account: Option<bool>,
    pub container: Option<CatalogItemId>,
    pub email_domain: Option<String>,
    pub upn_suffix: Option<String>,
}

/// Bir hedef sistem icin yetki ogeleri ve tek degerli ayarlar tasiyan kaynak.
#[derive(Debug, Clone, Default)]
pub struct Source {
    pub entitlements: Vec<CatalogItemId>,
    pub settings: SingleValued,
}

#[derive(Debug, Clone)]
pub struct AdditionalRole {
    pub entitlements: Vec<CatalogItemId>,
    /// Gunun sonu olarak yorumlanir (ADR-020).
    pub ends_on: Option<Date>,
}

#[derive(Debug, Clone)]
pub struct TargetDefaults {
    pub provision_account: bool,
    pub container: Option<CatalogItemId>,
    pub retention_days: u32,
    pub delete_requires_approval: bool,
    /// ADR-033 G; 0 = ayrilista hemen.
    pub password_reset_delay_days: u32,
}

/// Bir kimlik, bir hedef sistem icin yayimlanmis model.
#[derive(Debug, Clone)]
pub struct Model {
    pub base_entitlements: Vec<CatalogItemId>,
    /// Kimligin departmani once, kok sonda (ADR-017 "yakindan koke").
    pub department_chain: Vec<Source>,
    pub primary_role: Source,
    pub title: Option<String>,
    pub additional_roles: Vec<AdditionalRole>,
    pub target: TargetDefaults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Provisioned,
    Adopted,
}

/// Hesap baglantisinin fonksiyona giren kismi; yalnizca worker yazar (ADR-015).
#[derive(Debug, Clone)]
pub struct AccountLink {
    pub origin: Origin,
    /// ADR-048: hedefte "hic kullanilmamis" dogrulamasi; None = henuz bakilmadi.
    pub verified_unused: Option<bool>,
    /// ADR-024: onay gerektiren hedefte silme onaylandi.
    pub deletion_approved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountPresence {
    /// Hesap olmamali: silinmis, saklama dolmus ya da dogrulanmis iptal.
    Absent,
    /// Saklama doldu ama bu hedef onay ister; "silinmeyi bekliyor" (ADR-024).
    AwaitingDeletionApproval,
    /// Hesap yok ve "hesap acilsin = hayir": acilmaz (ADR-040).
    NotProvisioned,
    Present {
        enabled: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Item(CatalogItemId),
    /// Ayrilanin pasif OU'su; kurulumda tanimliysa (docs/04).
    Passive,
    /// Askida degismez (docs/04).
    Unchanged,
    /// Hicbir kaynak soylemiyor.
    Unspecified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredState {
    pub state: LifecycleState,
    pub account: AccountPresence,
    pub container: Container,
    pub memberships: BTreeSet<CatalogItemId>,
    pub title: Option<String>,
    pub email_domain: Option<String>,
    pub upn_suffix: Option<String>,
    /// None = suresiz (ADR-059 `accountExpires = 0`).
    pub account_expires: Option<i64>,
    /// ADR-033: parola rastgelelestirilmeli.
    pub password_reset_due: bool,
    /// ADR-048: iptal hedefte dogrulandi; saklama sifir, parola sifirlanmaz.
    pub cancellation_effective: bool,
    /// ADR-040: rol/departman "hesap acilsin = hayir" diyor ama bagli hesap var;
    /// hesap yonetilmeye devam eder, mutabakat "rol hesap ongormuyor" bilgisi yazar.
    pub provision_not_expected: bool,
}

pub fn desired_state(
    t: &Timeline,
    model: &Model,
    link: Option<&AccountLink>,
    clock: &Clock,
) -> DesiredState {
    let state = lifecycle_state(t, clock);
    let settings = resolve_single_valued(model);
    // Hic hesabi olmayan iptal dogrudan uygulanir (ADR-038); hesabi olanda
    // yalnizca OpenSicil'in actigi ve hedefte kullanilmamis dogrulanan (ADR-048).
    let cancellation_effective = t.cancelled
        && link.is_none_or(|l| l.origin == Origin::Provisioned && l.verified_unused == Some(true));
    let account = account_presence(state, t, model, link, cancellation_effective, clock);
    let memberships = match state {
        LifecycleState::Departed | LifecycleState::Deleted => BTreeSet::new(),
        _ => effective_entitlements(model, clock),
    };
    let container = match (state, account) {
        (_, AccountPresence::Absent | AccountPresence::NotProvisioned) => Container::Unspecified,
        (LifecycleState::Departed, _) => Container::Passive,
        (LifecycleState::Suspended, _) => Container::Unchanged,
        _ => settings
            .container
            .map_or(Container::Unspecified, Container::Item),
    };
    let password_reset_due = state == LifecycleState::Departed
        && !cancellation_effective
        && (t.emergency_departure
            || t.end_at.is_some_and(|end| {
                end + days(model.target.password_reset_delay_days) <= clock.now
            }));
    DesiredState {
        state,
        account,
        container,
        memberships,
        title: model.title.clone(),
        email_domain: settings.email_domain,
        upn_suffix: settings.upn_suffix,
        account_expires: t.end_at,
        password_reset_due,
        cancellation_effective,
        provision_not_expected: link.is_some() && !provision_expected(model),
    }
}

fn provision_expected(model: &Model) -> bool {
    resolve_single_valued(model)
        .provision_account
        .unwrap_or(model.target.provision_account)
}

fn days(count: u32) -> i64 {
    i64::from(count) * SECONDS_PER_DAY
}

fn account_presence(
    state: LifecycleState,
    t: &Timeline,
    model: &Model,
    link: Option<&AccountLink>,
    cancellation_effective: bool,
    clock: &Clock,
) -> AccountPresence {
    match state {
        LifecycleState::Deleted => AccountPresence::Absent,
        LifecycleState::Departed => {
            if cancellation_effective {
                return AccountPresence::Absent;
            }
            let retention_over = t
                .end_at
                .is_some_and(|end| end + days(model.target.retention_days) <= clock.now);
            if !retention_over {
                AccountPresence::Present { enabled: false }
            } else if model.target.delete_requires_approval
                && !link.is_some_and(|l| l.deletion_approved)
            {
                AccountPresence::AwaitingDeletionApproval
            } else {
                AccountPresence::Absent
            }
        }
        LifecycleState::Active => present_if_provisioned(model, link, true),
        LifecycleState::Pending | LifecycleState::Suspended => {
            present_if_provisioned(model, link, false)
        }
    }
}

// ADR-040: "hesap acilsin mi" yalnizca hesap yokken okunur; bagli hesap varsa
// `hayir` onu silmez, kapatmaz.
fn present_if_provisioned(
    model: &Model,
    link: Option<&AccountLink>,
    enabled: bool,
) -> AccountPresence {
    if link.is_none() && !provision_expected(model) {
        AccountPresence::NotProvisioned
    } else {
        AccountPresence::Present { enabled }
    }
}

/// Temel ∪ departman zinciri ∪ birincil ∪ suresi dolmamis ek roller (ADR-007, 017, 020).
pub fn effective_entitlements(model: &Model, clock: &Clock) -> BTreeSet<CatalogItemId> {
    let departments = model.department_chain.iter().flat_map(|d| &d.entitlements);
    let additional = model
        .additional_roles
        .iter()
        .filter(|r| r.ends_on.is_none_or(|end| clock.today <= end))
        .flat_map(|r| &r.entitlements);
    model
        .base_entitlements
        .iter()
        .chain(departments)
        .chain(&model.primary_role.entitlements)
        .chain(additional)
        .copied()
        .collect()
}

/// Birincil rol → departman → ust departmanlar (yakindan koke); ilk dolu kazanir.
/// Hedef sistem varsayilani `provision_account` ve `container` icin ayrica okunur.
pub fn resolve_single_valued(model: &Model) -> SingleValued {
    let mut sources = std::iter::once(&model.primary_role.settings)
        .chain(model.department_chain.iter().map(|d| &d.settings));
    let mut resolved = SingleValued::default();
    for s in sources.by_ref() {
        resolved.provision_account = resolved.provision_account.or(s.provision_account);
        resolved.container = resolved.container.or(s.container);
        resolved.email_domain = resolved
            .email_domain
            .clone()
            .or_else(|| s.email_domain.clone());
        resolved.upn_suffix = resolved.upn_suffix.clone().or_else(|| s.upn_suffix.clone());
    }
    resolved.container = resolved.container.or(model.target.container);
    resolved
}

#[derive(Debug, Clone)]
pub struct ManagerChain {
    pub manager: IdentityId,
    pub manager_state: LifecycleState,
    /// Yoneticinin kaydindaki devir yoneticisi ve onun durumu.
    pub handover: Option<(IdentityId, LifecycleState)>,
}

/// ADR-041: yonetici ayrilmamissa kendisi; ayrilmissa devir yoneticisi (tek
/// atlama); o da ayrilmissa bos. Hedefte hesabi olup olmadigi cagiranin isi (ADR-040).
pub fn effective_manager(chain: Option<&ManagerChain>) -> Option<IdentityId> {
    let c = chain?;
    let gone = |s: LifecycleState| matches!(s, LifecycleState::Departed | LifecycleState::Deleted);
    if !gone(c.manager_state) {
        return Some(c.manager);
    }
    c.handover.filter(|(_, s)| !gone(*s)).map(|(id, _)| id)
}
// --- END FEATURE: desired-state ---

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = SECONDS_PER_DAY;
    // 2026-03-10 00:00 UTC; testler "kurulum dilimi = UTC" varsayar
    const NOW: i64 = 1_773_100_800;

    fn d(month: u8, day: u8) -> Date {
        Date {
            year: 2026,
            month,
            day,
        }
    }

    fn clock() -> Clock {
        Clock {
            now: NOW,
            today: d(3, 10),
        }
    }

    fn active_timeline() -> Timeline {
        Timeline {
            start_date: d(1, 1),
            end_at: None,
            suspension_start: None,
            suspension_end: None,
            cancelled: false,
            emergency_departure: false,
            deleted_at: None,
        }
    }

    fn departed(days_ago: i64) -> Timeline {
        Timeline {
            end_at: Some(NOW - days_ago * DAY),
            ..active_timeline()
        }
    }

    fn source(entitlements: &[i64], container: Option<i64>) -> Source {
        Source {
            entitlements: entitlements.to_vec(),
            settings: SingleValued {
                container,
                ..SingleValued::default()
            },
        }
    }

    // docs/03 ornegi: Bilgi Islem'de Nobet Ekibi'nde de olan Sistem Uzmani (AD)
    fn example_model() -> Model {
        Model {
            base_entitlements: vec![1], // GG-Internet
            department_chain: vec![source(&[2], None), source(&[3], Some(90))], // BT, Ankara
            primary_role: source(&[4, 5], Some(20)), // GG-Sistem-Uzmanlari, GG-VPN; OU SistemUzmanlari
            title: Some("Sistem Uzmanı".to_string()),
            additional_roles: vec![AdditionalRole {
                entitlements: vec![6], // GG-Nobet
                ends_on: Some(d(3, 10)),
            }],
            target: TargetDefaults {
                provision_account: true,
                container: Some(10),
                retention_days: 90,
                delete_requires_approval: false,
                password_reset_delay_days: 7,
            },
        }
    }

    fn provisioned_link() -> AccountLink {
        AccountLink {
            origin: Origin::Provisioned,
            verified_unused: None,
            deletion_approved: false,
        }
    }

    #[test]
    fn date_from_iso_accepts_only_well_formed_dates() {
        assert_eq!(Date::from_iso("2026-03-10"), Some(d(3, 10)));
        assert_eq!(Date::from_iso("2026-13-01"), None);
        assert_eq!(Date::from_iso("2026-03-10-1"), None);
        assert_eq!(Date::from_iso("bugün"), None);
    }

    #[test]
    fn lifecycle_state_follows_adr_038_order() {
        use LifecycleState::*;
        let cases: [(&str, Timeline, LifecycleState); 9] = [
            ("aktif", active_timeline(), Active),
            (
                "ileri başlangıç → bekliyor",
                Timeline {
                    start_date: d(3, 11),
                    ..active_timeline()
                },
                Pending,
            ),
            (
                "bugün başlayan → aktif",
                Timeline {
                    start_date: d(3, 10),
                    ..active_timeline()
                },
                Active,
            ),
            ("bitiş anı geçti → ayrıldı", departed(1), Departed),
            (
                "bitiş anı ileride → aktif",
                Timeline {
                    end_at: Some(NOW + DAY),
                    ..active_timeline()
                },
                Active,
            ),
            (
                "bekliyor iken bitiş geçmiş → doğrudan ayrıldı (ADR-038)",
                Timeline {
                    start_date: d(3, 11),
                    end_at: Some(NOW - 1),
                    ..active_timeline()
                },
                Departed,
            ),
            (
                "askıdayken bitiş geçmiş → ayrıldı (ADR-053)",
                Timeline {
                    suspension_start: Some(d(3, 1)),
                    end_at: Some(NOW - 1),
                    ..active_timeline()
                },
                Departed,
            ),
            (
                "silindi her şeyi ezer",
                Timeline {
                    deleted_at: Some(NOW - 1),
                    end_at: Some(NOW - 1),
                    ..active_timeline()
                },
                Deleted,
            ),
            (
                "ileri tarihli askı henüz değil",
                Timeline {
                    suspension_start: Some(d(3, 11)),
                    ..active_timeline()
                },
                Active,
            ),
        ];
        for (why, timeline, expected) in cases {
            assert_eq!(lifecycle_state(&timeline, &clock()), expected, "{why}");
        }
    }

    #[test]
    fn suspension_end_is_last_day_of_leave() {
        use LifecycleState::*;
        let suspended = |end: Option<Date>| Timeline {
            suspension_start: Some(d(3, 1)),
            suspension_end: end,
            ..active_timeline()
        };
        assert_eq!(
            lifecycle_state(&suspended(Some(d(3, 10))), &clock()),
            Suspended,
            "son gün dahil"
        );
        assert_eq!(
            lifecycle_state(&suspended(Some(d(3, 9))), &clock()),
            Active,
            "ertesi gün açılır (ADR-059)"
        );
        assert_eq!(
            lifecycle_state(&suspended(None), &clock()),
            Suspended,
            "tarihsiz askı sürer"
        );
    }

    #[test]
    fn active_identity_gets_union_of_layers_and_primary_role_container() {
        let s = desired_state(
            &active_timeline(),
            &example_model(),
            Some(&provisioned_link()),
            &clock(),
        );
        assert_eq!(s.account, AccountPresence::Present { enabled: true });
        assert_eq!(
            s.container,
            Container::Item(20),
            "birincil rol departmanı ezer"
        );
        assert_eq!(s.memberships, BTreeSet::from([1, 2, 3, 4, 5, 6]));
        assert_eq!(s.title.as_deref(), Some("Sistem Uzmanı"));
        assert_eq!(s.account_expires, None, "bitiş yoksa süresiz (ADR-059)");
        assert!(!s.password_reset_due);
    }

    #[test]
    fn expired_additional_role_drops_out_next_day() {
        let mut model = example_model();
        model.additional_roles[0].ends_on = Some(d(3, 9));
        let s = desired_state(
            &active_timeline(),
            &model,
            Some(&provisioned_link()),
            &clock(),
        );
        assert!(!s.memberships.contains(&6), "günün sonu geçti (ADR-020)");
    }

    #[test]
    fn single_valued_falls_through_department_chain_to_target_default() {
        let mut model = example_model();
        model.primary_role.settings.container = None;
        model.department_chain[0].settings.container = None;
        assert_eq!(
            resolve_single_valued(&model).container,
            Some(90),
            "üst departman (Ankara)"
        );
        model.department_chain[1].settings.container = None;
        assert_eq!(
            resolve_single_valued(&model).container,
            Some(10),
            "hedef varsayılanı"
        );
        model.department_chain[0].settings.email_domain = Some("bt.example.com".into());
        model.department_chain[1].settings.email_domain = Some("example.com".into());
        assert_eq!(
            resolve_single_valued(&model).email_domain.as_deref(),
            Some("bt.example.com"),
            "yakın departman kazanır"
        );
    }

    #[test]
    fn pending_and_suspended_keep_account_disabled_and_memberships() {
        let model = example_model();
        let pending = Timeline {
            start_date: d(3, 11),
            ..active_timeline()
        };
        let s = desired_state(&pending, &model, Some(&provisioned_link()), &clock());
        assert_eq!(s.account, AccountPresence::Present { enabled: false });
        assert_eq!(s.container, Container::Item(20));

        let suspended = Timeline {
            suspension_start: Some(d(3, 1)),
            ..active_timeline()
        };
        let s = desired_state(&suspended, &model, Some(&provisioned_link()), &clock());
        assert_eq!(s.account, AccountPresence::Present { enabled: false });
        assert_eq!(s.container, Container::Unchanged, "askıda OU değişmez");
        assert_eq!(s.memberships.len(), 6, "askıda gruplar korunur");
    }

    #[test]
    fn departed_identity_loses_memberships_and_password_after_window() {
        let model = example_model();
        let fresh = desired_state(&departed(1), &model, Some(&provisioned_link()), &clock());
        assert_eq!(fresh.account, AccountPresence::Present { enabled: false });
        assert_eq!(fresh.container, Container::Passive);
        assert!(fresh.memberships.is_empty(), "katalog grupları kaldırılır");
        assert!(!fresh.password_reset_due, "7 gün dolmadı (ADR-033)");
        assert_eq!(fresh.account_expires, departed(1).end_at);

        let week = desired_state(&departed(7), &model, Some(&provisioned_link()), &clock());
        assert!(week.password_reset_due, "7. gün dolunca sıfırlanır");

        let emergency = Timeline {
            emergency_departure: true,
            ..departed(0)
        };
        let s = desired_state(&emergency, &model, Some(&provisioned_link()), &clock());
        assert!(s.password_reset_due, "acil ayrılışta hemen");
    }

    #[test]
    fn retention_deletes_ad_but_zimbra_waits_for_approval() {
        let mut model = example_model();
        let s = desired_state(&departed(90), &model, Some(&provisioned_link()), &clock());
        assert_eq!(
            s.account,
            AccountPresence::Absent,
            "AD 90 gün sonra silinir (ADR-024)"
        );
        let s = desired_state(&departed(89), &model, Some(&provisioned_link()), &clock());
        assert_eq!(s.account, AccountPresence::Present { enabled: false });

        model.target.delete_requires_approval = true;
        let s = desired_state(&departed(90), &model, Some(&provisioned_link()), &clock());
        assert_eq!(s.account, AccountPresence::AwaitingDeletionApproval);
        let approved = AccountLink {
            deletion_approved: true,
            ..provisioned_link()
        };
        let s = desired_state(&departed(90), &model, Some(&approved), &clock());
        assert_eq!(s.account, AccountPresence::Absent);
    }

    #[test]
    fn cancellation_only_effective_when_target_verified_unused_and_provisioned() {
        let model = example_model();
        let cancelled = Timeline {
            cancelled: true,
            ..departed(0)
        };
        let verified = AccountLink {
            verified_unused: Some(true),
            ..provisioned_link()
        };
        let s = desired_state(&cancelled, &model, Some(&verified), &clock());
        assert!(s.cancellation_effective);
        assert_eq!(s.account, AccountPresence::Absent, "saklama sıfır");
        assert!(!s.password_reset_due, "iptalde parola sıfırlanmaz");

        let used = AccountLink {
            verified_unused: Some(false),
            ..provisioned_link()
        };
        let s = desired_state(&cancelled, &model, Some(&used), &clock());
        assert!(
            !s.cancellation_effective,
            "kullanılmış hesap: planlı ayrılış gibi (ADR-048)"
        );
        assert_eq!(s.account, AccountPresence::Present { enabled: false });

        let adopted = AccountLink {
            origin: Origin::Adopted,
            verified_unused: Some(true),
            ..provisioned_link()
        };
        let s = desired_state(&cancelled, &model, Some(&adopted), &clock());
        assert!(
            !s.cancellation_effective,
            "sahiplenilen hesap iptal edilemez"
        );

        let s = desired_state(&cancelled, &model, None, &clock());
        assert!(
            s.cancellation_effective,
            "hiç hesabı yoksa iptal doğrudan (ADR-038)"
        );
        assert_eq!(s.account, AccountPresence::Absent);
    }

    // ADR-040: ayar `hayir` + bagli hesap → yonetim surer, yalnizca bilgi bayragi.
    #[test]
    fn provision_not_expected_flag_only_with_linked_account() {
        let mut model = example_model();
        let on = desired_state(
            &active_timeline(),
            &model,
            Some(&provisioned_link()),
            &clock(),
        );
        assert!(!on.provision_not_expected);
        model.primary_role.settings.provision_account = Some(false);
        let linked = desired_state(
            &active_timeline(),
            &model,
            Some(&provisioned_link()),
            &clock(),
        );
        assert!(linked.provision_not_expected);
        assert_eq!(linked.account, AccountPresence::Present { enabled: true });
        assert!(
            matches!(linked.container, Container::Item(_)),
            "bagli hesabin OU'su role gore surer"
        );
        let unlinked = desired_state(&active_timeline(), &model, None, &clock());
        assert!(
            !unlinked.provision_not_expected,
            "hesap yoksa bilgi degil karar"
        );
        assert_eq!(unlinked.account, AccountPresence::NotProvisioned);
    }

    #[test]
    fn provision_account_setting_is_only_read_when_no_account_exists() {
        let mut model = example_model();
        model.primary_role.settings.provision_account = Some(false);
        let s = desired_state(&active_timeline(), &model, None, &clock());
        assert_eq!(s.account, AccountPresence::NotProvisioned);
        assert_eq!(s.container, Container::Unspecified);

        let s = desired_state(
            &active_timeline(),
            &model,
            Some(&provisioned_link()),
            &clock(),
        );
        assert_eq!(
            s.account,
            AccountPresence::Present { enabled: true },
            "mevcut hesap silinmez (ADR-040)"
        );
    }

    #[test]
    fn effective_manager_hops_once_to_handover() {
        use LifecycleState::*;
        let chain = |manager_state, handover| ManagerChain {
            manager: 1,
            manager_state,
            handover,
        };
        assert_eq!(effective_manager(None), None);
        assert_eq!(
            effective_manager(Some(&chain(Active, Some((2, Active))))),
            Some(1)
        );
        assert_eq!(
            effective_manager(Some(&chain(Suspended, None))),
            Some(1),
            "askı geçiş değildir"
        );
        assert_eq!(
            effective_manager(Some(&chain(Departed, Some((2, Active))))),
            Some(2)
        );
        assert_eq!(
            effective_manager(Some(&chain(Departed, Some((2, Departed))))),
            None,
            "tek atlama"
        );
        assert_eq!(effective_manager(Some(&chain(Deleted, None))), None);
    }
}
