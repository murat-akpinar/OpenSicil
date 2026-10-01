mod ad_auth;
mod assets;
mod audit;
mod auth;
mod bootstrap_account;
mod bulk_adopt;
mod change_set;
mod common_settings;
mod cookie;
mod crypto;
mod dashboard;
mod db;
// Ikiz dosya (worker ile birebir ayni); backend durum turetme ve etki onizlemesi
// icin cagirir, motora ozel alanlari kullanmaz (ADR-038).
#[allow(dead_code)]
mod desired_state;
mod errors;
mod first_password;
mod health;
mod i18n;
mod identity;
mod identity_web;
mod interventions;
mod jobs;
mod logging;
// Ikiz dosya (worker ile birebir ayni); donusumler yalnizca worker'da calisir.
#[allow(dead_code)]
mod mapping_rules;
mod mapping_web;
mod migrate;
mod national_id;
mod oidc;
mod operator_guard;
mod operator_session;
mod org;
mod org_web;
mod reconcile;
mod reports;
mod search;
mod server;
mod settings;
mod shell;
#[cfg(test)]
mod test_support;
mod token;
mod upcoming;
mod used_names;
mod web;

use std::process::ExitCode;

enum Command {
    Migrate,
    Server,
    Unknown(String),
}

fn parse_command(arg: Option<&str>) -> Command {
    match arg {
        Some("migrate") => Command::Migrate,
        Some(other) => Command::Unknown(other.to_string()),
        None => Command::Server,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match parse_command(std::env::args().nth(1).as_deref()) {
        Command::Migrate => migrate::run().await,
        Command::Unknown(other) => {
            eprintln!("backend: bilinmeyen komut: {other}");
            ExitCode::FAILURE
        }
        Command::Server => server::run().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_argument_runs_server() {
        assert!(matches!(parse_command(None), Command::Server));
    }

    #[test]
    fn migrate_argument_runs_migrate() {
        assert!(matches!(parse_command(Some("migrate")), Command::Migrate));
    }

    #[test]
    fn unknown_argument_is_rejected() {
        match parse_command(Some("bogus")) {
            Command::Unknown(name) => assert_eq!(name, "bogus"),
            _ => panic!("beklenmeyen komut türü"),
        }
    }

    // ADR-070: paylasilan crate yok, iki crate'te birebir ayni dosyalar var.
    // Docker build context'inde worker dizini yoktur; orada test atlanir.
    #[test]
    fn twin_modules_match_worker_copies() {
        for name in [
            "common_settings.rs",
            "desired_state.rs",
            "crypto.rs",
            "mapping_rules.rs",
        ] {
            let mine =
                std::fs::read_to_string(format!("src/{name}")).expect("kendi kopyası okunamadı");
            let Ok(theirs) = std::fs::read_to_string(format!("../worker/src/{name}")) else {
                return;
            };
            assert_eq!(mine, theirs, "{name}: backend ve worker kopyaları ayrıştı");
        }
    }
}
