//! Tauri commands for the AI-assisted storage workflow: deterministic
//! evaluation, proposal validation, duplicate verification and the
//! "never touch this folder" list. Every command is read-only except the
//! exclusion list; nothing here deletes or moves a file.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde::Serialize;
use tauri::State;

use crate::{
    commands::AppState,
    duplicates::{duplicate_report, verify_duplicates, DiskAccess, DuplicateReport, Progress, VerifyOptions, VerifySummary, DEFAULT_MIN_SIZE},
    storage_ai::{self, ProposalInput, StorageEvaluation, StorageQuery, ValidatedProposal},
    storage_policy,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateJobStatus {
    pub state: JobState,
    pub run_id: Option<i64>,
    pub files_hashed: u32,
    pub bytes_read: u64,
    pub summary: Option<VerifySummary>,
    pub error: Option<String>,
}

/// The one duplicate-verification job that may run at a time.
#[derive(Default)]
pub struct StorageJobs {
    status: Arc<Mutex<DuplicateJobStatus>>,
    progress: Arc<Progress>,
    cancel: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl StorageJobs {
    fn snapshot(&self) -> DuplicateJobStatus {
        let mut status = lock(&self.status).clone();
        if status.state == JobState::Running {
            status.files_hashed = self.progress.files_hashed.load(Ordering::Relaxed);
            status.bytes_read = self.progress.bytes_read.load(Ordering::Relaxed);
        }
        status
    }
}

fn run_or_latest(state: &AppState, run_id: Option<i64>) -> Result<Option<i64>, String> {
    match run_id {
        Some(id) => Ok(Some(id)),
        None => state.db.latest_completed_run_id().map_err(|e| e.to_string()),
    }
}

fn on_battery() -> bool {
    matches!(crate::optimizer::power::system_power(), Ok(p) if p.ac_known && !p.on_ac)
}

/// WinClean's verified view of a scan: what may be cleaned or offloaded, what needs
/// judgment, what is protected, and optionally a selection that meets a goal.
#[tauri::command(async)]
pub fn storage_evaluate(state: State<'_, AppState>, run_id: Option<i64>, query: Option<StorageQuery>) -> Result<Option<StorageEvaluation>, String> {
    let Some(run_id) = run_or_latest(&state, run_id)? else {
        return Ok(None);
    };
    storage_ai::evaluate(&state.db, run_id, &query.unwrap_or_default()).map(Some).map_err(|e| e.to_string())
}

/// Resolve model proposals against WinClean's own candidates. A model only supplies
/// candidate ids; sizes, actions and files are recomputed and re-checked on disk here.
#[tauri::command(async)]
pub fn storage_validate_proposals(state: State<'_, AppState>, run_id: i64, proposals: Vec<ProposalInput>) -> Result<Vec<ValidatedProposal>, String> {
    let db = state.db.clone();
    let check = move |item: &storage_ai::EvaluatedItem| {
        let modified = db.file_entry_by_path(run_id, &item.path).ok().flatten().and_then(|e| e.modified_at_ms);
        storage_ai::disk_check(item, modified)
    };
    storage_ai::validate_proposals(&state.db, run_id, &proposals, &check).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn duplicate_report_for(state: State<'_, AppState>, run_id: Option<i64>, min_size: Option<u64>) -> Result<Option<DuplicateReport>, String> {
    let Some(run_id) = run_or_latest(&state, run_id)? else {
        return Ok(None);
    };
    duplicate_report(&state.db, run_id, min_size.unwrap_or(DEFAULT_MIN_SIZE)).map(Some).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn duplicates_status(jobs: State<'_, StorageJobs>) -> DuplicateJobStatus {
    jobs.snapshot()
}

#[tauri::command]
pub fn duplicates_cancel(jobs: State<'_, StorageJobs>) -> DuplicateJobStatus {
    jobs.cancel.store(true, Ordering::Relaxed);
    jobs.snapshot()
}

/// Compare same-size files by content. Reads only, within a byte/file budget, at
/// background I/O priority, and stops on battery. Started by an explicit click.
#[tauri::command]
pub fn duplicates_start(
    state: State<'_, AppState>,
    jobs: State<'_, StorageJobs>,
    run_id: Option<i64>,
    min_size: Option<u64>,
    max_bytes: Option<u64>,
    max_files: Option<usize>,
) -> Result<DuplicateJobStatus, String> {
    let Some(run_id) = run_or_latest(&state, run_id)? else {
        return Err("Run a scan first.".to_string());
    };
    {
        let mut status = lock(&jobs.status);
        if status.state == JobState::Running {
            return Err("A duplicate check is already running.".to_string());
        }
        *status = DuplicateJobStatus { state: JobState::Running, run_id: Some(run_id), ..Default::default() };
    }
    jobs.cancel.store(false, Ordering::Relaxed);
    jobs.progress.files_hashed.store(0, Ordering::Relaxed);
    jobs.progress.bytes_read.store(0, Ordering::Relaxed);

    let mut options = VerifyOptions::default();
    if let Some(v) = min_size {
        options.min_size = v;
    }
    if let Some(v) = max_bytes {
        options.max_bytes = v;
    }
    if let Some(v) = max_files {
        options.max_files = v;
    }

    let db = state.db.clone();
    let (status, progress, cancel) = (jobs.status.clone(), jobs.progress.clone(), jobs.cancel.clone());
    std::thread::spawn(move || {
        crate::duplicates::enter_background_io();
        let result = verify_duplicates(&db, run_id, &DiskAccess, &options, &cancel, &on_battery, &progress);
        let mut status = lock(&status);
        status.files_hashed = progress.files_hashed.load(Ordering::Relaxed);
        status.bytes_read = progress.bytes_read.load(Ordering::Relaxed);
        match result {
            Ok(summary) => {
                status.state = JobState::Completed;
                status.summary = Some(summary);
            }
            Err(error) => {
                status.state = JobState::Failed;
                status.error = Some(error.to_string());
            }
        }
    });
    Ok(jobs.snapshot())
}

#[tauri::command]
pub fn storage_exclusions(state: State<'_, AppState>) -> Vec<String> {
    storage_policy::exclusions(&state.db)
}

#[tauri::command]
pub fn storage_add_exclusion(state: State<'_, AppState>, path: String) -> Result<Vec<String>, String> {
    storage_policy::add_exclusion(&state.db, &path)
}

#[tauri::command]
pub fn storage_remove_exclusion(state: State<'_, AppState>, path: String) -> Result<Vec<String>, String> {
    storage_policy::remove_exclusion(&state.db, &path)
}
