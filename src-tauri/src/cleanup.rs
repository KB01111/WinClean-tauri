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
    let policy = crate::storage_policy::StoragePolicy::load(db, scan_run_id)?;
    let mut candidates = records
        .into_iter()
        .filter(|record| policy.check_for_cleanup(&record.path, record.extension.as_deref(), false).is_none())
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
    // Exclusions can change between planning and execution, so the policy is read again here.
    let policy = crate::storage_policy::StoragePolicy::load(db, scan_run_id)?;

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

        let checked = match policy.check_for_cleanup(&candidate.path, None, false) {
            Some(blocked) => Err(anyhow::anyhow!("{}", blocked.detail)),
            None => validate_file_for_action(&candidate.path, candidate.size, candidate.modified_at_ms).and_then(|path| ensure_still_a_cleanup_category(&path).map(|_| path)),
        };
        match checked {
            Ok(path) => match recycle_file(&path) {
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
                if let Some(action) = db.cleanup_action(action_id)? {
                    actions.push(action);
                }
                continue;
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

/// The scan-time label is not trusted at execution time: the file the path
/// resolves to *now* must still be of a category WinClean cleans (a swapped
/// file or junction target keeps its old "Log file" label otherwise).
pub fn ensure_still_a_cleanup_category(canonical: &Path) -> anyhow::Result<()> {
    let name = canonical.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let extension = canonical.extension().map(|e| e.to_string_lossy().to_string());
    let record = CleanupFileRecord {
        id: 0,
        path: canonical.to_string_lossy().to_string(),
        name,
        extension,
        size: 0,
        modified_at_ms: None,
        is_reparse_point: false,
        is_placeholder: false,
        is_system: false,
    };
    if cleanup_reason(&record).is_none() {
        anyhow::bail!("the file no longer matches a cleanup category: {}", canonical.display());
    }
    Ok(())
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
    if is_cleanup_protected_path(&candidate.path) {
        anyhow::bail!("cleanup target is protected: {}", candidate.path);
    }
    validate_cleanup_age(candidate.modified_at_ms, unix_millis())?;
    validate_scratch_age(&candidate.path, candidate.modified_at_ms, unix_millis())?;

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
    if is_cleanup_protected_path(&canonical_display) {
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

    if is_cleanup_protected_path(&record.path) {
        return None;
    }

    if validate_cleanup_age(record.modified_at_ms, now_ms).is_err() || validate_scratch_age(&record.path, record.modified_at_ms, now_ms).is_err() {
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

/// Locations no feature may touch: system, program and application-data folders, shares and roots.
pub fn is_protected_path(path: &str) -> bool {
    protected(path, false)
}

/// Like [`is_protected_path`], but a user's Temp and crash-dump folders (disposable by
/// Windows' own definition) may be cleaned. Used by cleanup only, never to authorise access.
pub fn is_cleanup_protected_path(path: &str) -> bool {
    protected(path, true)
}

fn protected(path: &str, allow_scratch: bool) -> bool {
    let mut normalized = normalize_for_rules(path);
    // `\\?\C:\…` is the same location as `C:\…`; `\\?\UNC\…` is a network share (never ours to clean).
    if let Some(rest) = normalized.strip_prefix("\\\\?\\") {
        if rest.starts_with("unc\\") {
            return true;
        }
        normalized = rest.to_string();
    }
    if normalized.starts_with("\\\\") {
        return true; // UNC path
    }
    let root_like = normalized.ends_with(":\\")
        || normalized.matches('\\').count() <= 1
        || PathBuf::from(path).parent().is_none();
    if root_like {
        return true;
    }
    // Compare with a trailing separator so `C:\Windows` itself is covered too.
    let padded = format!("{normalized}\\");
    // Application data is protected (profiles, settings, saves) except the two folders Windows
    // itself treats as disposable: a user's Temp folder and crash dumps.
    let appdata_allowed = allow_scratch && is_user_scratch_path(&normalized);
    const PROTECTED: &[&str] = &[
        "\\windows\\", "\\windows.old\\", "\\program files\\", "\\program files (x86)\\", "\\programdata\\", "\\appdata\\",
        "\\system volume information\\", "\\recovery\\", "\\boot\\", "\\msocache\\", "\\perflogs\\",
    ];
    if PROTECTED.iter().any(|p| padded.contains(p) && !(appdata_allowed && *p == "\\appdata\\")) {
        return true;
    }
    // Any component starting with `$` is a Windows system folder (`$Recycle.Bin`, `$Extend`, `$WINDOWS.~BT`, …).
    normalized.split('\\').skip(1).any(|component| component.starts_with('$'))
}

/// A file inside `C:\Users\<name>\AppData\Local\Temp` or `...\Local\CrashDumps` (already lower-cased, `\`-separated).
fn is_user_scratch_path(normalized: &str) -> bool {
    let mut parts = normalized.split('\\');
    let drive = parts.next().unwrap_or("");
    let drive_ok = drive.len() == 2 && drive.ends_with(':') && drive.as_bytes()[0].is_ascii_alphabetic();
    let rest: Vec<&str> = parts.collect();
    drive_ok
        && rest.len() >= 6
        && rest[0] == "users"
        && !rest[1].is_empty()
        && rest[2] == "appdata"
        && rest[3] == "local"
        && matches!(rest[4], "temp" | "crashdumps")
        && !rest[5].is_empty()
}

/// Files in the user Temp and crash-dump folders are only ever cleaned once they have been
/// untouched this long (Windows' own Disk Cleanup uses the same week).
pub const SCRATCH_MIN_AGE_DAYS: u64 = 7;

/// Fails for a file in a user Temp or crash-dump folder that was modified within the last week.
pub fn validate_scratch_age(path: &str, modified_at_ms: Option<i64>, now_ms: i64) -> anyhow::Result<()> {
    if !is_user_scratch_path(&normalize_for_rules(path)) {
        return Ok(());
    }
    let week_ms = SCRATCH_MIN_AGE_DAYS as i64 * 86_400_000;
    match modified_at_ms {
        Some(modified) if now_ms.saturating_sub(modified) >= week_ms => Ok(()),
        Some(_) => anyhow::bail!("files in the Temp folder are only cleaned after {SCRATCH_MIN_AGE_DAYS} days without changes"),
        None => anyhow::bail!("cleanup target modified time is unavailable"),
    }
}

/// True for files in a user's Temp folder (the only application-data location cleaned by age).
pub fn is_user_temp_file(path: &str) -> bool {
    let normalized = normalize_for_rules(path);
    is_user_scratch_path(&normalized) && normalized.split('\\').nth(5) == Some("temp")
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

/// The legacy shell file APIs stop at MAX_PATH (260) characters, terminator included.
const WINDOWS_MAX_PATH: usize = 260;

/// `\\?\C:\dir\file` → `C:\dir\file`. Other forms (plain paths, `\\?\UNC\…`) are returned unchanged.
pub fn strip_verbatim_prefix(wide: &[u16]) -> &[u16] {
    let prefix: Vec<u16> = "\\\\?\\".encode_utf16().collect();
    let drive_form = wide.len() >= prefix.len() + 2
        && wide.starts_with(&prefix)
        && char::from_u32(u32::from(wide[prefix.len()])).is_some_and(|c| c.is_ascii_alphabetic())
        && wide[prefix.len() + 1] == u16::from(b':');
    if drive_form {
        &wide[prefix.len()..]
    } else {
        wide
    }
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
    // Only fixed local drives have a Recycle Bin. On removable and network drives
    // SHFileOperation would delete permanently without a prompt, so refuse.
    ensure_fixed_drive(&canonical)?;

    // SHFileOperationW rejects the `\\?\` form that canonicalize returns (error 124), and it
    // only understands paths shorter than MAX_PATH.
    let wide = canonical.as_os_str().encode_wide().collect::<Vec<u16>>();
    let plain = strip_verbatim_prefix(&wide);
    if plain.len() >= WINDOWS_MAX_PATH {
        anyhow::bail!(
            "the path is longer than the Windows Recycle Bin API accepts, so it was not recycled: {}",
            path.display()
        );
    }
    let mut from = plain
        .iter()
        .copied()
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

#[cfg(windows)]
fn ensure_fixed_drive(canonical: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    /// `DRIVE_FIXED` from fileapi.h.
    const DRIVE_FIXED: u32 = 3;

    let text = canonical.to_string_lossy();
    let trimmed = text.strip_prefix("\\\\?\\").unwrap_or(&text);
    let root: String = trimmed.chars().take(3).collect();
    if root.len() < 3 || !root.as_bytes()[0].is_ascii_alphabetic() || &root[1..] != ":\\" {
        anyhow::bail!("cannot tell which drive holds {}; it was not recycled", canonical.display());
    }
    let wide: Vec<u16> = std::ffi::OsStr::new(&root).encode_wide().chain(std::iter::once(0)).collect();
    if unsafe { GetDriveTypeW(wide.as_ptr()) } != DRIVE_FIXED {
        anyhow::bail!("{root} is not a fixed drive, so it has no Recycle Bin and the file was not removed");
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

/// Nothing modified within the last 24 hours (or with an unknown modification time) is ever cleaned or offloaded.
pub fn validate_cleanup_age(modified_at_ms: Option<i64>, now_ms: i64) -> anyhow::Result<()> {
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
mod scratch_tests {
    use super::*;

    #[test]
    fn the_extended_length_prefix_is_removed_for_the_recycle_bin_api_only_on_drive_paths() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let strip = |s: &str| String::from_utf16(strip_verbatim_prefix(&w(s))).unwrap();
        assert_eq!(strip("\\\\?\\C:\\tmp\\a.tmp"), "C:\\tmp\\a.tmp");
        assert_eq!(strip("C:\\tmp\\a.tmp"), "C:\\tmp\\a.tmp");
        assert_eq!(strip("\\\\?\\UNC\\server\\share\\a.tmp"), "\\\\?\\UNC\\server\\share\\a.tmp");
        assert_eq!(strip("\\\\?\\"), "\\\\?\\");
        assert_eq!(strip(""), "");
    }

    #[test]
    fn user_temp_is_cleanable_but_still_protected_for_access_checks() {
        let temp = "C:\\Users\\me\\AppData\\Local\\Temp\\cache.dat";
        assert!(!is_cleanup_protected_path(temp));
        assert!(is_protected_path(temp), "the strict check (used for MCP roots) is unchanged");
        assert!(is_user_temp_file(temp));
        assert!(!is_user_temp_file("C:\\Users\\me\\AppData\\Local\\CrashDumps\\x.dmp"), "dumps are matched by extension, not by the age rule");
        assert!(!is_cleanup_protected_path("C:\\Users\\me\\AppData\\Local\\CrashDumps\\x.dmp"));
    }

    #[test]
    fn the_rest_of_appdata_and_other_system_places_stay_protected() {
        for path in [
            "C:\\Users\\me\\AppData\\Local\\Temp",
            "C:\\Users\\me\\AppData\\Local\\Microsoft\\Edge\\User Data\\Default\\a.log",
            "C:\\Users\\me\\AppData\\Roaming\\App\\a.log",
            "C:\\Users\\me\\AppData\\Local\\Temp\\..\\Packages\\a.log",
            "\\\\?\\UNC\\server\\share\\a.log",
            "C:\\Windows\\Temp\\a.log",
        ] {
            assert!(is_cleanup_protected_path(path) || path.contains(".."), "{path}");
        }
    }
}
