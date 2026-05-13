use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use tauri::State;

use crate::{
    db::{Database, TopDirectory},
    scan::{choose_scan_plan, run_recursive_scan, ScanProgress, ScanRunOptions, ScanState},
    volume::VolumeInfo,
};

pub struct AppState {
    db: Database,
    progress: Arc<Mutex<ScanProgress>>,
    cancel: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            progress: Arc::new(Mutex::new(ScanProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[tauri::command]
pub fn list_volumes() -> Result<Vec<VolumeInfo>, String> {
    Ok(crate::volume::list_volumes())
}

#[tauri::command]
pub fn scan_status(state: State<'_, AppState>) -> Result<ScanProgress, String> {
    state
        .progress
        .lock()
        .map(|progress| progress.clone())
        .map_err(|_| "scan status lock is poisoned".to_string())
}

#[tauri::command]
pub fn start_scan(
    state: State<'_, AppState>,
    root_path: String,
    file_system: Option<String>,
) -> Result<ScanProgress, String> {
    {
        let progress = state
            .progress
            .lock()
            .map_err(|_| "scan status lock is poisoned".to_string())?;
        if matches!(progress.state, ScanState::Running) {
            return Err("A scan is already running.".to_string());
        }
    }

    let plan = choose_scan_plan(&root_path, file_system.as_deref());
    state.cancel.store(false, Ordering::Relaxed);
    {
        let mut progress = state
            .progress
            .lock()
            .map_err(|_| "scan status lock is poisoned".to_string())?;
        *progress = ScanProgress {
            state: ScanState::Running,
            run_id: None,
            root_path: Some(root_path.clone()),
            mode: Some(plan.mode),
            attempted_mode: plan.attempted_mode,
            fallback_reason: plan.fallback_reason.clone(),
            files_indexed: 0,
            directories_indexed: 0,
            bytes_indexed: 0,
            access_denied_count: 0,
            skipped_reparse_count: 0,
            error_count: 0,
            last_error: None,
        };
    }

    let db = state.db.clone();
    let progress = Arc::clone(&state.progress);
    let cancel = Arc::clone(&state.cancel);
    let scan_root = PathBuf::from(root_path);

    thread::spawn(move || {
        let options = ScanRunOptions {
            file_system,
            attempted_mode: plan.attempted_mode,
            fallback_reason: plan.fallback_reason,
            cancel: Some(cancel),
            progress: Some(Arc::clone(&progress)),
        };

        if let Err(error) = run_recursive_scan(&db, &scan_root, Some(options)) {
            if let Ok(mut progress) = progress.lock() {
                progress.state = ScanState::Failed;
                progress.last_error = Some(error.to_string());
            }
        }
    });

    scan_status(state)
}

#[tauri::command]
pub fn cancel_scan(state: State<'_, AppState>) -> Result<ScanProgress, String> {
    state.cancel.store(true, Ordering::Relaxed);
    scan_status(state)
}

#[tauri::command]
pub fn top_directories(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<TopDirectory>, String> {
    let run_id = match run_id {
        Some(run_id) => Some(run_id),
        None => state
            .db
            .latest_completed_run_id()
            .map_err(to_command_error)?,
    };

    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };

    state
        .db
        .top_directories(run_id, limit.unwrap_or(25).clamp(1, 100))
        .map_err(to_command_error)
}

fn to_command_error(error: anyhow::Error) -> String {
    error.to_string()
}
