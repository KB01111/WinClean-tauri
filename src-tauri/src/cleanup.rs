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

#[derive(Clone, Debug, Serialize)]
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
    let plan = build_cleanup_plan(db, scan_run_id, 500)?;
    let candidates = resolve_cleanup_candidates(&plan.candidates, candidate_ids)?;

    let mut actions = Vec::new();
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

        match validate_cleanup_target(&candidate) {
            Ok(path) => match move_to_recycle_bin(&path) {
                Ok(()) => {
                    recycled_count += 1;
                    db.update_cleanup_action(
                        action_id,
                        "recycled",
                        Some(unix_millis()),
                        None,
                        Some("windows-recycle-bin"),
                    )?;
                }
                Err(error) => {
                    failed_count += 1;
                    db.update_cleanup_action(
                        action_id,
                        "failed",
                        Some(unix_millis()),
                        Some(&error.to_string()),
                        None,
                    )?;
                }
            },
            Err(error) => {
                skipped_count += 1;
                db.update_cleanup_action(
                    action_id,
                    "skipped",
                    Some(unix_millis()),
                    Some(&error.to_string()),
                    None,
                )?;
            }
        }

        if let Some(action) = db.cleanup_action(action_id)? {
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

fn cleanup_reason(record: &CleanupFileRecord) -> Option<(String, CleanupRisk)> {
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

fn is_protected_path(path: &str) -> bool {
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

#[cfg(windows)]
fn move_to_recycle_bin(path: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };

    let canonical = std::fs::canonicalize(path)
        .map_err(|error| anyhow::anyhow!("cannot recycle {}: {error}", path.display()))?;
    if !canonical.is_file() {
        anyhow::bail!(
            "cleanup only recycles files in this version: {}",
            path.display()
        );
    }

    let mut from = canonical
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_mut_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result != 0 {
        anyhow::bail!("Windows recycle operation failed with code {result}");
    }
    if operation.fAnyOperationsAborted != 0 {
        anyhow::bail!("Windows recycle operation was canceled");
    }
    Ok(())
}

#[cfg(not(windows))]
fn move_to_recycle_bin(path: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "Recycle Bin cleanup is only available on Windows in this build: {}",
        path.display()
    )
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
