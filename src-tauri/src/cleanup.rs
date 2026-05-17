use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    db::{CleanupActionInsert, CleanupActionRecord, CleanupFileRecord, Database},
    scan::classify_file_attributes,
};

const CLEANUP_MIN_AGE_MS: i64 = 24 * 60 * 60 * 1000;
const CLEANUP_METADATA_TOLERANCE_MS: i64 = 2_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPlan {
    pub scan_run_id: i64,
    pub candidates: Vec<CleanupCandidate>,
    pub total_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupCandidate {
    pub id: String,
    pub entry_id: i64,
    pub scan_run_id: i64,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub reason: String,
    pub risk_level: CleanupRisk,
    pub action_kind: String,
    pub modified_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupRisk {
    Low,
    Review,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupExecutionResult {
    pub actions: Vec<CleanupActionRecord>,
    pub recycled_count: u64,
    pub failed_count: u64,
    pub skipped_count: u64,
    pub bytes_requested: u64,
}

pub trait FileRecycler {
    fn recycle_file(&self, path: &Path) -> anyhow::Result<()>;

    fn recycle_files(&self, paths: &[PathBuf]) -> Vec<anyhow::Result<()>> {
        paths.iter().map(|path| self.recycle_file(path)).collect()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRecycleBin;

impl FileRecycler for SystemRecycleBin {
    fn recycle_file(&self, path: &Path) -> anyhow::Result<()> {
        recycle_file(path)
    }

    fn recycle_files(&self, paths: &[PathBuf]) -> Vec<anyhow::Result<()>> {
        recycle_files(paths)
    }
}

pub fn cleanup_candidate_id(scan_run_id: i64, entry_id: i64) -> String {
    format!("scan-{scan_run_id}:entry-{entry_id}")
}

pub fn build_cleanup_plan(
    db: &Database,
    scan_run_id: i64,
    limit: u32,
) -> anyhow::Result<CleanupPlan> {
    let records = db.cleanup_file_records(scan_run_id, 5000)?;
    let now_ms = unix_millis();
    let mut candidates = records
        .into_iter()
        .filter_map(|record| cleanup_candidate(scan_run_id, record, now_ms))
        .take(limit.clamp(1, 500) as usize)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| left.path.cmp(&right.path))
    });
    let total_bytes = candidates.iter().map(|candidate| candidate.size).sum();
    Ok(CleanupPlan {
        scan_run_id,
        candidates,
        total_bytes,
        warnings: vec![
            "WinClean only proposes low-risk temporary, log, dump, backup, and cache-like files in this version.".to_string(),
            "Cleanup uses the Windows Recycle Bin by default; no permanent delete action is exposed.".to_string(),
        ],
    })
}

pub fn execute_cleanup_plan(
    db: &Database,
    scan_run_id: i64,
    candidate_ids: &[String],
) -> anyhow::Result<CleanupExecutionResult> {
    execute_cleanup_plan_with_recycler(db, scan_run_id, candidate_ids, &SystemRecycleBin)
}

pub fn execute_cleanup_plan_with_recycler(
    db: &Database,
    scan_run_id: i64,
    candidate_ids: &[String],
    recycler: &dyn FileRecycler,
) -> anyhow::Result<CleanupExecutionResult> {
    let candidates = resolve_selected_cleanup_candidates(db, scan_run_id, candidate_ids)?;

    let mut actions = Vec::new();
    let mut pending_recycles = Vec::new();
    let mut recycled_count = 0;
    let mut failed_count = 0;
    let mut skipped_count = 0;
    let mut bytes_requested = 0;

    for candidate in candidates {
        bytes_requested += candidate.size;
        let created_at_ms = unix_millis();
        let action_id = db.record_cleanup_action(CleanupActionInsert {
            action_type: "recycle_bin".to_string(),
            status: "running".to_string(),
            target_path: candidate.path.clone(),
            original_path: Some(candidate.path.clone()),
            destination_path: Some("Windows Recycle Bin".to_string()),
            scan_run_id: Some(scan_run_id),
            explanation: Some(candidate.reason.clone()),
            created_at_ms,
            completed_at_ms: None,
            undo_token: None,
        })?;

        match validate_file_for_action(&candidate.path, candidate.size, candidate.modified_at_ms) {
            Ok(path) => pending_recycles.push(PendingRecycle { action_id, path }),
            Err(error) => {
                skipped_count += 1;
                db.update_cleanup_action(
                    action_id,
                    "skipped",
                    Some(unix_millis()),
                    Some(&error.to_string()),
                    None,
                )?;
                if let Some(action) = db.cleanup_action(action_id)? {
                    actions.push(action);
                }
                continue;
            }
        }
    }

    let recycle_paths = pending_recycles
        .iter()
        .map(|pending| pending.path.clone())
        .collect::<Vec<_>>();
    let mut recycle_results = recycler.recycle_files(&recycle_paths).into_iter();

    for pending in pending_recycles {
        let recycle_result = recycle_results.next().unwrap_or_else(|| {
            Err(anyhow::anyhow!(
                "Recycle Bin worker did not return a result for {}",
                pending.path.display()
            ))
        });
        match recycle_result {
            Ok(()) => {
                recycled_count += 1;
                db.update_cleanup_action(
                    pending.action_id,
                    "recycled",
                    Some(unix_millis()),
                    None,
                    Some("windows-recycle-bin"),
                )?;
            }
            Err(error) => {
                failed_count += 1;
                db.update_cleanup_action(
                    pending.action_id,
                    "failed",
                    Some(unix_millis()),
                    Some(&error.to_string()),
                    None,
                )?;
            }
        }

        if let Some(action) = db.cleanup_action(pending.action_id)? {
            actions.push(action);
        }
    }

    Ok(CleanupExecutionResult {
        actions,
        recycled_count,
        failed_count,
        skipped_count,
        bytes_requested,
    })
}

struct PendingRecycle {
    action_id: i64,
    path: PathBuf,
}

pub fn resolve_cleanup_candidates(
    candidates: &[CleanupCandidate],
    candidate_ids: &[String],
) -> anyhow::Result<Vec<CleanupCandidate>> {
    let candidates_by_id = candidates
        .iter()
        .map(|candidate| (candidate.id.as_str(), candidate))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();

    for candidate_id in candidate_ids {
        if !seen.insert(candidate_id.as_str()) {
            continue;
        }

        let Some(candidate) = candidates_by_id.get(candidate_id.as_str()) else {
            anyhow::bail!("cleanup candidate {candidate_id} is no longer available");
        };
        resolved.push((*candidate).clone());
    }

    Ok(resolved)
}

fn resolve_selected_cleanup_candidates(
    db: &Database,
    scan_run_id: i64,
    candidate_ids: &[String],
) -> anyhow::Result<Vec<CleanupCandidate>> {
    let mut seen = HashSet::new();
    let mut requested = Vec::new();
    for candidate_id in candidate_ids {
        if !seen.insert(candidate_id.as_str()) {
            continue;
        }
        let entry_id = cleanup_entry_id_from_candidate_id(scan_run_id, candidate_id)?;
        requested.push((candidate_id.clone(), entry_id));
    }

    let entry_ids = requested
        .iter()
        .map(|(_, entry_id)| *entry_id)
        .collect::<Vec<_>>();
    let now_ms = unix_millis();
    let candidates_by_entry_id = db
        .cleanup_file_records_by_ids(scan_run_id, &entry_ids)?
        .into_iter()
        .filter_map(|record| cleanup_candidate(scan_run_id, record, now_ms))
        .map(|candidate| (candidate.entry_id, candidate))
        .collect::<HashMap<_, _>>();

    let mut resolved = Vec::new();
    for (candidate_id, entry_id) in requested {
        let Some(candidate) = candidates_by_entry_id.get(&entry_id) else {
            anyhow::bail!("cleanup candidate {candidate_id} is no longer available");
        };
        resolved.push(candidate.clone());
    }

    Ok(resolved)
}

fn cleanup_entry_id_from_candidate_id(scan_run_id: i64, candidate_id: &str) -> anyhow::Result<i64> {
    let prefix = format!("scan-{scan_run_id}:entry-");
    let Some(raw_entry_id) = candidate_id.strip_prefix(&prefix) else {
        anyhow::bail!("cleanup candidate {candidate_id} is no longer available");
    };
    let entry_id = raw_entry_id.parse::<i64>().ok().filter(|value| *value > 0);
    entry_id
        .ok_or_else(|| anyhow::anyhow!("cleanup candidate {candidate_id} is no longer available"))
}

pub fn validate_cleanup_target(candidate: &CleanupCandidate) -> anyhow::Result<PathBuf> {
    if is_protected_path(&candidate.path) {
        anyhow::bail!("cleanup target is protected: {}", candidate.path);
    }
    validate_cleanup_age(candidate.modified_at_ms, unix_millis())?;

    let original = Path::new(&candidate.path);
    let metadata = std::fs::symlink_metadata(original).map_err(|error| {
        anyhow::anyhow!(
            "cleanup target is no longer available: {}: {error}",
            original.display()
        )
    })?;
    reject_live_cleanup_metadata(candidate, original, &metadata)?;

    let canonical = std::fs::canonicalize(original)
        .map_err(|error| anyhow::anyhow!("cannot recycle {}: {error}", original.display()))?;
    let canonical_display = canonical.to_string_lossy();
    if is_protected_path(&canonical_display) {
        anyhow::bail!("cleanup target resolves to a protected path: {canonical_display}");
    }

    let canonical_metadata = std::fs::symlink_metadata(&canonical).map_err(|error| {
        anyhow::anyhow!(
            "cannot inspect resolved target {}: {error}",
            canonical.display()
        )
    })?;
    reject_live_cleanup_metadata(candidate, &canonical, &canonical_metadata)?;

    Ok(canonical)
}

fn reject_live_cleanup_metadata(
    candidate: &CleanupCandidate,
    path: &Path,
    metadata: &std::fs::Metadata,
) -> anyhow::Result<()> {
    if !metadata.is_file() {
        anyhow::bail!(
            "cleanup only recycles files in this version: {}",
            path.display()
        );
    }

    let classification = classify_file_attributes(file_attributes(metadata));
    if classification.is_reparse_point
        || classification.is_placeholder
        || classification.is_system
        || metadata.file_type().is_symlink()
    {
        anyhow::bail!(
            "cleanup target is not eligible because it is protected, system, or reparse-backed: {}",
            path.display()
        );
    }

    if metadata.len() != candidate.size {
        anyhow::bail!("cleanup target changed since scan: {}", path.display());
    }
    let current_modified_at_ms = metadata
        .modified()
        .ok()
        .map(unix_millis_from_system_time)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cleanup target modified time is unavailable: {}",
                path.display()
            )
        })?;
    validate_cleanup_age(Some(current_modified_at_ms), unix_millis())?;
    let scanned_modified_at_ms = candidate.modified_at_ms.ok_or_else(|| {
        anyhow::anyhow!(
            "cleanup target modified time was unavailable during scan: {}",
            path.display()
        )
    })?;
    if (current_modified_at_ms - scanned_modified_at_ms).abs() > CLEANUP_METADATA_TOLERANCE_MS {
        anyhow::bail!("cleanup target changed since scan: {}", path.display());
    }

    Ok(())
}

fn cleanup_candidate(
    scan_run_id: i64,
    record: CleanupFileRecord,
    now_ms: i64,
) -> Option<CleanupCandidate> {
    if record.is_reparse_point || record.is_placeholder || record.is_system {
        return None;
    }

    if is_protected_path(&record.path) {
        return None;
    }

    if validate_cleanup_age(record.modified_at_ms, now_ms).is_err() {
        return None;
    }

    let (reason, risk_level) = cleanup_reason(&record)?;
    Some(CleanupCandidate {
        id: cleanup_candidate_id(scan_run_id, record.id),
        entry_id: record.id,
        scan_run_id,
        path: record.path,
        name: record.name,
        size: record.size,
        reason,
        risk_level,
        action_kind: "recycle_bin".to_string(),
        modified_at_ms: record.modified_at_ms,
    })
}

pub fn cleanup_reason(record: &CleanupFileRecord) -> Option<(String, CleanupRisk)> {
    let extension = record
        .extension
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = record.name.to_ascii_lowercase();

    match extension.as_str() {
        "tmp" | "temp" => Some(("Temporary file extension".to_string(), CleanupRisk::Low)),
        "log" => Some(("Log file".to_string(), CleanupRisk::Low)),
        "bak" | "old" => Some(("Backup artifact".to_string(), CleanupRisk::Review)),
        "dmp" => Some(("Crash dump file".to_string(), CleanupRisk::Review)),
        _ if name == "thumbs.db" => Some(("Windows thumbnail cache".to_string(), CleanupRisk::Low)),
        _ => None,
    }
}

pub fn is_protected_path(path: &str) -> bool {
    let normalized = normalize_for_rules(path);
    let root_like = normalized.ends_with(":\\")
        || normalized.matches('\\').count() <= 1
        || PathBuf::from(path).parent().is_none();

    root_like
        || normalized.contains("\\windows\\")
        || normalized.contains("\\program files\\")
        || normalized.contains("\\program files (x86)\\")
        || normalized.contains("\\programdata\\")
        || normalized.contains("\\appdata\\")
        || normalized.contains("\\appdata\\local\\microsoft\\windows\\")
}

fn normalize_for_rules(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

pub fn validate_file_for_action(
    path: &str,
    expected_size: u64,
    expected_modified_at_ms: Option<i64>,
) -> anyhow::Result<PathBuf> {
    let name = Path::new(path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let candidate = CleanupCandidate {
        id: String::new(),
        entry_id: 0,
        scan_run_id: 0,
        path: path.to_string(),
        name,
        size: expected_size,
        reason: String::new(),
        risk_level: CleanupRisk::Review,
        action_kind: String::new(),
        modified_at_ms: expected_modified_at_ms,
    };
    let canonical = validate_cleanup_target(&candidate)?;
    let metadata = std::fs::symlink_metadata(&canonical).map_err(|error| {
        anyhow::anyhow!(
            "cannot inspect resolved target {}: {error}",
            canonical.display()
        )
    })?;
    if metadata.len() != expected_size {
        anyhow::bail!(
            "file changed since scan: expected {} bytes, found {} bytes",
            expected_size,
            metadata.len()
        );
    }
    if let (Some(expected), Ok(modified)) = (expected_modified_at_ms, metadata.modified()) {
        let actual = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        if (actual - expected).abs() > 1_000 {
            anyhow::bail!("file timestamp changed since scan");
        }
    }
    Ok(canonical)
}

pub fn recycle_file(path: &Path) -> anyhow::Result<()> {
    move_to_recycle_bin(path)
}

pub fn recycle_files(paths: &[PathBuf]) -> Vec<anyhow::Result<()>> {
    move_to_recycle_bin_batch(paths)
}

#[cfg(windows)]
fn move_to_recycle_bin(path: &Path) -> anyhow::Result<()> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| anyhow::anyhow!("cannot recycle {}: {error}", path.display()))?;
    if !canonical.is_file() {
        anyhow::bail!(
            "cleanup only recycles files in this version: {}",
            path.display()
        );
    }

    run_recycle_operation_on_worker(move || {
        trash::delete(&canonical).map_err(|error| {
            anyhow::anyhow!(
                "failed to move {} to the Windows Recycle Bin: {error}",
                canonical.display()
            )
        })
    })
}

#[cfg(windows)]
fn move_to_recycle_bin_batch(paths: &[PathBuf]) -> Vec<anyhow::Result<()>> {
    if paths.is_empty() {
        return Vec::new();
    }

    let canonical_results = paths
        .iter()
        .map(|path| {
            let canonical = std::fs::canonicalize(path)
                .map_err(|error| format!("cannot recycle {}: {error}", path.display()))?;
            if !canonical.is_file() {
                return Err(format!(
                    "cleanup only recycles files in this version: {}",
                    path.display()
                ));
            }
            Ok(canonical)
        })
        .collect::<Vec<Result<PathBuf, String>>>();
    let valid_paths = canonical_results
        .iter()
        .filter_map(|result| result.as_ref().ok().cloned())
        .collect::<Vec<_>>();

    let mut valid_results = if valid_paths.is_empty() {
        Vec::new().into_iter()
    } else {
        match run_recycle_operation_on_worker(move || {
            Ok(valid_paths
                .into_iter()
                .map(|canonical| {
                    trash::delete(&canonical).map_err(|error| {
                        format!(
                            "failed to move {} to the Windows Recycle Bin: {error}",
                            canonical.display()
                        )
                    })
                })
                .collect::<Vec<Result<(), String>>>())
        }) {
            Ok(results) => results.into_iter(),
            Err(error) => {
                let message = error.to_string();
                return canonical_results
                    .into_iter()
                    .map(|result| match result {
                        Ok(_) => Err(anyhow::anyhow!(message.clone())),
                        Err(error) => Err(anyhow::anyhow!(error)),
                    })
                    .collect();
            }
        }
    };

    canonical_results
        .into_iter()
        .map(|result| match result {
            Ok(_) => valid_results
                .next()
                .unwrap_or_else(|| Err("Recycle Bin worker did not return a result".to_string()))
                .map_err(anyhow::Error::msg),
            Err(error) => Err(anyhow::anyhow!(error)),
        })
        .collect()
}

#[cfg(windows)]
fn run_recycle_operation_on_worker<F, T>(operation: F) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let worker = std::thread::Builder::new()
        .name("winclean-recycle-bin".to_string())
        .spawn(move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)))
        .map_err(|error| anyhow::anyhow!("failed to start Recycle Bin worker: {error}"))?;
    let operation_result = worker.join().map_err(recycle_worker_panic_error)?;

    operation_result.map_err(recycle_worker_panic_error)?
}

#[cfg(windows)]
fn recycle_worker_panic_error(payload: Box<dyn std::any::Any + Send>) -> anyhow::Error {
    let message = payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".to_string());
    anyhow::anyhow!("Recycle Bin worker panicked: {message}")
}

#[cfg(not(windows))]
fn move_to_recycle_bin(path: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "Recycle Bin cleanup is only available on Windows in this build: {}",
        path.display()
    )
}

#[cfg(not(windows))]
fn move_to_recycle_bin_batch(paths: &[PathBuf]) -> Vec<anyhow::Result<()>> {
    paths.iter().map(|path| move_to_recycle_bin(path)).collect()
}

fn unix_millis() -> i64 {
    unix_millis_from_system_time(std::time::SystemTime::now())
}

fn unix_millis_from_system_time(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn validate_cleanup_age(modified_at_ms: Option<i64>, now_ms: i64) -> anyhow::Result<()> {
    let Some(modified_at_ms) = modified_at_ms else {
        anyhow::bail!("cleanup target modified time is unavailable");
    };
    if now_ms.saturating_sub(modified_at_ms) < CLEANUP_MIN_AGE_MS {
        anyhow::bail!("cleanup target was modified within the last 24 hours");
    }
    Ok(())
}

#[cfg(windows)]
fn file_attributes(metadata: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes()
}

#[cfg(not(windows))]
fn file_attributes(_metadata: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        path::{Path, PathBuf},
        sync::Mutex,
        time::{Duration, UNIX_EPOCH},
    };

    use crate::{
        db::Database,
        scan::{IndexedEntry, ScanMode},
    };

    use super::{
        build_cleanup_plan, cleanup_candidate_id, execute_cleanup_plan_with_recycler, FileRecycler,
    };

    #[test]
    fn cleanup_execution_recycles_valid_candidates_through_recycler() {
        let temp = safe_tempdir();
        let root = temp.path().join("fixture");
        fs::create_dir_all(&root).expect("fixture dir");
        let candidate_path = root.join("old.tmp");
        fs::write(&candidate_path, vec![1_u8; 20]).expect("tmp file");
        let old_modified = current_test_millis() - hours_ms(25);
        set_modified_time(&candidate_path, old_modified);

        let db_path = temp.path().join("index.sqlite3");
        let db = Database::open(&db_path).expect("open db");
        db.migrate().expect("migrate db");
        let run_id = db
            .create_scan_run(
                root.to_string_lossy().as_ref(),
                Some("NTFS"),
                ScanMode::Recursive,
                None,
                None,
                100,
            )
            .expect("create scan run");
        db.insert_file_entries(
            run_id,
            &[cleanup_indexed_entry(
                &candidate_path.to_string_lossy(),
                20,
                Some(old_modified),
            )],
        )
        .expect("insert file entries");
        let plan = build_cleanup_plan(&db, run_id, 50).expect("cleanup plan");
        let candidate = plan
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with("old.tmp"))
            .expect("tmp cleanup candidate");
        assert_eq!(
            candidate.id,
            cleanup_candidate_id(run_id, candidate.entry_id)
        );
        let recycler = RecordingRecycler::default();

        let result = execute_cleanup_plan_with_recycler(
            &db,
            run_id,
            std::slice::from_ref(&candidate.id),
            &recycler,
        )
        .expect("cleanup execution");

        assert_eq!(result.recycled_count, 1);
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.skipped_count, 0);
        assert_eq!(
            recycler.recycled_paths(),
            vec![std::fs::canonicalize(&candidate_path).expect("canonical candidate")]
        );
        assert_eq!(
            result
                .actions
                .first()
                .map(|action| (action.status.as_str(), action.undo_token.as_deref())),
            Some(("recycled", Some("windows-recycle-bin")))
        );
    }

    #[test]
    fn cleanup_execution_resolves_selected_candidates_beyond_plan_preview_limit() {
        let temp = safe_tempdir();
        let root = temp.path().join("fixture");
        fs::create_dir_all(&root).expect("fixture dir");
        let old_modified = current_test_millis() - hours_ms(25);

        let db_path = temp.path().join("index.sqlite3");
        let db = Database::open(&db_path).expect("open db");
        db.migrate().expect("migrate db");
        let run_id = db
            .create_scan_run(
                root.to_string_lossy().as_ref(),
                Some("NTFS"),
                ScanMode::Recursive,
                None,
                None,
                100,
            )
            .expect("create scan run");
        let mut entries = Vec::new();
        for index in 0..501 {
            let candidate_path = root.join(format!("cache-{index:03}.tmp"));
            fs::write(&candidate_path, vec![1_u8; 10]).expect("tmp file");
            set_modified_time(&candidate_path, old_modified);
            entries.push(cleanup_indexed_entry(
                &candidate_path.to_string_lossy(),
                10,
                Some(old_modified),
            ));
        }
        db.insert_file_entries(run_id, &entries)
            .expect("insert file entries");
        let hidden_candidate_id = cleanup_candidate_id(run_id, 501);
        let recycler = RecordingRecycler::default();

        let result = execute_cleanup_plan_with_recycler(
            &db,
            run_id,
            std::slice::from_ref(&hidden_candidate_id),
            &recycler,
        )
        .expect("cleanup execution");

        assert_eq!(result.recycled_count, 1);
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.skipped_count, 0);
        assert_eq!(recycler.recycled_paths().len(), 1);
        assert!(recycler.recycled_paths()[0].ends_with("cache-500.tmp"));
    }

    #[cfg(windows)]
    #[test]
    fn recycle_worker_returns_operation_errors() {
        let error = super::run_recycle_operation_on_worker::<_, ()>(|| {
            anyhow::bail!("simulated recycle failure")
        })
        .expect_err("worker should return operation errors");

        assert!(error.to_string().contains("simulated recycle failure"));
    }

    #[derive(Default)]
    struct RecordingRecycler {
        recycled: Mutex<Vec<PathBuf>>,
    }

    impl RecordingRecycler {
        fn recycled_paths(&self) -> Vec<PathBuf> {
            self.recycled.lock().expect("recycler lock").clone()
        }
    }

    impl FileRecycler for RecordingRecycler {
        fn recycle_file(&self, path: &Path) -> anyhow::Result<()> {
            self.recycled
                .lock()
                .expect("recycler lock")
                .push(path.to_path_buf());
            Ok(())
        }
    }

    fn safe_tempdir() -> tempfile::TempDir {
        let fixture_root = std::env::current_dir()
            .expect("current dir")
            .join("target")
            .join("test-fixtures");
        fs::create_dir_all(&fixture_root).expect("fixture root");
        tempfile::Builder::new()
            .prefix("winclean-cleanup-")
            .tempdir_in(fixture_root)
            .expect("safe temp dir")
    }

    fn set_modified_time(path: &Path, modified_at_ms: i64) {
        let modified = UNIX_EPOCH + Duration::from_millis(modified_at_ms as u64);
        File::options()
            .write(true)
            .open(path)
            .expect("open file for timestamp")
            .set_modified(modified)
            .expect("set modified time");
    }

    fn cleanup_indexed_entry(path: &str, size: u64, modified_at_ms: Option<i64>) -> IndexedEntry {
        let path_buf = PathBuf::from(path);
        IndexedEntry {
            path: path.to_string(),
            parent_path: path_buf
                .parent()
                .map(|parent| parent.to_string_lossy().to_string()),
            name: path_buf
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string()),
            extension: path_buf
                .extension()
                .map(|ext| ext.to_string_lossy().to_string()),
            is_directory: false,
            size,
            modified_at_ms,
            created_at_ms: None,
            accessed_at_ms: None,
            attributes: 0,
            is_reparse_point: false,
            is_placeholder: false,
            is_system: false,
        }
    }

    fn current_test_millis() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
    }

    fn hours_ms(hours: i64) -> i64 {
        hours * 60 * 60 * 1000
    }
}
