mod db;
mod health;
mod logging;
mod migrate;
mod server;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("migrate") => migrate::run().await,
        Some(other) => {
            eprintln!("backend: bilinmeyen komut: {other}");
            ExitCode::FAILURE
        }
        None => server::run().await,
    }
}
