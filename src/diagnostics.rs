//! Limited local diagnostics; never record document content, paths or encoder output.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_LOG: u64 = 256 * 1024;
static LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
#[derive(Clone, Copy)]
pub enum Event {
    RecoveryUnavailable,
    RecoveryWrite,
    RecoveryDelete,
    ProjectOpen,
    ProjectSave,
    Export,
}
impl Event {
    fn code(self) -> &'static str {
        match self {
            Self::RecoveryUnavailable => "recovery_unavailable",
            Self::RecoveryWrite => "recovery_write_failed",
            Self::RecoveryDelete => "recovery_delete_failed",
            Self::ProjectOpen => "project_open_failed",
            Self::ProjectSave => "project_save_failed",
            Self::Export => "export_failed",
        }
    }
}
fn write_event(dir: &Path, event: Event, timestamp: u64) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join("diagnostics.log");
    let line = format!("{timestamp} {}\n", event.code());
    let truncate = fs::metadata(&path)
        .map(|m| m.len().saturating_add(line.len() as u64) > MAX_LOG)
        .unwrap_or(false);
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if truncate {
        options.truncate(true);
    } else {
        options.append(true);
    }
    options.open(path)?.write_all(line.as_bytes())
}
pub fn record(event: Event) {
    std::thread::spawn(move || {
        let _guard = LOG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Ok(dir) = crate::recovery::app_directory() {
            if fs::create_dir_all(&dir).is_err() {
                return;
            }
            let Ok(lock) = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("diagnostics.lock"))
            else {
                return;
            };
            if lock.lock().is_err() {
                return;
            }
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let _ = write_event(&dir, event, timestamp);
        }
    });
}
pub fn summary() -> String {
    format!(
        "AnimaPro {}\nОС: {}\nАрхитектура: {}\nФормат проекта: 2\nЖурнал содержит только время и категории ошибок, без рисунков и путей.",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_is_bounded_and_contains_only_known_event_codes() {
        let dir = std::env::temp_dir().join(format!("animapro-diag-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("diagnostics.log"), vec![b'x'; MAX_LOG as usize]).unwrap();
        write_event(&dir, Event::ProjectSave, 123).unwrap();
        let data = fs::read_to_string(dir.join("diagnostics.log")).unwrap();
        assert_eq!(data, "123 project_save_failed\n");
        assert!(data.len() < MAX_LOG as usize);
        assert!(!summary().contains(&dir.to_string_lossy().to_string()));
        fs::remove_dir_all(dir).unwrap();
    }
}
