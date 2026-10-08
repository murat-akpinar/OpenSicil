// --- START FEATURE: json-logs ---
// Log satirlari JSON (ADR-113): her satir tek nesne, alan adlari OpenTelemetry
// semantic conventions. Tasima collector'in isi; uygulama OTLP konusmaz.
// backend/src/log.rs ve worker/src/log.rs birebir aynidir (ADR-070) — backend
// main.rs testi ikisini karsilastirir. Bagimlilik yok: kacis ve zaman elle.

use std::time::{SystemTime, UNIX_EPOCH};

const SERVICE: &str = concat!("opensicil-", env!("CARGO_PKG_NAME"));

/// JSON dize sabiti (tirnaklariyla): `"`, `\` ve kontrol karakterleri kacirilir;
/// LDAP hata metnindeki satir sonu satiri bolmez.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Saf: tek log satiri. `fields` degerleri hazir JSON (`quote(..)` ya da sayi).
pub fn line(timestamp: &str, severity: &str, body: &str, fields: &[(&str, String)]) -> String {
    let mut out = format!(
        "{{\"timestamp\":{},\"severity\":{},\"service.name\":{},\"body\":{}",
        quote(timestamp),
        quote(severity),
        quote(SERVICE),
        quote(body)
    );
    for (key, value) in fields {
        out.push_str(&format!(",{}:{value}", quote(key)));
    }
    out.push('}');
    out
}

/// UTC, RFC 3339, milisaniye (`2026-10-08T01:02:03.456Z`).
pub fn now() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    utc(since.as_secs(), since.subsec_millis())
}

// Gunden takvime (Howard Hinnant, civil_from_days); 1970 sonrasi icin yeterli.
fn utc(secs: u64, millis: u32) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn emit(severity: &str, body: &str, fields: &[(&str, String)]) {
    let text = line(&now(), severity, body, fields);
    if severity == "ERROR" {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}

/// Denetim satirinin SIEM kopyasi (ADR-113 madde 4). DB satiri yetkili kayittir;
/// buraya detay (deger alanlari) girmez — kim, ne, hangi kimlik/hedef, sonuc.
/// Kimlik ic numarasiyla gider, ad ya da kisisel veri tasimaz (docs/07).
pub fn audit_fields(
    event: &str,
    actor: Option<&str>,
    identity: Option<i64>,
    target: Option<i64>,
    outcome: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("event.category", quote("iam")),
        ("event.name", quote(event)),
    ];
    fields.extend(actor.map(|a| ("user.name", quote(a))));
    fields.extend(identity.map(|i| ("opensicil.identity.id", i.to_string())));
    fields.extend(target.map(|t| ("opensicil.target_system.id", t.to_string())));
    fields.extend(outcome.map(|o| ("event.outcome", quote(o))));
    fields
}

pub fn audit(
    event: &str,
    actor: Option<&str>,
    identity: Option<i64>,
    target: Option<i64>,
    outcome: Option<&str>,
) {
    let fields = audit_fields(event, actor, identity, target, outcome);
    emit("INFO", &format!("denetim: {event}"), &fields);
}

/// `println!` yerine: govde `format!` ile, satir JSON.
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log::emit("INFO", &format!($($arg)*), &[]) };
}

/// `eprintln!` yerine.
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::log::emit("ERROR", &format!($($arg)*), &[]) };
}
// --- END FEATURE: json-logs ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_is_one_json_object_with_escaped_body_and_typed_fields() {
        let text = line(
            "2026-10-08T00:00:00.000Z",
            "INFO",
            "GET \"x\"\nsatır\t\u{1}",
            &[
                ("http.response.status_code", "200".into()),
                ("url.path", quote("/a")),
            ],
        );
        assert!(!text.contains('\n'), "tek satır");
        assert!(
            text.starts_with("{\"timestamp\":\"2026-10-08T00:00:00.000Z\",\"severity\":\"INFO\",")
        );
        assert!(text.contains("\"service.name\":\"opensicil-"));
        assert!(
            text.contains("\"body\":\"GET \\\"x\\\"\\nsatır\\t\\u0001\""),
            "{text}"
        );
        assert!(text.ends_with(",\"http.response.status_code\":200,\"url.path\":\"/a\"}"));
    }

    #[test]
    fn audit_line_carries_who_what_and_no_detail() {
        let fields = audit_fields("identity.changed", Some("ayse"), Some(7), None, None);
        let text = line("t", "INFO", "denetim: identity.changed", &fields);
        assert!(text.contains("\"event.category\":\"iam\""), "{text}");
        assert!(text.contains("\"event.name\":\"identity.changed\""));
        assert!(text.contains("\"user.name\":\"ayse\""));
        assert!(text.contains("\"opensicil.identity.id\":7"));
        assert!(!text.contains("opensicil.target_system.id") && !text.contains("event.outcome"));
        let worker = audit_fields(
            "ad.account.created",
            None,
            Some(1),
            Some(2),
            Some("succeeded"),
        );
        assert_eq!(worker.len(), 5);
    }

    #[test]
    fn utc_formats_known_instants() {
        assert_eq!(utc(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(utc(951_782_400, 7), "2000-02-29T00:00:00.007Z");
        assert_eq!(utc(1_791_421_323, 456), "2026-10-08T01:02:03.456Z");
    }
}
