use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use serde::Serialize;
use tauri::State;

use crate::{
    cleanup::{
        build_cleanup_plan, execute_cleanup_plan, restore_cleanup_action as restore_cleanup,
        CleanupPlan,
    },
    db::{
        CategorySummary, CleanupActionRecord, Database, DirectoryChild, ExtensionSummary,
        FileSearchResult, ScanRunRecord, TopDirectory,
    },
    reports::{write_diagnostics_report, write_scan_report, ExportArtifact},
    scan::{choose_scan_plan, run_recursive_scan, ScanProgress, ScanRunOptions, ScanState},
    volume::VolumeInfo,
};

pub struct AppState {
    db: Database,
    app_data_dir: PathBuf,
    progress: Arc<Mutex<ScanProgress>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppHealth {
    pub app_version: String,
    pub database_path: String,
    pub schema_version: Option<String>,
    pub latest_completed_run_id: Option<i64>,
    pub active_scan_state: ScanState,
    pub pending_action_count: u64,
    pub local_diagnostics_only: bool,
    pub recycle_bin_cleanup: bool,
}

impl AppState {
    pub fn new(db: Database, app_data_dir: PathBuf) -> Self {
        Self {
            db,
            app_data_dir,
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
pub fn scan_runs(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<ScanRunRecord>, String> {
    state
        .db
        .list_scan_runs(limit.unwrap_or(25))
        .map_err(to_command_error)
}

#[tauri::command]
pub fn top_directories(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<TopDirectory>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };

    state
        .db
        .top_directories(run_id, limit.unwrap_or(25).clamp(1, 100))
        .map_err(to_command_error)
}

#[tauri::command]
pub fn directory_children(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    parent_path: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<DirectoryChild>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    state
        .db
        .directory_children(run_id, parent_path.as_deref(), limit.unwrap_or(200))
        .map_err(to_command_error)
}

#[tauri::command]
pub fn file_search(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    query: Option<String>,
    extension: Option<String>,
    min_size: Option<u64>,
    limit: Option<u32>,
) -> Result<Vec<FileSearchResult>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    state
        .db
        .file_search(
            run_id,
            query.as_deref(),
            extension.as_deref(),
            min_size,
            limit.unwrap_or(100),
        )
        .map_err(to_command_error)
}

#[tauri::command]
pub fn extension_summary(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<ExtensionSummary>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    state
        .db
        .extension_summary(run_id, limit.unwrap_or(25))
        .map_err(to_command_error)
}

#[tauri::command]
pub fn category_summary(
    state: State<'_, AppState>,
    run_id: Option<i64>,
) -> Result<Vec<CategorySummary>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    state.db.category_summary(run_id).map_err(to_command_error)
}

#[tauri::command]
pub fn cleanup_plan(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    limit: Option<u32>,
) -> Result<Option<CleanupPlan>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    build_cleanup_plan(&state.db, run_id, limit.unwrap_or(100))
        .map(Some)
        .map_err(to_command_error)
}

#[tauri::command]
pub fn execute_cleanup(
    state: State<'_, AppState>,
    run_id: i64,
    candidate_ids: Vec<String>,
) -> Result<crate::cleanup::CleanupExecutionResult, String> {
    if candidate_ids.is_empty() {
        return Err("Select at least one cleanup candidate.".to_string());
    }
    execute_cleanup_plan(&state.db, run_id, &candidate_ids).map_err(to_command_error)
}

#[tauri::command]
pub fn cleanup_history(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<CleanupActionRecord>, String> {
    state
        .db
        .cleanup_history(limit.unwrap_or(50))
        .map_err(to_command_error)
}

#[tauri::command]
pub fn restore_cleanup_action(
    state: State<'_, AppState>,
    action_id: i64,
) -> Result<CleanupActionRecord, String> {
    restore_cleanup(&state.db, action_id).map_err(to_command_error)
}

#[tauri::command]
pub fn export_report(
    state: State<'_, AppState>,
    run_id: Option<i64>,
) -> Result<Option<ExportArtifact>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    let export_dir = state.app_data_dir.join("exports");
    write_scan_report(&state.db, run_id, &export_dir)
        .map(Some)
        .map_err(to_command_error)
}

#[tauri::command]
pub fn export_diagnostics(state: State<'_, AppState>) -> Result<ExportArtifact, String> {
    let export_dir = state.app_data_dir.join("diagnostics");
    write_diagnostics_report(&state.db, env!("CARGO_PKG_VERSION"), &export_dir)
        .map_err(to_command_error)
}

#[tauri::command]
pub fn app_health(state: State<'_, AppState>) -> Result<AppHealth, String> {
    let active_scan_state = state
        .progress
        .lock()
        .map(|progress| progress.state.clone())
        .map_err(|_| "scan status lock is poisoned".to_string())?;

    Ok(AppHealth {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        database_path: state.db.path().display().to_string(),
        schema_version: state.db.schema_version().map_err(to_command_error)?,
        latest_completed_run_id: state
            .db
            .latest_completed_run_id()
            .map_err(to_command_error)?,
        active_scan_state,
        pending_action_count: state.db.pending_action_count().map_err(to_command_error)?,
        local_diagnostics_only: true,
        recycle_bin_cleanup: true,
    })
}

fn resolve_run_id(db: &Database, run_id: Option<i64>) -> Result<Option<i64>, String> {
    match run_id {
        Some(run_id) => Ok(Some(run_id)),
        None => db.latest_completed_run_id().map_err(to_command_error),
    }
}

fn to_command_error(error: anyhow::Error) -> String {
    error.to_string()
}
