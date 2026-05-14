use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use anyhow::Context;

use crate::db::Database;

const BATCH_SIZE: usize = 2_048;
const MAX_BALANCED_WORKERS: usize = 8;
const DIR_QUEUE_MULTIPLIER: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanMode {
    NtfsUsn,
    Recursive,
}

impl ScanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NtfsUsn => "ntfs_usn",
            Self::Recursive => "recursive",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanPlan {
    pub mode: ScanMode,
    pub attempted_mode: Option<ScanMode>,
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileAttributeFlags {
    pub bits: u32,
}

impl FileAttributeFlags {
    pub const DIRECTORY: Self = Self { bits: 0x10 };
    pub const OFFLINE: Self = Self { bits: 0x1000 };
    pub const REPARSE_POINT: Self = Self { bits: 0x400 };
    pub const SYSTEM: Self = Self { bits: 0x4 };
    pub const TEMPORARY: Self = Self { bits: 0x100 };
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileAttributeClassification {
    pub is_reparse_point: bool,
    pub is_placeholder: bool,
    pub is_system: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Idle,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub state: ScanState,
    pub run_id: Option<i64>,
    pub root_path: Option<String>,
    pub mode: Option<ScanMode>,
    pub attempted_mode: Option<ScanMode>,
    pub fallback_reason: Option<String>,
    pub files_indexed: u64,
    pub directories_indexed: u64,
    pub bytes_indexed: u64,
    pub access_denied_count: u64,
    pub skipped_reparse_count: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
    pub worker_count: Option<usize>,
    pub phase: Option<String>,
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self {
            state: ScanState::Idle,
            run_id: None,
            root_path: None,
            mode: None,
            attempted_mode: None,
            fallback_reason: None,
            files_indexed: 0,
            directories_indexed: 0,
            bytes_indexed: 0,
            access_denied_count: 0,
            skipped_reparse_count: 0,
            error_count: 0,
            last_error: None,
            worker_count: None,
            phase: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScanRunOptions {
    pub file_system: Option<String>,
    pub attempted_mode: Option<ScanMode>,
    pub fallback_reason: Option<String>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub progress: Option<Arc<Mutex<ScanProgress>>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub run_id: i64,
    pub root_path: String,
    pub mode: ScanMode,
    pub attempted_mode: Option<ScanMode>,
    pub fallback_reason: Option<String>,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub files_indexed: u64,
    pub directories_indexed: u64,
    pub bytes_indexed: u64,
    pub access_denied_count: u64,
    pub skipped_reparse_count: u64,
    pub error_count: u64,
    pub canceled: bool,
}

#[derive(Clone, Debug)]
pub struct IndexedEntry {
    pub path: String,
    pub parent_path: Option<String>,
    pub name: String,
    pub extension: Option<String>,
    pub is_directory: bool,
    pub size: u64,
    pub modified_at_ms: Option<i64>,
    pub created_at_ms: Option<i64>,
    pub accessed_at_ms: Option<i64>,
    pub attributes: u32,
    pub is_reparse_point: bool,
    pub is_placeholder: bool,
    pub is_system: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DirectoryRollup {
    pub total_size: u64,
    pub file_count: u64,
    pub directory_count: u64,
}

#[derive(Clone, Debug)]
struct ScannedEntry {
    path: PathBuf,
    indexed: IndexedEntry,
    is_file: bool,
    is_directory: bool,
    size: u64,
}

#[derive(Debug)]
enum ScanEvent {
    DirectoryScanned {
        dir: PathBuf,
        entries: Vec<ScannedEntry>,
        child_dirs: Vec<PathBuf>,
        access_denied_count: u64,
        error_count: u64,
        skipped_reparse_count: u64,
    },
    DirectoryReadError {
        dir: PathBuf,
        kind: io::ErrorKind,
        message: String,
    },
}

pub fn balanced_worker_count(logical_cores: usize) -> usize {
    logical_cores
        .saturating_sub(1)
        .max(1)
        .min(MAX_BALANCED_WORKERS)
}

pub fn classify_file_attributes(attributes: u32) -> FileAttributeClassification {
    let is_reparse_point = attributes & FileAttributeFlags::REPARSE_POINT.bits != 0;
    let is_offline = attributes & FileAttributeFlags::OFFLINE.bits != 0;
    let is_system = attributes & FileAttributeFlags::SYSTEM.bits != 0;

    FileAttributeClassification {
        is_reparse_point,
        is_placeholder: is_reparse_point || is_offline,
        is_system,
    }
}

pub fn choose_scan_plan(_root_path: &str, file_system: Option<&str>) -> ScanPlan {
    if file_system
        .map(|value| value.eq_ignore_ascii_case("NTFS"))
        .unwrap_or(false)
    {
        return ScanPlan {
            mode: ScanMode::Recursive,
            attempted_mode: Some(ScanMode::NtfsUsn),
            fallback_reason: Some(
                "USN journal provider is reserved for incremental NTFS refresh; milestone 1 uses recursive fallback after capability detection.".to_string(),
            ),
        };
    }

    ScanPlan {
        mode: ScanMode::Recursive,
        attempted_mode: None,
        fallback_reason: file_system
            .map(|fs| format!("{fs} is not NTFS; using recursive traversal for this location.")),
    }
}

pub fn run_recursive_scan(
    db: &Database,
    root: &Path,
    options: Option<ScanRunOptions>,
) -> anyhow::Result<ScanSummary> {
    let options = options.unwrap_or_else(|| ScanRunOptions {
        file_system: None,
        attempted_mode: None,
        fallback_reason: None,
        cancel: None,
        progress: None,
    });

    let root = root.to_path_buf();
    let root_path = root.to_string_lossy().to_string();
    let worker_count = balanced_worker_count(
        thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
    );
    let started_at_ms = unix_millis(SystemTime::now());
    let run_id = db.create_scan_run(
        &root_path,
        options.file_system.as_deref(),
        ScanMode::Recursive,
        options.attempted_mode,
        options.fallback_reason.as_deref(),
        started_at_ms,
    )?;

    let mut summary = ScanSummary {
        run_id,
        root_path: root_path.clone(),
        mode: ScanMode::Recursive,
        attempted_mode: options.attempted_mode,
        fallback_reason: options.fallback_reason.clone(),
        started_at_ms,
        finished_at_ms: started_at_ms,
        files_indexed: 0,
        directories_indexed: 0,
        bytes_indexed: 0,
        access_denied_count: 0,
        skipped_reparse_count: 0,
        error_count: 0,
        canceled: false,
    };

    update_progress(
        &options.progress,
        &summary,
        ScanState::Running,
        None,
        Some(worker_count),
        Some("scanning"),
    );

    match collect_parallel_scan(db, run_id, &root, &options, worker_count, &mut summary) {
        Ok(rollups) => {
            summary.finished_at_ms = unix_millis(SystemTime::now());
            let status = if summary.canceled {
                "canceled"
            } else {
                "completed"
            };
            if let Err(error) = db
                .replace_directory_rollups(run_id, &rollups)
                .and_then(|_| db.finish_scan_run(&summary, status, None))
            {
                let last_error = error.to_string();
                let _ = db.finish_scan_run(&summary, "failed", Some(&last_error));
                update_progress(
                    &options.progress,
                    &summary,
                    ScanState::Failed,
                    Some(last_error),
                    Some(worker_count),
                    Some("failed"),
                );
                return Err(error);
            }
            update_progress(
                &options.progress,
                &summary,
                if summary.canceled {
                    ScanState::Canceled
                } else {
                    ScanState::Completed
                },
                None,
                Some(worker_count),
                Some(if summary.canceled {
                    "canceled"
                } else {
                    "complete"
                }),
            );
            Ok(summary)
        }
        Err(error) => {
            let last_error = error.to_string();
            summary.finished_at_ms = unix_millis(SystemTime::now());
            db.finish_scan_run(&summary, "failed", Some(&last_error))?;
            update_progress(
                &options.progress,
                &summary,
                ScanState::Failed,
                Some(last_error),
                Some(worker_count),
                Some("failed"),
            );
            Err(error)
        }
    }
}

fn collect_parallel_scan(
    db: &Database,
    run_id: i64,
    root: &Path,
    options: &ScanRunOptions,
    worker_count: usize,
    summary: &mut ScanSummary,
) -> anyhow::Result<HashMap<PathBuf, DirectoryRollup>> {
    let queue_capacity = worker_count.max(1) * DIR_QUEUE_MULTIPLIER;
    let (dir_sender, dir_receiver) = mpsc::sync_channel::<PathBuf>(queue_capacity);
    let (event_sender, event_receiver) = mpsc::channel::<ScanEvent>();
    let dir_receiver = Arc::new(Mutex::new(dir_receiver));
    let cancel = options
        .cancel
        .clone()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let mut handles = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let dir_receiver = Arc::clone(&dir_receiver);
        let event_sender = event_sender.clone();
        let cancel = Arc::clone(&cancel);
        handles.push(thread::spawn(move || {
            scan_worker(dir_receiver, event_sender, cancel);
        }));
    }
    drop(event_sender);

    let mut dir_sender = Some(dir_sender);
    dir_sender
        .as_ref()
        .context("directory queue is unavailable")?
        .send(root.to_path_buf())
        .context("queue scan root")?;

    let mut active_dirs = 1usize;
    let mut pending_entries = Vec::with_capacity(BATCH_SIZE);
    let mut rollups: HashMap<PathBuf, DirectoryRollup> = HashMap::new();
    let mut scan_error: Option<anyhow::Error> = None;

    while active_dirs > 0 {
        let event = match event_receiver.recv() {
            Ok(event) => event,
            Err(error) => {
                scan_error = Some(anyhow::anyhow!(
                    "scanner workers stopped unexpectedly: {error}"
                ));
                cancel.store(true, Ordering::Relaxed);
                dir_sender.take();
                break;
            }
        };
        active_dirs -= 1;

        if is_canceled(Some(cancel.as_ref())) {
            summary.canceled = true;
            dir_sender.take();
        }

        match event {
            ScanEvent::DirectoryReadError { dir, kind, message } => {
                record_io_error_kind(summary, kind);
                if dir == root && scan_error.is_none() {
                    scan_error = Some(anyhow::anyhow!("scan root is not readable: {message}"));
                    cancel.store(true, Ordering::Relaxed);
                    dir_sender.take();
                }
            }
            ScanEvent::DirectoryScanned {
                dir: _dir,
                entries,
                child_dirs,
                access_denied_count,
                error_count,
                skipped_reparse_count,
            } => {
                summary.access_denied_count += access_denied_count;
                summary.error_count += error_count;
                summary.skipped_reparse_count += skipped_reparse_count;

                if scan_error.is_none() {
                    for scanned in entries {
                        if scanned.is_file {
                            summary.files_indexed += 1;
                            summary.bytes_indexed += scanned.size;
                            add_file_to_rollups(&mut rollups, root, &scanned.path, scanned.size);
                        } else if scanned.is_directory {
                            summary.directories_indexed += 1;
                            add_directory_to_rollups(&mut rollups, root, &scanned.path);
                        }

                        pending_entries.push(scanned.indexed);
                        if pending_entries.len() >= BATCH_SIZE {
                            if let Err(error) = db.insert_file_entries(run_id, &pending_entries) {
                                scan_error = Some(error);
                                cancel.store(true, Ordering::Relaxed);
                                dir_sender.take();
                                break;
                            }
                            pending_entries.clear();
                            update_progress(
                                &options.progress,
                                summary,
                                ScanState::Running,
                                None,
                                Some(worker_count),
                                Some("indexing"),
                            );
                        }
                    }
                }

                if !summary.canceled && scan_error.is_none() {
                    if let Some(sender) = dir_sender.as_ref() {
                        for child_dir in child_dirs {
                            if sender.send(child_dir).is_err() {
                                scan_error = Some(anyhow::anyhow!(
                                    "scanner workers stopped while queuing directories"
                                ));
                                cancel.store(true, Ordering::Relaxed);
                                dir_sender.take();
                                break;
                            }
                            active_dirs += 1;
                        }
                    }
                }
            }
        }

        update_progress(
            &options.progress,
            summary,
            ScanState::Running,
            None,
            Some(worker_count),
            Some(if summary.canceled {
                "canceling"
            } else {
                "scanning"
            }),
        );
    }

    drop(dir_sender);
    for handle in handles {
        if handle.join().is_err() && scan_error.is_none() {
            scan_error = Some(anyhow::anyhow!("scanner worker panicked"));
        }
    }

    if let Some(error) = scan_error {
        return Err(error);
    }

    if !pending_entries.is_empty() {
        db.insert_file_entries(run_id, &pending_entries)?;
    }

    Ok(rollups)
}

fn scan_worker(
    dir_receiver: Arc<Mutex<mpsc::Receiver<PathBuf>>>,
    event_sender: mpsc::Sender<ScanEvent>,
    cancel: Arc<AtomicBool>,
) {
    loop {
        let dir = {
            let receiver = match dir_receiver.lock() {
                Ok(receiver) => receiver,
                Err(_) => return,
            };
            receiver.recv()
        };

        let Ok(dir) = dir else {
            return;
        };

        let event = scan_directory(&dir, &cancel);
        if event_sender.send(event).is_err() {
            return;
        }
    }
}

fn scan_directory(dir: &Path, cancel: &AtomicBool) -> ScanEvent {
    if is_canceled(Some(cancel)) {
        return ScanEvent::DirectoryScanned {
            dir: dir.to_path_buf(),
            entries: Vec::new(),
            child_dirs: Vec::new(),
            access_denied_count: 0,
            error_count: 0,
            skipped_reparse_count: 0,
        };
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            return ScanEvent::DirectoryReadError {
                dir: dir.to_path_buf(),
                kind: error.kind(),
                message: error.to_string(),
            }
        }
    };

    let mut scanned_entries = Vec::new();
    let mut child_dirs = Vec::new();
    let mut access_denied_count = 0;
    let mut error_count = 0;
    let mut skipped_reparse_count = 0;

    for entry_result in entries {
        if is_canceled(Some(cancel)) {
            break;
        }

        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                record_io_error_counts(&mut access_denied_count, &mut error_count, error.kind());
                continue;
            }
        };

        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                record_io_error_counts(&mut access_denied_count, &mut error_count, error.kind());
                continue;
            }
        };

        let attributes = file_attributes(&metadata);
        let classification = classify_file_attributes(attributes);
        let is_directory = metadata.is_dir();
        let is_file = metadata.is_file();
        let size = if is_file { metadata.len() } else { 0 };

        if is_directory && classification.is_reparse_point {
            skipped_reparse_count += 1;
        } else if is_directory {
            child_dirs.push(path.clone());
        }

        scanned_entries.push(ScannedEntry {
            indexed: indexed_entry(
                &path,
                is_directory,
                size,
                attributes,
                &classification,
                &metadata,
            ),
            path,
            is_file,
            is_directory,
            size,
        });
    }

    ScanEvent::DirectoryScanned {
        dir: dir.to_path_buf(),
        entries: scanned_entries,
        child_dirs,
        access_denied_count,
        error_count,
        skipped_reparse_count,
    }
}

fn indexed_entry(
    path: &Path,
    is_directory: bool,
    size: u64,
    attributes: u32,
    classification: &FileAttributeClassification,
    metadata: &fs::Metadata,
) -> IndexedEntry {
    IndexedEntry {
        path: path.to_string_lossy().to_string(),
        parent_path: path
            .parent()
            .map(|parent| parent.to_string_lossy().to_string()),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string()),
        extension: path
            .extension()
            .map(|ext| ext.to_string_lossy().to_string()),
        is_directory,
        size,
        modified_at_ms: metadata.modified().ok().map(unix_millis),
        created_at_ms: metadata.created().ok().map(unix_millis),
        accessed_at_ms: metadata.accessed().ok().map(unix_millis),
        attributes,
        is_reparse_point: classification.is_reparse_point,
        is_placeholder: classification.is_placeholder,
        is_system: classification.is_system,
    }
}

fn add_file_to_rollups(
    rollups: &mut HashMap<PathBuf, DirectoryRollup>,
    root: &Path,
    file_path: &Path,
    size: u64,
) {
    let mut current = file_path.parent();
    while let Some(path) = current {
        if !path.starts_with(root) {
            break;
        }

        let rollup = rollups.entry(path.to_path_buf()).or_default();
        rollup.total_size += size;
        rollup.file_count += 1;

        if path == root {
            break;
        }
        current = path.parent();
    }
}

fn add_directory_to_rollups(
    rollups: &mut HashMap<PathBuf, DirectoryRollup>,
    root: &Path,
    dir: &Path,
) {
    let mut current = Some(dir);
    while let Some(path) = current {
        if !path.starts_with(root) {
            break;
        }

        rollups
            .entry(path.to_path_buf())
            .or_default()
            .directory_count += 1;
        if path == root {
            break;
        }
        current = path.parent();
    }
}

fn record_io_error_kind(summary: &mut ScanSummary, kind: io::ErrorKind) {
    if kind == io::ErrorKind::PermissionDenied {
        summary.access_denied_count += 1;
    } else {
        summary.error_count += 1;
    }
}

fn record_io_error_counts(
    access_denied_count: &mut u64,
    error_count: &mut u64,
    kind: io::ErrorKind,
) {
    if kind == io::ErrorKind::PermissionDenied {
        *access_denied_count += 1;
    } else {
        *error_count += 1;
    }
}

fn update_progress(
    progress: &Option<Arc<Mutex<ScanProgress>>>,
    summary: &ScanSummary,
    state: ScanState,
    last_error: Option<String>,
    worker_count: Option<usize>,
    phase: Option<&str>,
) {
    let Some(progress) = progress else {
        return;
    };

    if let Ok(mut progress) = progress.lock() {
        *progress = ScanProgress {
            state,
            run_id: Some(summary.run_id),
            root_path: Some(summary.root_path.clone()),
            mode: Some(summary.mode),
            attempted_mode: summary.attempted_mode,
            fallback_reason: summary.fallback_reason.clone(),
            files_indexed: summary.files_indexed,
            directories_indexed: summary.directories_indexed,
            bytes_indexed: summary.bytes_indexed,
            access_denied_count: summary.access_denied_count,
            skipped_reparse_count: summary.skipped_reparse_count,
            error_count: summary.error_count,
            last_error,
            worker_count,
            phase: phase.map(str::to_string),
        };
    }
}

fn is_canceled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|cancel| cancel.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn unix_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(windows)]
fn file_attributes(metadata: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes()
}

#[cfg(not(windows))]
fn file_attributes(_metadata: &fs::Metadata) -> u32 {
    0
}
