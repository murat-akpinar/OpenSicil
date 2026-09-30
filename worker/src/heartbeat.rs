use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

const HEARTBEAT_PATH: &str = "/tmp/worker-heartbeat";
const MAX_AGE: Duration = Duration::from_secs(30);

// Worker her döngü turunda bu dosyaya dokunur; worker-health dosya 30 sn'den
// eskiyse 1 döner (ADR-061 süreç sözleşmesi, madde 2).
pub fn touch() -> std::io::Result<()> {
    std::fs::write(HEARTBEAT_PATH, b"")
}

pub fn check() -> ExitCode {
    check_path(Path::new(HEARTBEAT_PATH))
}

fn check_path(path: &Path) -> ExitCode {
    let age = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());

    match age {
        Some(age) if age < MAX_AGE => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "opensicil-worker-heartbeat-test-{}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn touch_creates_fresh_heartbeat_file() {
        touch().expect("nabız dosyasına yazılamadı");
        assert_eq!(check(), ExitCode::SUCCESS);
    }

    #[test]
    fn missing_file_is_unhealthy() {
        let path = test_path("missing");
        std::fs::remove_file(&path).ok();

        assert_eq!(check_path(&path), ExitCode::FAILURE);
    }

    #[test]
    fn fresh_file_is_healthy() {
        let path = test_path("fresh");
        std::fs::write(&path, b"").unwrap();

        assert_eq!(check_path(&path), ExitCode::SUCCESS);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn stale_file_is_unhealthy() {
        let path = test_path("stale");
        std::fs::write(&path, b"").unwrap();
        let stale = SystemTime::now() - Duration::from_secs(45);
        let file = std::fs::File::open(&path).unwrap();
        file.set_modified(stale).unwrap();

        assert_eq!(check_path(&path), ExitCode::FAILURE);

        std::fs::remove_file(&path).ok();
    }
}
