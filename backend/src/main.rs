mod db;
mod health;
mod logging;
mod migrate;
mod server;

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
}
