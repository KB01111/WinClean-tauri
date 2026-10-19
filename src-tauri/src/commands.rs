use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::{
    cleanup::{build_cleanup_plan, execute_cleanup_plan, CleanupPlan},
    db::{
        CategorySummary, CleanupActionRecord, Database, DirectoryChild, ExtensionSummary,
        FileSearchResult, OffloadItemRecord, ScanRunRecord, TopDirectory,
    },
    offload::{
        active_offload_job as read_active_offload_job,
        begin_google_drive_auth as begin_drive_auth, create_offload_job,
        disconnect_google_drive as disconnect_drive, google_drive_status as drive_status,
        list_offload_jobs as read_offload_jobs, offload_status as read_offload_status,
        process_offload_job_with_sink, restore_offload_item as restore_item, GoogleDriveClient,
        GoogleDriveStatus, OffloadJobStatus, WindowsRecycleBin,
    },
    recommendations::{
        build_recommendations, resolve_offload_recommendation_candidates, RecommendationFilters,
        RecommendationPlan,
    },
    reports::{write_diagnostics_report, write_scan_report, ExportArtifact},
    scan::{choose_scan_plan, run_recursive_scan, ScanProgress, ScanRunOptions, ScanState},
    volume::{resolve_local_scan_root, VolumeInfo},
};

pub struct AppState {
    db: Database,
    app_data_dir: PathBuf,
    progress: Arc<Mutex<ScanProgress>>,
    cancel: Arc<AtomicBool>,
    offload_cancels: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>,
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
    pub pending_offload_count: u64,
    pub local_diagnostics_only: bool,
    pub recycle_bin_cleanup: bool,
    pub google_drive_configured: bool,
    pub google_drive_connected: bool,
}

impl AppState {
    pub fn new(db: Database, app_data_dir: PathBuf) -> Self {
        Self {
            db,
            app_data_dir,
            progress: Arc::new(Mutex::new(ScanProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            offload_cancels: Arc::new(Mutex::new(HashMap::new())),
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
    app: AppHandle,
    state: State<'_, AppState>,
    root_path: String,
    file_system: Option<String>,
) -> Result<ScanProgress, String> {
    let _renderer_file_system = file_system;
    let selected_volume = resolve_local_scan_root(&root_path, &crate::volume::list_volumes())?;
    let root_path = selected_volume.root_path;
    let trusted_file_system = selected_volume.file_system;
    let plan = choose_scan_plan(&root_path, trusted_file_system.as_deref());
    begin_scan_progress(&state.progress, &root_path, &plan)?;
    state.cancel.store(false, Ordering::Relaxed);

    let db = state.db.clone();
    let progress = Arc::clone(&state.progress);
    let cancel = Arc::clone(&state.cancel);
    let scan_root = PathBuf::from(root_path);
    let handle = app.clone();

    thread::spawn(move || {
        let sink_handle = handle.clone();
        let options = ScanRunOptions {
            file_system: trusted_file_system,
            attempted_mode: plan.attempted_mode,
            fallback_reason: plan.fallback_reason,
            cancel: Some(cancel),
            progress: Some(Arc::clone(&progress)),
            progress_sink: Some(Arc::new(move |progress: &ScanProgress| {
                let _ = sink_handle.emit("scan:progress", progress);
            })),
        };

        if let Err(error) = run_recursive_scan(&db, &scan_root, Some(options)) {
            if let Ok(mut progress) = progress.lock() {
                progress.state = ScanState::Failed;
                progress.last_error = Some(error.to_string());
                progress.phase = Some("failed".to_string());
                let _ = handle.emit("scan:progress", &*progress);
            }
        }
    });

    scan_status(state)
}

pub fn begin_scan_progress(
    progress: &Mutex<ScanProgress>,
    root_path: &str,
    plan: &crate::scan::ScanPlan,
) -> Result<(), String> {
    let mut progress = progress
        .lock()
        .map_err(|_| "scan status lock is poisoned".to_string())?;
    if matches!(progress.state, ScanState::Running) {
        return Err("A scan is already running.".to_string());
    }

    *progress = ScanProgress {
        state: ScanState::Running,
        run_id: None,
        root_path: Some(root_path.to_string()),
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
        worker_count: None,
        phase: Some("starting".to_string()),
    };

    Ok(())
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
pub fn recommendations(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    filters: Option<RecommendationFilters>,
) -> Result<Option<RecommendationPlan>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    build_recommendations(&state.db, run_id, filters.unwrap_or_default())
        .map(Some)
        .map_err(to_command_error)
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
pub fn google_drive_status() -> Result<GoogleDriveStatus, String> {
    Ok(drive_status())
}

#[tauri::command]
pub fn begin_google_drive_auth() -> Result<GoogleDriveStatus, String> {
    begin_drive_auth().map_err(to_command_error)
}

#[tauri::command]
pub fn disconnect_google_drive() -> Result<GoogleDriveStatus, String> {
    disconnect_drive().map_err(to_command_error)
}

#[tauri::command]
pub fn start_offload(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: i64,
    candidate_ids: Vec<String>,
) -> Result<OffloadJobStatus, String> {
    if candidate_ids.is_empty() {
        return Err("Select at least one offload candidate.".to_string());
    }
    if state
        .db
        .latest_active_offload_job_id()
        .map_err(to_command_error)?
        .is_some()
    {
        return Err("An offload batch is already running. Wait for it to finish.".to_string());
    }
    let plan = build_recommendations(&state.db, run_id, RecommendationFilters::default())
        .map_err(to_command_error)?;
    let candidates = resolve_offload_recommendation_candidates(&plan.candidates, &candidate_ids)
        .map_err(to_command_error)?;

    let drive = GoogleDriveClient::configured().map_err(to_command_error)?;
    if !drive_status().connected {
        return Err("Connect Google Drive before starting an offload.".to_string());
    }
    let recycler = WindowsRecycleBin;
    let initial = create_offload_job(&state.db, run_id, &candidates).map_err(to_command_error)?;
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .offload_cancels
        .lock()
        .map_err(|_| "offload status lock is poisoned".to_string())?
        .insert(initial.id, Arc::clone(&cancel));

    let db = state.db.clone();
    let cancels = Arc::clone(&state.offload_cancels);
    let job_id = initial.id;
    let handle = app.clone();
    thread::spawn(move || {
        let _ = process_offload_job_with_sink(
            &db,
            job_id,
            candidates,
            &drive,
            &recycler,
            &cancel,
            Some(Arc::new(move |status: &OffloadJobStatus| {
                let _ = handle.emit("offload:progress", status);
            })),
        );
        if let Ok(mut cancels) = cancels.lock() {
            cancels.remove(&job_id);
        }
    });
    Ok(initial)
}

#[tauri::command]
pub fn offload_status(state: State<'_, AppState>, job_id: i64) -> Result<OffloadJobStatus, String> {
    read_offload_status(&state.db, job_id).map_err(to_command_error)
}

#[tauri::command]
pub fn cancel_offload(state: State<'_, AppState>, job_id: i64) -> Result<OffloadJobStatus, String> {
    if let Some(cancel) = state
        .offload_cancels
        .lock()
        .map_err(|_| "offload status lock is poisoned".to_string())?
        .get(&job_id)
    {
        cancel.store(true, Ordering::Relaxed);
    }
    read_offload_status(&state.db, job_id).map_err(to_command_error)
}

#[tauri::command]
pub fn list_offload_jobs(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<OffloadJobStatus>, String> {
    read_offload_jobs(&state.db, limit.unwrap_or(20)).map_err(to_command_error)
}

#[tauri::command]
pub fn active_offload_job(state: State<'_, AppState>) -> Result<Option<OffloadJobStatus>, String> {
    read_active_offload_job(&state.db).map_err(to_command_error)
}

#[tauri::command]
pub fn restore_offload_item(
    state: State<'_, AppState>,
    item_id: i64,
    target_dir: Option<String>,
) -> Result<OffloadItemRecord, String> {
    let drive = GoogleDriveClient::configured().map_err(to_command_error)?;
    if !drive_status().connected {
        return Err("Connect Google Drive before restoring files.".to_string());
    }
    restore_item(&state.db, item_id, &drive, target_dir).map_err(to_command_error)
}

#[tauri::command]
pub fn largest_stale_files(
    state: State<'_, AppState>,
    run_id: Option<i64>,
    min_size: Option<u64>,
    min_age_days: Option<u64>,
    limit: Option<u32>,
) -> Result<Vec<FileSearchResult>, String> {
    let run_id = resolve_run_id(&state.db, run_id)?;
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    state
        .db
        .largest_stale_files(
            run_id,
            min_size.unwrap_or(50 * 1024 * 1024),
            min_age_days.unwrap_or(180),
            limit.unwrap_or(25),
        )
        .map_err(to_command_error)
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Only https links can be opened.".to_string());
    }
    open::that(&url).map_err(|error| format!("failed to open link: {error}"))
}

const AI_CREDENTIAL_SERVICE: &str = "WinClean AI";
const AI_API_KEY_USER: &str = "openai_api_key";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiKeyStatus {
    pub configured: bool,
}

fn ai_credential_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(AI_CREDENTIAL_SERVICE, AI_API_KEY_USER)
        .map_err(|error| format!("open Windows Credential Manager entry: {error}"))
}

#[tauri::command]
pub fn ai_key_status() -> Result<AiKeyStatus, String> {
    let configured = ai_credential_entry()
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .is_some();
    Ok(AiKeyStatus { configured })
}

#[tauri::command]
pub fn set_ai_api_key(api_key: String) -> Result<AiKeyStatus, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("API key cannot be empty.".to_string());
    }
    ai_credential_entry()?
        .set_password(trimmed)
        .map_err(|error| format!("store AI API key: {error}"))?;
    ai_key_status()
}

#[tauri::command]
pub fn clear_ai_api_key() -> Result<AiKeyStatus, String> {
    if let Ok(entry) = ai_credential_entry() {
        let _ = entry.delete_credential();
    }
    ai_key_status()
}

#[tauri::command]
pub fn get_ai_api_key() -> Result<Option<String>, String> {
    Ok(ai_credential_entry()
        .ok()
        .and_then(|entry| entry.get_password().ok()))
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
pub fn open_recycle_bin() -> Result<(), String> {
    open_recycle_bin_impl().map_err(to_command_error)
}

#[tauri::command]
pub fn delete_scan_run(state: State<'_, AppState>, run_id: i64) -> Result<bool, String> {
    let active_run_id = state
        .progress
        .lock()
        .map(|progress| {
            if matches!(progress.state, ScanState::Running) {
                progress.run_id
            } else {
                None
            }
        })
        .map_err(|_| "scan status lock is poisoned".to_string())?;
    if active_run_id == Some(run_id) {
        return Err("Cannot delete a scan while it is running.".to_string());
    }

    state.db.delete_scan_run(run_id).map_err(to_command_error)
}

#[tauri::command]
pub fn clear_scan_history(state: State<'_, AppState>) -> Result<usize, String> {
    state.db.clear_scan_history().map_err(to_command_error)
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
    let drive = drive_status();

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
        pending_offload_count: state.db.pending_offload_count().map_err(to_command_error)?,
        local_diagnostics_only: true,
        recycle_bin_cleanup: true,
        google_drive_configured: drive.configured,
        google_drive_connected: drive.connected,
    })
}

#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    reveal_in_explorer_impl(&path)
}

#[cfg(windows)]
fn reveal_in_explorer_impl(path: &str) -> Result<(), String> {
    if !std::path::Path::new(path).exists() {
        return Err(format!("path does not exist: {path}"));
    }
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{path}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open Explorer: {e}"))
}

#[cfg(not(windows))]
fn reveal_in_explorer_impl(_path: &str) -> Result<(), String> {
    Err("Reveal in Explorer is only available on Windows.".into())
}
// --- WinClean 2.0 optimizer commands ---
// Every mutation: policy check -> snapshot -> pending audit -> apply ->
// verify by re-read -> completed audit. Failures keep the audit row.

use crate::optimizer::{
    capabilities::capability_report,
    hardware::hardware_info,
    policy::{classify_service, is_protected_service, EcoQosDecision, ServiceRisk},
    power::{battery_status, list_profiles, set_active_scheme},
    processes::{list_all_processes, list_processes, parse_process_target, process_ecoqos_state, process_liveness, process_target, set_process_ecoqos, ProcessLiveness},
    services::{list_services, set_service_start_type, snapshot_payload, start_type_from_snapshot, StartType},
    OptimizationAudit, OptimizationSnapshot,
};

/// Serializes optimizer mutations (profiles, rollbacks, EcoQoS, services) so
/// overlapping requests cannot snapshot a stale scheme or complete their
/// audit rows out of order. Reads never take it.
static OPTIMIZER_MUTATION: Mutex<()> = Mutex::new(());

fn lock_optimizer_mutations() -> std::sync::MutexGuard<'static, ()> {
    OPTIMIZER_MUTATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Enumeration/mutation work runs on a blocking thread, never the UI thread.
#[tauri::command(async)]
pub async fn optimizer_list_services() -> Result<Vec<crate::optimizer::services::ServiceInfo>, String> {
    tauri::async_runtime::spawn_blocking(list_services).await.map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub async fn optimizer_set_service_start_type(
    state: State<'_, AppState>,
    name: String,
    display_name: String,
    start_type: String,
) -> Result<crate::optimizer::services::ServiceInfo, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || set_service_start_type_blocking(&db, &name, &display_name, &start_type))
        .await.map_err(|e| e.to_string())?
}

fn set_service_start_type_blocking(db: &Database, name: &str, display_name: &str, start_type: &str) -> Result<crate::optimizer::services::ServiceInfo, String> {
    let _guard = lock_optimizer_mutations();
    if is_protected_service(name, display_name) {
        return Err(format!("'{display_name}' is protected (essential Windows/security/driver service) and cannot be changed by WinClean."));
    }
    let target = StartType::parse(start_type).ok_or_else(|| format!("unknown start type: {start_type}"))?;
    let services = list_services()?;
    let current = services.iter().find(|s| s.name == name)
        .ok_or_else(|| format!("service not found: {name}"))?;
    let classification = classify_service(&current.name, &current.display_name);
    if classification.risk == ServiceRisk::Protected {
        return Err(format!("'{}' is protected and cannot be changed by WinClean.", current.display_name));
    }
    if !crate::optimizer::policy::start_type_allowed(&classification, target == StartType::Disabled) {
        return Err(format!("'{}' is an unknown service; WinClean can switch it between Automatic/Manual but will not disable an unlisted service.", current.display_name));
    }
    // Refuse the mutation when the current state could not be restored.
    if start_type_from_snapshot(&snapshot_payload(current)).is_none() {
        return Err(format!("current start type '{}' of '{}' cannot be snapshotted for rollback; refusing to change it.", current.start_type, current.display_name));
    }
    let snapshot_id = db.insert_optimizer_snapshot("service", name, &snapshot_payload(current).to_string()).map_err(to_command_error)?;
    let audit_id = db.insert_optimizer_audit("service", name, &format!("setStartType:{start_type}"), "pending", None, Some(snapshot_id)).map_err(to_command_error)?;
    match set_service_start_type(name, target) {
        Ok(_updated) => {
            // Verify against the *requested* target, not the setter's echo.
            let verified = list_services().ok().and_then(|all| all.into_iter().find(|s| s.name == name));
            match verified {
                Some(v) if v.start_type == target.as_str() => {
                    let detail = format!("verified startType={}", v.start_type);
                    let _ = db.complete_optimizer_audit(audit_id, "completed", Some(&detail));
                    Ok(v)
                }
                Some(v) => {
                    let e = format!("SCM reports startType={} after change, expected {}", v.start_type, target.as_str());
                    let _ = db.complete_optimizer_audit(audit_id, "failed", Some(&e));
                    Err(e)
                }
                None => {
                    let e = "could not re-read service from SCM to verify the change".to_string();
                    let _ = db.complete_optimizer_audit(audit_id, "failed", Some(&e));
                    Err(e)
                }
            }
        }
        Err(e) => {
            let _ = db.complete_optimizer_audit(audit_id, "failed", Some(&e));
            Err(e)
        }
    }
}

#[tauri::command(async)]
pub async fn optimizer_list_processes() -> Result<Vec<crate::optimizer::processes::ProcessInfo>, String> {
    tauri::async_runtime::spawn_blocking(list_processes).await.map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub async fn optimizer_set_process_ecoqos(
    state: State<'_, AppState>,
    pid: u32,
    name: String,
    enable: bool,
) -> Result<(), String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = lock_optimizer_mutations();
        let action = if enable { "ecoqos:on" } else { "ecoqos:off" };
        let audit_id = db.insert_optimizer_audit("process", &process_target(&name, pid), action, "pending", None, None).map_err(to_command_error)?;
        // The setter revalidates the PID's real image name, foreground
        // state and live policy verdict before enabling; restore (off)
        // always passes identity revalidation only.
        match set_process_ecoqos(pid, &name, None, enable) {
            Ok(real_name) => {
                let _ = db.complete_optimizer_audit(audit_id, "completed", Some(&format!("verified process: {real_name}")));
                Ok(())
            }
            Err(e) => {
                let _ = db.complete_optimizer_audit(audit_id, "failed", Some(&e));
                Err(e)
            }
        }
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn optimizer_battery_status(state: State<'_, AppState>) -> Result<crate::optimizer::power::BatteryStatus, String> {
    let status = battery_status()?;
    if let Some(p) = status.percent {
        // Unknown AC state is stored as DC=false sample only when known;
        // record the sample with the known flag encoded via on_ac only
        // when ac_known, otherwise skip sampling entirely.
        if status.ac_known {
            let _ = state.db.record_power_sample(p, status.on_ac);
        }
    }
    Ok(status)
}

#[tauri::command]
pub fn optimizer_power_history(state: State<'_, AppState>, limit: Option<u32>) -> Result<Vec<crate::optimizer::power::PowerSample>, String> {
    state.db.power_history(limit.unwrap_or(200).clamp(1, 2000)).map_err(to_command_error)
}

#[tauri::command(async)]
pub async fn optimizer_apply_power_profile(state: State<'_, AppState>, profile_id: String) -> Result<String, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || apply_power_profile_blocking(&db, &profile_id))
        .await.map_err(|e| e.to_string())?
}

fn apply_power_profile_blocking(db: &Database, profile_id: &str) -> Result<String, String> {
    let _guard = lock_optimizer_mutations();
    let profiles = list_profiles();
    let profile = profiles.iter().find(|p| format!("{:?}", p.id).eq_ignore_ascii_case(profile_id)
        || p.label.eq_ignore_ascii_case(profile_id))
        .ok_or_else(|| format!("unknown power profile: {profile_id}"))?;
    let guid = profile.scheme_guid.clone().ok_or("profile has no Windows scheme GUID")?;
    // A restorable snapshot is mandatory: refuse when the current scheme
    // cannot be identified.
    let before = battery_status()?;
    let previous_guid = before.active_scheme_guid.clone()
        .ok_or_else(|| "could not determine the active power scheme; refusing to change it without a rollback snapshot".to_string())?;
    let mut payload = serde_json::json!({
        "profileId": format!("{:?}", profile.id),
        "schemeGuid": previous_guid,
        "schemeName": before.active_scheme_name.clone(),
        "ecoQosForBackground": profile.eco_qos_for_background,
    });
    let snapshot_id = db.insert_optimizer_snapshot("powerScheme", &profile.label, &payload.to_string()).map_err(to_command_error)?;
    let audit_id = db.insert_optimizer_audit("power", &profile.label, "applyProfile", "pending", None, Some(snapshot_id)).map_err(to_command_error)?;
    if let Err(e) = set_active_scheme(&guid) {
        let _ = db.complete_optimizer_audit(audit_id, "failed", Some(&e));
        return Err(e);
    }
    // Scheme is applied from here on; a failed verification re-read is
    // reported as applied-unverified, never as a failed change.
    let scheme_detail = match battery_status() {
        Ok(after) => {
            let verified = after.active_scheme_guid.as_deref() == Some(guid.as_str());
            format!("active scheme now {} ({})", after.active_scheme_name.clone().unwrap_or_default(), if verified { "verified" } else { "differs from requested GUID" })
        }
        Err(e) => format!("scheme applied; verification re-read failed: {e}"),
    };
    // Apply the profile's EcoQoS promise: throttle eligible background
    // processes, or restore the ones WinClean throttled earlier.
    let eco = apply_profile_ecoqos(db, profile.eco_qos_for_background, snapshot_id);
    // Record exactly which processes this profile changed so rollback can
    // undo them (the scheme GUID alone cannot).
    let key = if profile.eco_qos_for_background { "ecoQosEnabled" } else { "ecoQosRestored" };
    payload[key] = serde_json::json!(eco.touched);
    let snapshot_note = match db.update_optimizer_snapshot_payload(snapshot_id, &payload.to_string()) {
        Ok(()) => String::new(),
        Err(e) => format!("; WARNING: could not record processes for rollback: {e}"),
    };
    let detail = format!("{scheme_detail}; EcoQoS: {}{snapshot_note}", eco.summary);
    let _ = db.complete_optimizer_audit(audit_id, "completed", Some(&detail));
    Ok(detail)
}

/// A process whose EcoQoS state a power profile changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EcoQosTouched {
    pid: u32,
    name: String,
    /// When WinClean changed this process. Rollback treats a process created
    /// after this moment as a PID reuse; absent in older snapshots, where the
    /// snapshot time is used instead.
    #[serde(default, rename = "sinceMs")]
    since_ms: Option<i64>,
}

struct EcoQosOutcome {
    summary: String,
    touched: Vec<EcoQosTouched>,
    /// Processes that should have been restored but could not be (liveness
    /// unknown, access denied). A rollback with any of these stays retryable.
    incomplete: usize,
}

/// A process WinClean may own EcoQoS throttling for: `(name, pid, since_ms)`.
/// `since_ms` is when WinClean recorded the throttle; a process created
/// after it is a different process that reused the PID.
type EcoQosCandidate = (String, u32, i64);

fn apply_profile_ecoqos(db: &Database, enable: bool, snapshot_id: i64) -> EcoQosOutcome {
    if enable {
        return throttle_eligible_processes(db, snapshot_id);
    }
    match owned_ecoqos_candidates(db) {
        Ok(candidates) => restore_ecoqos(db, candidates, "via power profile", Some(snapshot_id)),
        Err(e) => EcoQosOutcome { summary: format!("could not read throttled processes: {e}"), touched: Vec::new(), incomplete: 0 },
    }
}

/// `snapshot_id` links a real change to the profile snapshot that can undo
/// it; releases and rollback rows pass `None` so they are never mistaken for
/// changes when a snapshot is rolled back again.
fn record_ecoqos_audit(db: &Database, name: &str, pid: u32, enable: bool, detail: &str, snapshot_id: Option<i64>) {
    let action = if enable { "ecoqos:on" } else { "ecoqos:off" };
    let _ = db.insert_optimizer_audit("process", &process_target(name, pid), action, "completed", Some(detail), snapshot_id);
}

/// Enables EcoQoS for every currently eligible process.
fn throttle_eligible_processes(db: &Database, snapshot_id: i64) -> EcoQosOutcome {
    let processes = match list_all_processes() {
        Ok(p) => p,
        Err(e) => return EcoQosOutcome { summary: format!("process listing failed: {e}"), touched: Vec::new(), incomplete: 0 },
    };
    let mut touched = Vec::new();
    let mut skipped = 0usize;
    let eligible = processes.iter().filter(|p| p.eco_decision == EcoQosDecision::Eligible && !p.eco_qos_applied);
    for proc in eligible {
        match set_process_ecoqos(proc.pid, &proc.name, None, true) {
            Ok(real) => {
                record_ecoqos_audit(db, &real, proc.pid, true, "via power profile", Some(snapshot_id));
                touched.push(EcoQosTouched { pid: proc.pid, name: real, since_ms: Some(now_ms()) });
            }
            Err(_) => skipped += 1,
        }
    }
    EcoQosOutcome { summary: format!("{} process(es) throttled, {skipped} skipped", touched.len()), touched, incomplete: 0 }
}

/// Processes the audit trail shows WinClean throttled and never restored.
/// Independent of the UI listing cap and of EcoQoS set by other apps.
fn owned_ecoqos_candidates(db: &Database) -> Result<Vec<EcoQosCandidate>, String> {
    let rows = db.ecoqos_owned_processes().map_err(to_command_error)?;
    Ok(rows.into_iter()
        .filter_map(|(target, at_ms)| parse_process_target(&target).map(|(name, pid)| (name, pid, at_ms)))
        .collect())
}

/// Restores EcoQoS for candidates that still identify the process WinClean
/// throttled. Exited processes and reused PIDs are released from ownership
/// without touching anything.
fn restore_ecoqos(db: &Database, candidates: Vec<EcoQosCandidate>, detail: &str, snapshot_id: Option<i64>) -> EcoQosOutcome {
    let (mut touched, mut released, mut skipped) = (Vec::new(), 0usize, 0usize);
    for (name, pid, since_ms) in candidates {
        match process_liveness(pid) {
            ProcessLiveness::Gone => {
                record_ecoqos_audit(db, &name, pid, false, "process had already exited", None);
                released += 1;
            }
            ProcessLiveness::Alive { started_ms } if started_ms > since_ms => {
                record_ecoqos_audit(db, &name, pid, false, "PID now belongs to a newer process; not throttled by WinClean", None);
                released += 1;
            }
            ProcessLiveness::Unknown => skipped += 1,
            // Another app (or an earlier restore) already cleared it: nothing
            // to undo, and it must not be recorded as a change WinClean made.
            ProcessLiveness::Alive { .. } if process_ecoqos_state(pid) == Some(false) => {
                record_ecoqos_audit(db, &name, pid, false, "EcoQoS was already off; left untouched", None);
                released += 1;
            }
            ProcessLiveness::Alive { .. } => match set_process_ecoqos(pid, &name, Some(since_ms), false) {
                Ok(real) => {
                    record_ecoqos_audit(db, &real, pid, false, detail, snapshot_id);
                    touched.push(EcoQosTouched { pid, name: real, since_ms: Some(since_ms) });
                }
                Err(_) => skipped += 1,
            },
        }
    }
    EcoQosOutcome { summary: format!("{} process(es) restored, {released} already gone or off, {skipped} skipped", touched.len()), touched, incomplete: skipped }
}

/// Re-applies EcoQoS to processes a profile had restored (rollback of a
/// profile that turned background throttling off). The setter still
/// enforces the current policy, so a process that is now busy or protected
/// stays untouched.
fn rethrottle_ecoqos(db: &Database, candidates: Vec<EcoQosCandidate>, detail: &str) -> EcoQosOutcome {
    let (mut touched, mut skipped) = (Vec::new(), 0usize);
    for (name, pid, since_ms) in candidates {
        match process_liveness(pid) {
            ProcessLiveness::Alive { started_ms } if started_ms <= since_ms && process_ecoqos_state(pid) == Some(true) => {}
            ProcessLiveness::Alive { started_ms } if started_ms <= since_ms => match set_process_ecoqos(pid, &name, Some(since_ms), true) {
                Ok(real) => {
                    record_ecoqos_audit(db, &real, pid, true, detail, None);
                    touched.push(EcoQosTouched { pid, name: real, since_ms: Some(since_ms) });
                }
                Err(_) => skipped += 1,
            },
            _ => skipped += 1,
        }
    }
    // Best effort: re-throttling only restores a nicety, so failures here do
    // not hold a rollback open (leaving a process throttled does).
    EcoQosOutcome { summary: format!("{} process(es) re-throttled, {skipped} skipped", touched.len()), touched, incomplete: 0 }
}

/// Undoes the EcoQoS side of an applied power profile from its snapshot.
fn rollback_profile_ecoqos(db: &Database, payload: &serde_json::Value, snapshot_id: i64, snapshot_created_ms: i64) -> Option<(String, usize)> {
    // The payload list is written once after the process loop; the per-process
    // audit rows are written as each process changes. Use both so a failed or
    // interrupted payload write cannot make the changes un-undoable.
    let list = |key: &str, action: &str| -> Result<Vec<EcoQosCandidate>, String> {
        let mut out: Vec<EcoQosCandidate> = payload.get(key)
            .and_then(|v| serde_json::from_value::<Vec<EcoQosTouched>>(v.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.name, t.pid, t.since_ms.unwrap_or(snapshot_created_ms)))
            .collect();
        for (target, at_ms) in db.ecoqos_changes_for_snapshot(snapshot_id, action).map_err(to_command_error)? {
            if let Some((name, pid)) = parse_process_target(&target) {
                if !out.iter().any(|(n, p, _)| *p == pid && n.eq_ignore_ascii_case(&name)) {
                    out.push((name, pid, at_ms));
                }
            }
        }
        Ok(out)
    };
    // An unreadable recovery record must keep the rollback retryable, never
    // look like "nothing to undo".
    let (enabled, restored) = match (list("ecoQosEnabled", "ecoqos:on"), list("ecoQosRestored", "ecoqos:off")) {
        (Ok(enabled), Ok(restored)) => (enabled, restored),
        (Err(e), _) | (_, Err(e)) => return Some((format!("could not read the processes this profile changed ({e}); EcoQoS left unchanged"), 1)),
    };
    if enabled.is_empty() && restored.is_empty() { return None; }
    // Only undo throttles WinClean still owns. A later manual Restore, or
    // EcoQoS another app set afterwards, must not be cleared by rolling back
    // an old profile. Unowned entries are skipped without holding the
    // rollback open; an unreadable ownership record does hold it open.
    let enabled = if enabled.is_empty() {
        enabled
    } else {
        match owned_ecoqos_candidates(db) {
            Ok(owned) => enabled.into_iter()
                .filter(|(n, p, _)| owned.iter().any(|(on, op, _)| op == p && on.eq_ignore_ascii_case(n)))
                .collect(),
            Err(e) => return Some((format!("could not read which throttles WinClean still owns ({e}); EcoQoS left unchanged"), enabled.len())),
        }
    };
    if enabled.is_empty() && restored.is_empty() {
        return Some(("no throttles from this profile are still owned by WinClean".to_string(), 0));
    }
    let (mut parts, mut incomplete) = (Vec::new(), 0usize);
    if !enabled.is_empty() {
        let outcome = restore_ecoqos(db, enabled, "rollback of power profile", None);
        incomplete += outcome.incomplete;
        parts.push(outcome.summary);
    }
    if !restored.is_empty() { parts.push(rethrottle_ecoqos(db, restored, "rollback of power profile").summary); }
    Some((parts.join("; "), incomplete))
}
#[tauri::command(async)]
pub async fn optimizer_hardware_info() -> Result<crate::optimizer::hardware::HardwareInfo, String> {
    hardware_info()
}

#[tauri::command]
pub fn optimizer_capabilities() -> Result<crate::optimizer::capabilities::CapabilityReport, String> {
    let hw = hardware_info().ok();
    let npu_present = hw.as_ref().map(|h| h.npu.present).unwrap_or(false);
    let igpu = hw.as_ref().map(|h| h.adapters.iter().any(|a| a.kind == "igpu")).unwrap_or(false);
    Ok(capability_report(npu_present, igpu))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerHistory {
    pub audit: Vec<OptimizationAudit>,
    pub snapshots: Vec<OptimizationSnapshot>,
    pub profiles: Vec<crate::optimizer::power::PowerProfile>,
}

#[tauri::command]
pub fn optimizer_history(state: State<'_, AppState>, limit: Option<u32>) -> Result<OptimizerHistory, String> {
    let limit = limit.unwrap_or(100).clamp(1, 500);
    Ok(OptimizerHistory {
        audit: state.db.list_optimizer_audit(limit).map_err(to_command_error)?,
        snapshots: state.db.list_optimizer_snapshots(limit).map_err(to_command_error)?,
        profiles: list_profiles(),
    })
}

#[tauri::command(async)]
pub async fn optimizer_rollback(state: State<'_, AppState>, snapshot_id: i64) -> Result<String, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || rollback_blocking(&db, snapshot_id))
        .await.map_err(|e| e.to_string())?
}

fn rollback_blocking(db: &Database, snapshot_id: i64) -> Result<String, String> {
    let _guard = lock_optimizer_mutations();
    let snapshot = db.get_optimizer_snapshot(snapshot_id).map_err(to_command_error)?
        .ok_or_else(|| format!("snapshot {snapshot_id} not found"))?;
    if snapshot.rolled_back_at_ms.is_some() {
        return Err("snapshot was already rolled back".into());
    }
    let payload: serde_json::Value = serde_json::from_str(&snapshot.payload_json).map_err(|e| e.to_string())?;
    // Processes that should have been restored but could not be; the
    // snapshot then stays retryable instead of being marked rolled back.
    let mut incomplete = 0usize;
    let result = match snapshot.kind.as_str() {
        "service" => {
            let target = start_type_from_snapshot(&payload).ok_or("snapshot has no restorable start type")?;
            set_service_start_type(&snapshot.target, target).map(|s| format!("{} restored to {}", s.display_name, s.start_type))
        }
        "powerScheme" => {
            let guid = payload.get("schemeGuid").and_then(|v| v.as_str()).ok_or("snapshot has no previous scheme GUID")?;
            set_active_scheme(guid).map(|_| {
                let scheme = format!("power scheme restored to {guid}");
                match rollback_profile_ecoqos(db, &payload, snapshot_id, snapshot.created_at_ms) {
                    Some((eco, missed)) => {
                        incomplete = missed;
                        format!("{scheme}; EcoQoS: {eco}")
                    }
                    None => scheme,
                }
            })
        }
        other => Err(format!("rollback is not supported for snapshot kind '{other}'")),
    };
    match result {
        Ok(msg) if incomplete > 0 => {
            let msg = format!("{msg}. {incomplete} process(es) could not be restored; this rollback stays available to retry.");
            let _ = db.insert_optimizer_audit(&snapshot.kind, &snapshot.target, "rollback", "partial", Some(&msg), Some(snapshot_id));
            Ok(msg)
        }
        Ok(msg) => {
            db.mark_snapshot_rolled_back(snapshot_id).map_err(to_command_error)?;
            let _ = db.insert_optimizer_audit(&snapshot.kind, &snapshot.target, "rollback", "completed", Some(&msg), Some(snapshot_id));
            Ok(msg)
        }
        Err(e) => {
            let _ = db.insert_optimizer_audit(&snapshot.kind, &snapshot.target, "rollback", "failed", Some(&e), Some(snapshot_id));
            Err(e)
        }
    }
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

#[cfg(windows)]
fn open_recycle_bin_impl() -> anyhow::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg("shell:RecycleBinFolder")
        .spawn()
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("failed to open Windows Recycle Bin: {error}"))
}

#[cfg(not(windows))]
fn open_recycle_bin_impl() -> anyhow::Result<()> {
    anyhow::bail!("Opening Recycle Bin is only available on Windows in this build")
}

#[cfg(test)]
mod optimizer_tests {
    use super::*;

    #[test]
    fn touched_process_round_trips_and_older_snapshots_default_since() {
        let touched = EcoQosTouched { pid: 7, name: "Updater.exe".into(), since_ms: Some(1_234) };
        let json = serde_json::to_value(&touched).unwrap();
        assert_eq!(json["sinceMs"], 1_234);
        assert_eq!(serde_json::from_value::<EcoQosTouched>(json).unwrap(), touched);

        // Snapshots written before sinceMs existed still load.
        let old: EcoQosTouched = serde_json::from_str(r#"{"pid":9,"name":"Old.exe"}"#).unwrap();
        assert_eq!(old.since_ms, None);
    }

    #[test]
    fn rollback_only_restores_throttles_winclean_still_owns() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(&temp.path().join("index.sqlite3")).unwrap();
        db.migrate().unwrap();
        let snapshot_id = db.insert_optimizer_snapshot("powerScheme", "Balanced", "{}").unwrap();
        let audit = |target: &str, action: &str| {
            db.insert_optimizer_audit("process", target, action, "completed", None, Some(snapshot_id)).unwrap();
        };
        // The profile throttled both; the user then restored one by hand.
        audit("Owned.exe (10)", "ecoqos:on");
        audit("Restored.exe (11)", "ecoqos:on");
        db.insert_optimizer_audit("process", "Restored.exe (11)", "ecoqos:off", "completed", None, None).unwrap();
        let payload = serde_json::json!({});

        // Restored.exe is no longer owned, so only Owned.exe is attempted. Off
        // Windows its liveness is unknown, so that one counts as incomplete.
        let (_, incomplete) = rollback_profile_ecoqos(&db, &payload, snapshot_id, 0).unwrap();
        assert_eq!(incomplete, 1);

        // With nothing owned, nothing is touched and the rollback can finish.
        db.insert_optimizer_audit("process", "Owned.exe (10)", "ecoqos:off", "completed", None, None).unwrap();
        let (summary, incomplete) = rollback_profile_ecoqos(&db, &payload, snapshot_id, 0).unwrap();
        assert_eq!(incomplete, 0);
        assert!(summary.contains("no throttles"));
    }

    #[test]
    fn mutation_lock_recovers_from_poisoning() {
        let result = std::thread::spawn(|| {
            let _guard = lock_optimizer_mutations();
            panic!("poison the lock");
        })
        .join();
        assert!(result.is_err());
        // A panicked mutation must not wedge every later optimizer change.
        let _guard = lock_optimizer_mutations();
    }
}
