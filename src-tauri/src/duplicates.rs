//! Staged duplicate detection on the scan index.
//!
//! 1. **Size** — free, from the index: only files of identical size can match.
//! 2. **Head** — on request, a 64 KiB prefix hash prunes most same-size files
//!    that are not copies without reading them whole.
//! 3. **Full SHA-256** — only for files whose prefix still matches. The result is
//!    remembered (and discarded automatically when the file's size or
//!    modification time no longer match the index).
//!
//! Reading is lazy (never during a scan or in the background without a click),
//! bounded by a byte and file budget, cancellable between chunks, skips
//! anything the storage policy protects or the user excluded, stops when the PC
//! goes onto battery if asked to, and runs at background I/O priority.
//! Nothing here deletes anything; confirmed duplicates only become *review*
//! recommendations.

use std::{
    collections::HashMap,
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    cleanup::cleanup_candidate_id,
    db::{Database, FileSearchResult},
    storage_policy::StoragePolicy,
};

pub const HEAD_BYTES: u64 = 64 * 1024;
const CHUNK: usize = 1024 * 1024;
pub const DEFAULT_MIN_SIZE: u64 = 1024 * 1024;
pub const DEFAULT_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_BYTES_CEILING: u64 = 32 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: usize = 400;
const MAX_GROUPS_EXAMINED: u32 = 200;
const MAX_REPORT_GROUPS: usize = 100;

/// How the bytes of one file are read. The real implementation re-validates the
/// file against the index first; tests substitute an in-memory one.
pub trait FileAccess {
    /// Fails unless the file is still an ordinary, unprotected file that matches
    /// what the scan indexed.
    fn check(&self, entry: &FileSearchResult) -> Result<(), String>;
    /// Feeds at most `limit` bytes (all, when `None`) to `sink` chunk by chunk;
    /// `sink` returns false to stop (cancellation).
    fn read(&self, entry: &FileSearchResult, limit: Option<u64>, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), String>;
}

pub struct DiskAccess;

impl FileAccess for DiskAccess {
    fn check(&self, entry: &FileSearchResult) -> Result<(), String> {
        crate::cleanup::validate_file_for_action(&entry.path, entry.size, entry.modified_at_ms).map(|_| ()).map_err(|e| e.to_string())
    }

    fn read(&self, entry: &FileSearchResult, limit: Option<u64>, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), String> {
        let mut file = std::fs::File::open(&entry.path).map_err(|e| format!("cannot open: {e}"))?;
        let mut remaining = limit.unwrap_or(u64::MAX);
        let mut buf = vec![0u8; CHUNK];
        while remaining > 0 {
            let want = buf.len().min(usize::try_from(remaining).unwrap_or(usize::MAX));
            let n = file.read(&mut buf[..want]).map_err(|e| format!("read failed: {e}"))?;
            if n == 0 {
                break;
            }
            remaining -= n as u64;
            if !sink(&buf[..n]) {
                break;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct VerifyOptions {
    pub min_size: u64,
    pub max_bytes: u64,
    pub max_files: usize,
    /// Stop (not fail) as soon as the PC runs on battery.
    pub stop_on_battery: bool,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self { min_size: DEFAULT_MIN_SIZE, max_bytes: DEFAULT_MAX_BYTES, max_files: DEFAULT_MAX_FILES, stop_on_battery: true }
    }
}

impl VerifyOptions {
    pub fn clamped(mut self) -> Self {
        self.min_size = self.min_size.max(4096);
        self.max_bytes = self.max_bytes.clamp(HEAD_BYTES, MAX_BYTES_CEILING);
        self.max_files = self.max_files.clamp(2, 2_000);
        self
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VerifySummary {
    pub groups_examined: u32,
    pub files_hashed: u32,
    pub bytes_read: u64,
    pub files_skipped: u32,
    pub budget_exhausted: bool,
    pub cancelled: bool,
    /// Why the run ended early, in plain words.
    pub stopped: Option<String>,
}

#[derive(Default)]
pub struct Progress {
    pub files_hashed: std::sync::atomic::AtomicU32,
    pub bytes_read: std::sync::atomic::AtomicU64,
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

/// SHA-256 of `limit` bytes (all by default). `None` means cancelled or unreadable.
fn hash_of(access: &dyn FileAccess, entry: &FileSearchResult, limit: Option<u64>, cancel: &AtomicBool, read_total: &mut u64) -> Result<Option<String>, String> {
    let mut hasher = Sha256::new();
    let mut cancelled = false;
    access.read(entry, limit, &mut |chunk| {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            return false;
        }
        *read_total += chunk.len() as u64;
        hasher.update(chunk);
        true
    })?;
    if cancelled {
        return Ok(None);
    }
    Ok(Some(hex(&hasher.finalize())))
}

/// Compare same-size files by content, within the budget. Only ever reads.
pub fn verify_duplicates(
    db: &Database,
    run_id: i64,
    access: &dyn FileAccess,
    options: &VerifyOptions,
    cancel: &AtomicBool,
    on_battery: &dyn Fn() -> bool,
    progress: &Progress,
) -> anyhow::Result<VerifySummary> {
    let options = options.clone().clamped();
    let mut policy = StoragePolicy::load(db, run_id)?;
    let known = db.valid_hashes(run_id)?;
    let heads = db.valid_heads(run_id)?;
    let groups = db.duplicate_size_groups(run_id, options.min_size, MAX_GROUPS_EXAMINED)?;
    let mut summary = VerifySummary::default();

    'groups: for group in groups {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            summary.stopped = Some("Cancelled".to_string());
            break;
        }
        if options.stop_on_battery && on_battery() {
            summary.stopped = Some("Paused because the PC is running on battery".to_string());
            break;
        }

        // Skip what policy protects or the user excluded: those are never read. A folder excluded
        // while this job runs applies from the next group on.
        policy.refresh_exclusions(db);
        let mut eligible = Vec::new();
        for file in group.files {
            if policy.check_with(&file.path, file.extension.as_deref(), true).is_some() {
                summary.files_skipped += 1;
            } else {
                eligible.push(file);
            }
        }
        if eligible.len() < 2 {
            continue;
        }
        summary.groups_examined += 1;

        // Nothing to learn when every file in the group already has a verified hash.
        if eligible.iter().all(|f| known.contains_key(&f.id)) {
            continue;
        }

        // Stage 2: prefix hashes (remembered ones are reused). Files that already have a full
        // hash are probed too, so a new file is compared against them without a full read.
        let mut by_head: HashMap<String, Vec<FileSearchResult>> = HashMap::new();
        for file in eligible {
            if let Some(head) = heads.get(&file.id) {
                by_head.entry(head.clone()).or_default().push(file);
                continue;
            }
            if summary.files_hashed as usize >= options.max_files || summary.bytes_read + HEAD_BYTES > options.max_bytes {
                summary.budget_exhausted = true;
                break 'groups;
            }
            if access.check(&file).is_err() {
                summary.files_skipped += 1;
                continue;
            }
            let mut read = 0;
            match hash_of(access, &file, Some(HEAD_BYTES.min(file.size)), cancel, &mut read) {
                Ok(Some(head)) => {
                    summary.bytes_read += read;
                    progress.bytes_read.fetch_add(read, Ordering::Relaxed);
                    db.save_duplicate_head(run_id, file.id, &head, file.size, file.modified_at_ms)?;
                    by_head.entry(head).or_default().push(file);
                }
                Ok(None) => {
                    summary.cancelled = true;
                    summary.stopped = Some("Cancelled".to_string());
                    break 'groups;
                }
                Err(_) => summary.files_skipped += 1,
            }
        }

        // Stage 3: a full hash only for unhashed files whose prefix matches another file.
        for (_, mut files) in by_head {
            if files.len() < 2 {
                continue;
            }
            files.sort_by_key(|f| f.id);
            for file in files.into_iter().filter(|f| !known.contains_key(&f.id)) {
                if cancel.load(Ordering::Relaxed) {
                    summary.cancelled = true;
                    summary.stopped = Some("Cancelled".to_string());
                    break 'groups;
                }
                if summary.files_hashed as usize >= options.max_files || summary.bytes_read + file.size > options.max_bytes {
                    summary.budget_exhausted = true;
                    break 'groups;
                }
                // Re-check right before the full read: the file may have changed since the prefix.
                if access.check(&file).is_err() {
                    summary.files_skipped += 1;
                    continue;
                }
                let mut read = 0;
                match hash_of(access, &file, None, cancel, &mut read) {
                    Ok(Some(sha)) => {
                        summary.bytes_read += read;
                        progress.bytes_read.fetch_add(read, Ordering::Relaxed);
                        // A file that changed while it was being read proves nothing.
                        if access.check(&file).is_err() {
                            summary.files_skipped += 1;
                            continue;
                        }
                        db.save_duplicate_hash(run_id, file.id, &sha, file.size, file.modified_at_ms, now_ms())?;
                        summary.files_hashed += 1;
                        progress.files_hashed.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(None) => {
                        summary.cancelled = true;
                        summary.stopped = Some("Cancelled".to_string());
                        break 'groups;
                    }
                    Err(_) => summary.files_skipped += 1,
                }
            }
        }
    }
    if summary.budget_exhausted && summary.stopped.is_none() {
        summary.stopped = Some("Stopped at the read budget; run again to continue".to_string());
    }
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    pub entry_id: i64,
    pub candidate_id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified_at_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicateStatus {
    /// Identical content, proven by SHA-256.
    Confirmed,
    /// Same size only; content not compared.
    Possible,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub status: DuplicateStatus,
    pub size: u64,
    /// Short fingerprint of a confirmed group.
    pub fingerprint: Option<String>,
    pub files: Vec<DuplicateFile>,
    /// Space freed if all but one copy were removed (an upper bound for possible groups).
    pub reclaimable_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateReport {
    pub scan_run_id: i64,
    pub confirmed: Vec<DuplicateGroup>,
    pub possible: Vec<DuplicateGroup>,
    pub confirmed_reclaimable_bytes: u64,
    pub possible_reclaimable_bytes: u64,
    /// Same-size files that policy keeps out of every list.
    pub protected_files: u32,
    pub notes: Vec<String>,
}

fn dup_file(run_id: i64, f: &FileSearchResult) -> DuplicateFile {
    DuplicateFile { entry_id: f.id, candidate_id: cleanup_candidate_id(run_id, f.id), path: f.path.clone(), name: f.name.clone(), size: f.size, modified_at_ms: f.modified_at_ms }
}

/// Confirmed and possible duplicate groups from what is already in the index
/// and hash table. Reads no file.
pub fn duplicate_report(db: &Database, run_id: i64, min_size: u64) -> anyhow::Result<DuplicateReport> {
    let policy = StoragePolicy::load(db, run_id)?;
    let hashes = db.valid_hashes(run_id)?;
    let heads = db.valid_heads(run_id)?;
    let mut report = DuplicateReport {
        scan_run_id: run_id,
        confirmed: Vec::new(),
        possible: Vec::new(),
        confirmed_reclaimable_bytes: 0,
        possible_reclaimable_bytes: 0,
        protected_files: 0,
        notes: vec![
            "Confirmed means the contents were compared (SHA-256). Possible means only the size matches.".to_string(),
            "WinClean never deletes duplicates automatically; keep at least one copy of each.".to_string(),
        ],
    };

    for group in db.duplicate_size_groups(run_id, min_size, MAX_GROUPS_EXAMINED)? {
        let mut open = Vec::new();
        for file in group.files {
            if policy.check_with(&file.path, file.extension.as_deref(), true).is_some() {
                report.protected_files += 1;
            } else {
                open.push(file);
            }
        }
        let mut by_hash: HashMap<&str, Vec<&FileSearchResult>> = HashMap::new();
        let mut unhashed: Vec<&FileSearchResult> = Vec::new();
        for file in &open {
            match hashes.get(&file.id) {
                Some(sha) => by_hash.entry(sha.as_str()).or_default().push(file),
                None => unhashed.push(file),
            }
        }
        // Everything not confirmed: hashed-but-unique files and untested files. Files whose
        // prefixes were compared and differ cannot be duplicates of each other.
        let mut leftovers: Vec<&FileSearchResult> = unhashed;
        for (sha, files) in by_hash {
            if files.len() >= 2 {
                let reclaim = group.size * (files.len() as u64 - 1);
                report.confirmed_reclaimable_bytes += reclaim;
                report.confirmed.push(DuplicateGroup {
                    status: DuplicateStatus::Confirmed,
                    size: group.size,
                    fingerprint: Some(sha.chars().take(12).collect()),
                    files: files.iter().map(|f| dup_file(run_id, f)).collect(),
                    reclaimable_bytes: reclaim,
                });
            } else {
                leftovers.extend(files);
            }
        }
        if !leftovers.iter().any(|f| !hashes.contains_key(&f.id)) {
            continue; // everything here was fully compared
        }
        let mut by_head: HashMap<&str, Vec<&FileSearchResult>> = HashMap::new();
        let mut untested: Vec<&FileSearchResult> = Vec::new();
        for f in leftovers {
            match heads.get(&f.id) {
                Some(h) => by_head.entry(h.as_str()).or_default().push(f),
                None => untested.push(f),
            }
        }
        let mut push_possible = |files: Vec<&FileSearchResult>| {
            if files.len() >= 2 {
                let reclaim = group.size * (files.len() as u64 - 1);
                report.possible_reclaimable_bytes += reclaim;
                report.possible.push(DuplicateGroup {
                    status: DuplicateStatus::Possible,
                    size: group.size,
                    fingerprint: None,
                    files: files.iter().map(|f| dup_file(run_id, f)).collect(),
                    reclaimable_bytes: reclaim,
                });
            }
        };
        for (_, files) in by_head {
            // A partition that was fully hashed and found different has nothing left to confirm.
            if files.iter().any(|f| !hashes.contains_key(&f.id)) {
                push_possible(files);
            }
        }
        push_possible(untested);
    }
    report.confirmed.sort_by(|a, b| b.reclaimable_bytes.cmp(&a.reclaimable_bytes));
    report.possible.sort_by(|a, b| b.reclaimable_bytes.cmp(&a.reclaimable_bytes));
    report.confirmed.truncate(MAX_REPORT_GROUPS);
    report.possible.truncate(MAX_REPORT_GROUPS);
    Ok(report)
}

/// Entry ids that belong to a confirmed duplicate group → (fingerprint, copies).
pub fn confirmed_entry_ids(db: &Database, run_id: i64) -> HashMap<i64, (String, u32)> {
    db.confirmed_duplicates(run_id).unwrap_or_default()
}

/// Ask Windows to treat this thread's disk and CPU work as background work.
#[cfg(windows)]
pub fn enter_background_io() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

#[cfg(not(windows))]
pub fn enter_background_io() {}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::db_storage::testing::*;

    /// Files held in memory, keyed by path; counts how many bytes are served.
    struct Memory {
        files: HashMap<String, Vec<u8>>,
        served: RefCell<u64>,
        changed: RefCell<HashSet<String>>,
    }

    impl Memory {
        fn new(files: &[(&str, Vec<u8>)]) -> Self {
            Self { files: files.iter().map(|(p, b)| (p.to_string(), b.clone())).collect(), served: RefCell::new(0), changed: RefCell::new(HashSet::new()) }
        }
    }

    impl FileAccess for Memory {
        fn check(&self, entry: &FileSearchResult) -> Result<(), String> {
            if self.changed.borrow().contains(&entry.path) {
                return Err("changed since scan".into());
            }
            self.files.contains_key(&entry.path).then_some(()).ok_or_else(|| "missing".to_string())
        }
        fn read(&self, entry: &FileSearchResult, limit: Option<u64>, sink: &mut dyn FnMut(&[u8]) -> bool) -> Result<(), String> {
            let data = self.files.get(&entry.path).ok_or("missing")?;
            let end = limit.map(|l| (l as usize).min(data.len())).unwrap_or(data.len());
            for chunk in data[..end].chunks(1000) {
                *self.served.borrow_mut() += chunk.len() as u64;
                if !sink(chunk) {
                    break;
                }
            }
            Ok(())
        }
    }

    fn body(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| seed.wrapping_add((i % 251) as u8)).collect()
    }

    const A: &str = "C:\\Users\\me\\Downloads\\a.bin";
    const B: &str = "C:\\Users\\me\\Pictures\\b.bin";
    const C: &str = "C:\\Users\\me\\Videos\\c.bin"; // same size, same first 64 KiB, different tail
    const D: &str = "C:\\Users\\me\\Music\\d.bin"; // same size, different head

    fn scenario() -> (tempfile::TempDir, Database, i64, Memory) {
        let len = 200 * 1024;
        let a = body(1, len);
        let mut c = a.clone();
        *c.last_mut().unwrap() ^= 0xFF;
        let d = body(9, len);
        let entries = vec![file(A, len as u64, 40), file(B, len as u64, 40), file(C, len as u64, 40), file(D, len as u64, 40)];
        let (dir, db, run) = seeded(entries);
        (dir, db, run, Memory::new(&[(A, a.clone()), (B, a), (C, c), (D, d)]))
    }

    fn opts() -> VerifyOptions {
        VerifyOptions { min_size: 4096, max_bytes: 10 * 1024 * 1024, max_files: 50, stop_on_battery: false }
    }

    fn run_verify(db: &Database, run: i64, mem: &Memory, o: &VerifyOptions, cancel: &AtomicBool) -> VerifySummary {
        verify_duplicates(db, run, mem, o, cancel, &|| false, &Progress::default()).unwrap()
    }

    #[test]
    fn before_any_hashing_same_size_files_are_only_possible_duplicates() {
        let (_d, db, run, _m) = scenario();
        let report = duplicate_report(&db, run, 1).unwrap();
        assert!(report.confirmed.is_empty());
        assert_eq!(report.possible.len(), 1);
        assert_eq!(report.possible[0].files.len(), 4);
        assert_eq!(report.possible[0].status, DuplicateStatus::Possible);
    }

    #[test]
    fn identical_content_is_confirmed_and_same_size_lookalikes_are_not() {
        let (_d, db, run, mem) = scenario();
        let summary = run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        assert!(!summary.cancelled && !summary.budget_exhausted);
        let report = duplicate_report(&db, run, 1).unwrap();
        assert_eq!(report.confirmed.len(), 1, "{report:?}");
        let paths: HashSet<_> = report.confirmed[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, HashSet::from([A, B]));
        assert!(report.possible.is_empty(), "C and D were compared and differ: {:?}", report.possible);
        assert_eq!(report.confirmed_reclaimable_bytes, 200 * 1024);
    }

    #[test]
    fn a_different_head_is_pruned_without_reading_the_whole_file() {
        let (_d, db, run, mem) = scenario();
        run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        let hashed: u64 = db.valid_hashes(run).unwrap().len() as u64;
        assert_eq!(hashed, 3, "A, B and C needed a full hash (matching head); D did not");
        // 4 prefixes + 3 full files; never D in full.
        assert_eq!(*mem.served.borrow(), 4 * HEAD_BYTES + 3 * 200 * 1024);
    }

    #[test]
    fn hashing_is_resumable_and_never_repeats_work() {
        let (_d, db, run, mem) = scenario();
        run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        let first = *mem.served.borrow();
        let again = run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        assert_eq!(again.files_hashed, 0, "known hashes are reused");
        assert!(*mem.served.borrow() - first <= HEAD_BYTES * 4, "only prefixes are read again, never whole files");
    }

    #[test]
    fn the_byte_budget_stops_the_run_and_reports_it() {
        let (_d, db, run, mem) = scenario();
        let tight = VerifyOptions { max_bytes: 300 * 1024, ..opts() };
        let summary = run_verify(&db, run, &mem, &tight, &AtomicBool::new(false));
        assert!(summary.budget_exhausted);
        assert!(summary.bytes_read <= 300 * 1024);
        assert!(summary.stopped.is_some());
        assert!(duplicate_report(&db, run, 1).unwrap().confirmed.is_empty(), "no false confirmation from a partial run");
    }

    #[test]
    fn cancellation_stops_before_reading() {
        let (_d, db, run, mem) = scenario();
        let summary = run_verify(&db, run, &mem, &opts(), &AtomicBool::new(true));
        assert!(summary.cancelled);
        assert_eq!(*mem.served.borrow(), 0);
        assert!(db.valid_hashes(run).unwrap().is_empty());
    }

    #[test]
    fn running_on_battery_pauses_the_job_when_asked_to() {
        let (_d, db, run, mem) = scenario();
        let o = VerifyOptions { stop_on_battery: true, ..opts() };
        let summary = verify_duplicates(&db, run, &mem, &o, &AtomicBool::new(false), &|| true, &Progress::default()).unwrap();
        assert!(summary.stopped.unwrap().contains("battery"));
        assert_eq!(*mem.served.borrow(), 0, "no disk reads on battery");
    }

    #[test]
    fn a_file_that_changed_is_never_hashed_into_a_confirmation() {
        let (_d, db, run, mem) = scenario();
        mem.changed.borrow_mut().insert(B.to_string());
        run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        let report = duplicate_report(&db, run, 1).unwrap();
        assert!(report.confirmed.is_empty(), "B was skipped, so A has nothing to match: {report:?}");
    }

    #[test]
    fn protected_and_excluded_files_are_never_read_or_listed() {
        let len = 200 * 1024u64;
        let data = body(5, len as usize);
        let ex = "C:\\Users\\me\\Private\\x.bin";
        let proj = "C:\\Users\\me\\code\\node_modules\\y.bin";
        let ok1 = "C:\\Users\\me\\Downloads\\1.bin";
        let ok2 = "C:\\Users\\me\\Downloads\\2.bin";
        let (_d, db, run) = seeded(vec![file(ex, len, 40), file(proj, len, 40), file(ok1, len, 40), file(ok2, len, 40)]);
        crate::storage_policy::add_exclusion(&db, "C:\\Users\\me\\Private").unwrap();
        let mem = Memory::new(&[(ex, data.clone()), (proj, data.clone()), (ok1, data.clone()), (ok2, data)]);
        run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        assert_eq!(*mem.served.borrow(), 2 * HEAD_BYTES + 2 * len, "only the two ordinary files were read");
        let report = duplicate_report(&db, run, 1).unwrap();
        assert_eq!(report.confirmed.len(), 1);
        assert_eq!(report.confirmed[0].files.len(), 2);
        assert_eq!(report.protected_files, 2);
    }

    #[test]
    fn a_stale_hash_is_ignored_when_the_file_changed() {
        let (_d, db, run, mem) = scenario();
        run_verify(&db, run, &mem, &opts(), &AtomicBool::new(false));
        let b = db.file_entry_by_path(run, B).unwrap().unwrap();
        db.save_duplicate_hash(run, b.id, "deadbeef", b.size, Some(1), 1).unwrap();
        assert!(duplicate_report(&db, run, 1).unwrap().confirmed.is_empty());
    }

    #[test]
    fn options_are_bounded() {
        let o = VerifyOptions { min_size: 0, max_bytes: u64::MAX, max_files: 1_000_000, stop_on_battery: true }.clamped();
        assert!(o.min_size >= 4096 && o.max_bytes <= MAX_BYTES_CEILING && o.max_files <= 2000);
    }
}
