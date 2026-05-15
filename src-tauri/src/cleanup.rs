use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::db::{CleanupActionInsert, CleanupActionRecord, CleanupFileRecord, Database};

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

pub fn cleanup_candidate_id(scan_run_id: i64, entry_id: i64) -> String {
    format!("scan-{scan_run_id}:entry-{entry_id}")
}

pub fn build_cleanup_plan(
    db: &Database,
    scan_run_id: i64,
    limit: u32,
) -> anyhow::Result<CleanupPlan> {
    let records = db.cleanup_file_records(scan_run_id, 5000)?;
    let mut candidates = records
        .into_iter()
        .filter_map(|record| cleanup_candidate(scan_run_id, record))
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

        match validate_file_for_action(&candidate.path, candidate.size, candidate.modified_at_ms) {
            Ok(()) => {}
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

        match recycle_file(Path::new(&candidate.path)) {
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

pub fn restore_cleanup_action(
    db: &Database,
    action_id: i64,
) -> anyhow::Result<CleanupActionRecord> {
    let action = db
        .cleanup_action(action_id)?
        .ok_or_else(|| anyhow::anyhow!("cleanup action {action_id} was not found"))?;

    if action.status != "recycled" {
        anyhow::bail!("only recycled cleanup actions can be restored");
    }

    db.update_cleanup_action(
        action_id,
        "restore_requested",
        Some(unix_millis()),
        Some("Restore is available from the Windows Recycle Bin. Open Recycle Bin and restore this item to its original path."),
        action.undo_token.as_deref(),
    )?;

    db.cleanup_action(action_id)?
        .ok_or_else(|| anyhow::anyhow!("cleanup action {action_id} was not found after update"))
}

fn cleanup_candidate(scan_run_id: i64, record: CleanupFileRecord) -> Option<CleanupCandidate> {
    if record.is_reparse_point || record.is_placeholder || record.is_system {
        return None;
    }

    if is_protected_path(&record.path) {
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
        || normalized.contains("\\appdata\\local\\microsoft\\windows\\")
}

fn normalize_for_rules(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

pub fn validate_file_for_action(
    path: &str,
    expected_size: u64,
    expected_modified_at_ms: Option<i64>,
) -> anyhow::Result<()> {
    if is_protected_path(path) {
        anyhow::bail!("protected path is not eligible for automated actions");
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| anyhow::anyhow!("file is no longer available: {error}"))?;
    if !metadata.is_file() {
        anyhow::bail!("only files are eligible for this action");
    }
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
    Ok(())
}

pub fn recycle_file(path: &Path) -> anyhow::Result<()> {
    move_to_recycle_bin(path)
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
