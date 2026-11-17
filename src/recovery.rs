//! Per-session recovery snapshots. A lock distinguishes live sessions from abandoned ones.
use crate::{Project, storage};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

// Recovery adds an original path and timestamp around the project JSON.
const MAX_SNAPSHOT: usize = storage::MAX_JSON as usize + 1024 * 1024;

#[derive(Debug)]
pub(super) enum RecoveryError {
    DirectoryUnavailable,
    Io(std::io::Error),
    LockUnavailable,
    WorkerStopped,
    #[cfg(test)]
    Disabled,
}
#[cfg(test)]
impl From<&str> for RecoveryError {
    fn from(_: &str) -> Self {
        Self::Disabled
    }
}

#[derive(Debug)]
pub(super) enum SnapshotError {
    Project(storage::StorageError),
    Serialization(serde_json::Error),
    TooLarge,
    Io(std::io::Error),
    WorkerStopped,
}

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub project: Project,
    pub source: Option<PathBuf>,
    pub timestamp: u64,
}
impl Snapshot {
    pub fn new(project: Project, source: Option<PathBuf>) -> Self {
        Self {
            project,
            source,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
    pub fn description(&self) -> String {
        let name = self
            .source
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.project.name.clone());
        format!(
            "Проект: {name}\nКопия: {} (UTC)",
            format_time(self.timestamp)
        )
    }
}
fn format_time(seconds: u64) -> String {
    // Gregorian calendar date from days since 1970-01-01.
    let days = (seconds / 86400).min(i64::MAX as u64) as i64;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let time = seconds % 86400;
    format!(
        "{day:02}.{month:02}.{year:04} {:02}:{:02}:{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}
pub(super) fn app_directory() -> Result<PathBuf, RecoveryError> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base =
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    base.map(|p| p.join("AnimaPro"))
        .ok_or(RecoveryError::DirectoryUnavailable)
}

pub struct Candidate {
    pub snapshot: Snapshot,
    path: PathBuf,
    _lock: File,
}
impl Candidate {
    pub fn discard(self) -> Result<(), RecoveryError> {
        remove_if_exists(&self.path)
    }
}
fn remove_if_exists(path: &Path) -> Result<(), RecoveryError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RecoveryError::Io(e)),
    }
}
fn write_snapshot(path: &Path, snapshot: &Snapshot) -> Result<(), SnapshotError> {
    storage::ensure_saveable(&snapshot.project).map_err(SnapshotError::Project)?;
    let data = serde_json::to_vec(snapshot).map_err(SnapshotError::Serialization)?;
    if data.len() > MAX_SNAPSHOT {
        return Err(SnapshotError::TooLarge);
    }
    let temp = path.with_file_name(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(SnapshotError::Io)?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(SnapshotError::Io)?;
        fs::rename(&temp, path).map_err(SnapshotError::Io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
enum Command {
    Save(Snapshot),
    Clear(mpsc::Sender<Result<(), RecoveryError>>),
    Stop,
}
pub struct Recovery {
    directory: PathBuf,
    id: Uuid,
    _lock: File,
    sender: mpsc::Sender<Command>,
    errors: mpsc::Receiver<SnapshotError>,
    pending: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Recovery {
    pub fn new() -> Result<Self, RecoveryError> {
        Self::in_directory(app_directory()?.join("recovery"))
    }
    pub(super) fn in_directory(directory: PathBuf) -> Result<Self, RecoveryError> {
        fs::create_dir_all(&directory).map_err(RecoveryError::Io)?;
        let id = Uuid::new_v4();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.join(format!("{id}.lock")))
            .map_err(RecoveryError::Io)?;
        lock.try_lock()
            .map_err(|_| RecoveryError::LockUnavailable)?;
        let path = directory.join(format!("{id}.json"));
        let (sender, receiver) = mpsc::channel();
        let (error_sender, errors) = mpsc::channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = pending.clone();
        let worker = thread::spawn(move || {
            while let Ok(command) = receiver.recv() {
                match command {
                    Command::Save(snapshot) => {
                        if let Err(e) = write_snapshot(&path, &snapshot) {
                            let _ = error_sender.send(e);
                        }
                        worker_pending.fetch_sub(1, Ordering::Release);
                    }
                    Command::Clear(reply) => {
                        let _ = reply.send(remove_if_exists(&path));
                    }
                    Command::Stop => break,
                }
            }
        });
        Ok(Self {
            directory,
            id,
            _lock: lock,
            sender,
            errors,
            pending,
            worker: Some(worker),
        })
    }
    pub fn candidates(&self) -> Vec<Candidate> {
        let mut result = Vec::new();
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return result;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json")
                || path.file_stem().and_then(|s| s.to_str()) == Some(&self.id.to_string())
            {
                continue;
            }
            let Ok(lock) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path.with_extension("lock"))
            else {
                continue;
            };
            if lock.try_lock().is_err() {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            let mut bytes = Vec::new();
            if file
                .take(MAX_SNAPSHOT as u64 + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() > MAX_SNAPSHOT
            {
                continue;
            }
            let Ok(snapshot) = serde_json::from_slice::<Snapshot>(&bytes) else {
                continue;
            };
            if storage::ensure_saveable(&snapshot.project).is_err() {
                continue;
            }
            result.push(Candidate {
                snapshot,
                path,
                _lock: lock,
            });
        }
        result.sort_by_key(|c| std::cmp::Reverse(c.snapshot.timestamp));
        result
    }
    pub fn save(&self, project: Project, source: Option<PathBuf>) -> Result<(), SnapshotError> {
        self.pending.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .send(Command::Save(Snapshot::new(project, source)))
            .is_err()
        {
            self.pending.fetch_sub(1, Ordering::Release);
            return Err(SnapshotError::WorkerStopped);
        }
        Ok(())
    }
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) != 0
    }
    pub fn clear(&self) -> Result<(), RecoveryError> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(Command::Clear(sender))
            .map_err(|_| RecoveryError::WorkerStopped)?;
        receiver.recv().map_err(|_| RecoveryError::WorkerStopped)?
    }
    pub fn error(&self) -> Option<SnapshotError> {
        self.errors.try_recv().ok()
    }
}
impl Drop for Recovery {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_sessions_are_hidden_and_abandoned_copies_are_independent() {
        let dir = std::env::temp_dir().join(format!("animapro-recovery-test-{}", Uuid::new_v4()));
        let first = Recovery::in_directory(dir.clone()).unwrap();
        let second = Recovery::in_directory(dir.clone()).unwrap();
        let mut project = Project::new();
        project.name = "first".into();
        first
            .save(project.clone(), Some(PathBuf::from("first.animapro")))
            .unwrap();
        let path = dir.join(format!("{}.json", first.id));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !path.exists() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(second.candidates().is_empty());
        project.name = "second".into();
        second.save(project, None).unwrap();
        drop(second);
        let found = first.candidates();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].snapshot.project.name, "second");
        drop(found);
        drop(first);
        let third = Recovery::in_directory(dir.clone()).unwrap();
        let mut found = third.candidates();
        assert_eq!(found.len(), 2);
        let candidate = found.pop().unwrap();
        candidate.discard().unwrap();
        drop(found);
        assert_eq!(third.candidates().len(), 1);
        drop(third);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn recovery_metadata_does_not_block_a_project_near_the_json_limit() {
        let dir = std::env::temp_dir().join(format!("animapro-recovery-limit-{}", Uuid::new_v4()));
        let recovery = Recovery::in_directory(dir.clone()).unwrap();
        let mut project = Project::new();
        let base = serde_json::to_vec(&project).unwrap().len() - project.name.len();
        project.name = "x".repeat(storage::MAX_JSON as usize - base - 64);
        storage::ensure_saveable(&project).unwrap();
        let snapshot = Snapshot::new(project, Some(PathBuf::from("p".repeat(1024))));
        assert!(serde_json::to_vec(&snapshot).unwrap().len() > storage::MAX_JSON as usize);
        write_snapshot(&dir.join(format!("{}.json", recovery.id)), &snapshot).unwrap();
        drop(snapshot);
        drop(recovery);
        let next = Recovery::in_directory(dir.clone()).unwrap();
        assert_eq!(next.candidates().len(), 1);
        drop(next);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn clear_is_ordered_after_pending_writes() {
        let dir = std::env::temp_dir().join(format!("animapro-recovery-test-{}", Uuid::new_v4()));
        let recovery = Recovery::in_directory(dir.clone()).unwrap();
        recovery.save(Project::new(), None).unwrap();
        recovery.clear().unwrap();
        assert!(!dir.join(format!("{}.json", recovery.id)).exists());
        drop(recovery);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn stopped_worker_reports_that_snapshot_was_not_queued() {
        let dir = std::env::temp_dir().join(format!("animapro-recovery-test-{}", Uuid::new_v4()));
        let mut recovery = Recovery::in_directory(dir.clone()).unwrap();
        recovery.sender.send(Command::Stop).unwrap();
        recovery.worker.take().unwrap().join().unwrap();
        assert!(matches!(
            recovery.save(Project::new(), None),
            Err(SnapshotError::WorkerStopped)
        ));
        assert!(matches!(
            recovery.clear(),
            Err(RecoveryError::WorkerStopped)
        ));
        drop(recovery);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_recovery_directory_reports_io_error() {
        let path = std::env::temp_dir().join(format!("animapro-recovery-file-{}", Uuid::new_v4()));
        fs::write(&path, b"keep").unwrap();
        assert!(matches!(
            Recovery::in_directory(path.clone()),
            Err(RecoveryError::Io(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn failed_background_snapshot_reports_error_and_preserves_existing_data() {
        let dir = std::env::temp_dir().join(format!("animapro-recovery-test-{}", Uuid::new_v4()));
        let recovery = Recovery::in_directory(dir.clone()).unwrap();
        let destination = dir.join(format!("{}.json", recovery.id));
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), b"original").unwrap();
        recovery.save(Project::new(), None).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while recovery.is_pending() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(recovery.error(), Some(SnapshotError::Io(_))));
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"original");
        assert!(!fs::read_dir(&dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "tmp")
        }));
        drop(recovery);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn dates_are_readable() {
        assert_eq!(format_time(0), "01.01.1970 00:00:00");
        assert_eq!(format_time(1_704_067_200), "01.01.2024 00:00:00");
    }
}
