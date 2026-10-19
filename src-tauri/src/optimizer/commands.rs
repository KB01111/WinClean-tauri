//! Tauri commands for the WinClean 2.0 optimizer.
//!
//! These are thin adapters: parsing, then a call into the independently
//! tested modules (`orchestrator`, `service_plan`, `profiles`, …). Blocking
//! Windows work runs on blocking threads, never on the UI thread.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;

use crate::commands::AppState;
use crate::db::Database;
use crate::db_optimizer::MeasurementRecord;

use super::auto_switch::AutoSwitchConfig;
use super::backend::WriteCapability;
use super::battery::{BatteryTelemetry, TelemetrySample};
use super::capabilities::{capability_report, windows_build, CapabilityReport};
use super::ecoqos::now_ms;
use super::hardware::{hardware_info, hardware_info_refresh, HardwareInfo};
use super::helper_client::HelperStatus;
use super::mcp;
use super::measure::{compare_windows, MeasurementResult};
use super::orchestrator::{self, Ctx, ErrorKind, OptError, Origin, ProfileOutcome, ServiceApplyOutcome};
use super::policy::PowerProfile;
use super::power::{battery_status, BatteryStatus, PowerSample};
use super::processes::{list_processes, ProcessInfo};
use super::profiles::{self, ActiveProfile};
use super::recovery::RecoveryReport;
use super::service_plan::{recommend_service_changes, validate_service_plan, ServiceChange, ServicePlan, ServiceRecommendation};
use super::services::ServiceInfo;
use super::state::{OptimizerState, PreparedPlan, PLAN_TTL_MS};
use super::telemetry::SharedTelemetry;
use super::workload::{self, CpuTracker, WorkloadSnapshot};
use super::{OptimizationAudit, OptimizationSnapshot};

/// Error returned to the frontend: a stable code plus a readable message.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorKind,
    pub message: String,
}

impl From<OptError> for CommandError {
    fn from(e: OptError) -> Self {
        Self { code: e.code, message: e.message }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self { code: ErrorKind::Failed, message }
    }
}

impl From<anyhow::Error> for CommandError {
    fn from(e: anyhow::Error) -> Self {
        Self { code: ErrorKind::Failed, message: e.to_string() }
    }
}

type CmdResult<T> = Result<T, CommandError>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| CommandError::from(e.to_string()))?
}

/// Last crash-recovery result, kept so the UI can show what was reconciled.
#[derive(Default)]
pub struct RecoveryHolder(pub Mutex<RecoveryReport>);

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub async fn optimizer_list_services(state: State<'_, OptimizerState>) -> CmdResult<Vec<ServiceInfo>> {
    let backends = state.backends.clone();
    blocking(move || backends.services.list().map_err(CommandError::from)).await
}

#[tauri::command(async)]
pub async fn optimizer_service_recommendations(state: State<'_, OptimizerState>) -> CmdResult<Vec<ServiceRecommendation>> {
    let backends = state.backends.clone();
    blocking(move || Ok(recommend_service_changes(&backends.services.list()?))).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPlanView {
    /// Present only when the plan has at least one valid item.
    pub token: Option<String>,
    pub plan: ServicePlan,
    pub fingerprint: String,
    pub expires_at_ms: i64,
    pub write_capability: WriteCapability,
}

/// Validates `changes` against the live SCM and, if anything is valid, issues
/// a single-use approval token for exactly that plan.
#[tauri::command(async)]
pub async fn optimizer_prepare_service_plan(state: State<'_, OptimizerState>, changes: Vec<ServiceChange>, origin: Option<String>) -> CmdResult<PreparedPlanView> {
    let backends = state.backends.clone();
    let origin = if origin.as_deref() == Some("ai") { Origin::AiApproved } else { Origin::User };
    let (plan, capability) = blocking(move || {
        let services = backends.services.list()?;
        Ok((validate_service_plan(&changes, &services), backends.services.write_capability()))
    })
    .await?;
    let now = now_ms();
    let fingerprint = plan.fingerprint();
    let expires_at_ms = now + PLAN_TTL_MS;
    let token = if plan.items.is_empty() {
        None
    } else {
        let accepted: Vec<ServiceChange> = plan.items.iter().map(|i| ServiceChange { service_name: i.name.clone(), start_type: i.to.as_str().to_string() }).collect();
        Some(state.plans.insert(PreparedPlan { changes: accepted, fingerprint: fingerprint.clone(), origin, expires_at_ms }, now)?)
    };
    Ok(PreparedPlanView { token, plan, fingerprint, expires_at_ms, write_capability: capability })
}

/// Applies a previously prepared plan. The token is consumed even on failure,
/// so a failed plan must be reviewed again.
#[tauri::command(async)]
pub async fn optimizer_apply_service_plan(app: State<'_, AppState>, state: State<'_, OptimizerState>, token: String) -> CmdResult<ServiceApplyOutcome> {
    let prepared = state.plans.take(&token, now_ms())?;
    let (db, backends) = (app.db.clone(), state.backends.clone());
    blocking(move || {
        let ctx = Ctx { db: &db, backends: &backends };
        orchestrator::apply_service_changes(&ctx, &prepared.changes, Some(&prepared.fingerprint), prepared.origin).map_err(CommandError::from)
    })
    .await
}

/// Single-service convenience (the per-row dropdown): validates, then applies
/// through the same transactional path as a plan of one.
#[tauri::command(async)]
pub async fn optimizer_set_service_start_type(app: State<'_, AppState>, state: State<'_, OptimizerState>, name: String, start_type: String) -> CmdResult<ServiceApplyOutcome> {
    let (db, backends) = (app.db.clone(), state.backends.clone());
    blocking(move || {
        let ctx = Ctx { db: &db, backends: &backends };
        orchestrator::apply_service_changes(&ctx, &[ServiceChange { service_name: name, start_type }], None, Origin::User).map_err(CommandError::from)
    })
    .await
}

// ---------------------------------------------------------------------------
// Administrator helper
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn optimizer_helper_status(state: State<'_, OptimizerState>) -> HelperStatus {
    state.helper.status()
}

/// Shows the UAC prompt and starts the helper. Blocks until answered.
#[tauri::command(async)]
pub async fn optimizer_helper_connect(state: State<'_, OptimizerState>, allow_untrusted_location: Option<bool>) -> CmdResult<HelperStatus> {
    let helper = state.helper.clone();
    blocking(move || {
        helper.connect(allow_untrusted_location.unwrap_or(false))?;
        Ok(helper.status())
    })
    .await
}

#[tauri::command(async)]
pub async fn optimizer_helper_disconnect(state: State<'_, OptimizerState>) -> CmdResult<HelperStatus> {
    let helper = state.helper.clone();
    blocking(move || {
        helper.disconnect();
        Ok(helper.status())
    })
    .await
}

// ---------------------------------------------------------------------------
// Processes, workload
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub async fn optimizer_list_processes() -> CmdResult<Vec<ProcessInfo>> {
    blocking(|| list_processes().map_err(CommandError::from)).await
}

#[tauri::command(async)]
pub async fn optimizer_set_process_ecoqos(app: State<'_, AppState>, state: State<'_, OptimizerState>, pid: u32, name: String, enable: bool) -> CmdResult<()> {
    let (db, backends) = (app.db.clone(), state.backends.clone());
    blocking(move || {
        let ctx = Ctx { db: &db, backends: &backends };
        orchestrator::set_process_ecoqos(&ctx, pid, &name, enable).map_err(CommandError::from)
    })
    .await
}

/// Classifies what the user is doing right now (one cheap sample; CPU shares
/// need two samples, so the first call reports toolchains as "not measured").
#[tauri::command(async)]
pub async fn optimizer_workload() -> CmdResult<WorkloadSnapshot> {
    blocking(|| {
        let mut tracker = CpuTracker::default();
        // Two observations 400 ms apart give real CPU shares without a long sleep.
        let _ = workload::sample(&mut tracker);
        std::thread::sleep(std::time::Duration::from_millis(400));
        Ok(workload::sample(&mut tracker))
    })
    .await
}

// ---------------------------------------------------------------------------
// Battery, power, profiles
// ---------------------------------------------------------------------------

/// Legacy 2.0 command: AC/DC, percentage, active scheme.
#[tauri::command]
pub fn optimizer_battery_status() -> CmdResult<BatteryStatus> {
    battery_status().map_err(CommandError::from)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryView {
    pub telemetry: BatteryTelemetry,
    pub status: Option<BatteryStatus>,
    pub last_sample: Option<TelemetrySample>,
}

#[tauri::command(async)]
pub async fn optimizer_battery_telemetry(app: State<'_, AppState>, telemetry: State<'_, SharedTelemetry>) -> CmdResult<TelemetryView> {
    let (db, telemetry) = (app.db.clone(), telemetry.inner().clone());
    blocking(move || {
        let recent = db.recent_power_telemetry(super::telemetry::TelemetryService::summary_window())?;
        Ok(TelemetryView { telemetry: telemetry.telemetry(&recent), status: battery_status().ok(), last_sample: telemetry.last_sample() })
    })
    .await
}

#[tauri::command]
pub fn optimizer_telemetry_history(app: State<'_, AppState>, limit: Option<u32>) -> CmdResult<Vec<TelemetrySample>> {
    Ok(app.db.recent_power_telemetry(limit.unwrap_or(240).clamp(1, 5000))?)
}

/// Legacy 2.0 command, now derived from the richer telemetry table.
#[tauri::command]
pub fn optimizer_power_history(app: State<'_, AppState>, limit: Option<u32>) -> CmdResult<Vec<PowerSample>> {
    let samples = app.db.recent_power_telemetry(limit.unwrap_or(200).clamp(1, 2000))?;
    Ok(samples.into_iter().filter_map(|s| s.percent.map(|p| PowerSample { at_ms: s.at_ms, percent: p, on_ac: s.on_ac })).collect())
}

/// The Battery view calls this every ~20 s while visible so sampling speeds
/// up only while someone is looking; `active = false` releases the lease.
#[tauri::command]
pub fn optimizer_telemetry_lease(telemetry: State<'_, SharedTelemetry>, active: bool) {
    telemetry.lease(now_ms(), active);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesView {
    pub profiles: Vec<PowerProfile>,
    pub active: Option<ActiveProfile>,
    pub auto_switch: AutoSwitchConfig,
}

fn profiles_view(db: &Database) -> ProfilesView {
    ProfilesView { profiles: profiles::list_profiles(db), active: profiles::get_active(db), auto_switch: profiles::get_auto_switch(db) }
}

#[tauri::command]
pub fn optimizer_list_profiles(app: State<'_, AppState>) -> ProfilesView {
    profiles_view(&app.db)
}

#[tauri::command(async)]
pub async fn optimizer_apply_power_profile(app: State<'_, AppState>, state: State<'_, OptimizerState>, profile_id: String) -> CmdResult<ProfileOutcome> {
    let (db, backends) = (app.db.clone(), state.backends.clone());
    blocking(move || {
        let ctx = Ctx { db: &db, backends: &backends };
        orchestrator::apply_profile(&ctx, &profile_id, Origin::User).map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub fn optimizer_save_custom_profile(app: State<'_, AppState>, profile: PowerProfile) -> CmdResult<ProfilesView> {
    profiles::save_custom_profile(&app.db, profile)?;
    Ok(profiles_view(&app.db))
}

#[tauri::command]
pub fn optimizer_delete_custom_profile(app: State<'_, AppState>, profile_id: String) -> CmdResult<ProfilesView> {
    profiles::delete_custom_profile(&app.db, &profile_id)?;
    Ok(profiles_view(&app.db))
}

#[tauri::command]
pub fn optimizer_set_auto_switch(app: State<'_, AppState>, config: AutoSwitchConfig) -> CmdResult<ProfilesView> {
    profiles::set_auto_switch(&app.db, config)?;
    Ok(profiles_view(&app.db))
}

// ---------------------------------------------------------------------------
// Measurements
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementView {
    pub record: MeasurementRecord,
    pub result: Option<MeasurementResult>,
}

fn measurement_view(record: MeasurementRecord) -> MeasurementView {
    let result = record.result_json.as_deref().and_then(|j| serde_json::from_str(j).ok());
    MeasurementView { record, result }
}

/// Starts the baseline window: sampling runs at the measurement rate until
/// the measurement is finished.
#[tauri::command]
pub fn optimizer_measurement_start(app: State<'_, AppState>, telemetry: State<'_, SharedTelemetry>, label: String, profile_id: String) -> CmdResult<MeasurementView> {
    if profiles::find_profile(&app.db, &profile_id).is_none() {
        return Err(OptError::new(ErrorKind::NotFound, format!("unknown profile {profile_id}")).into());
    }
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 60 {
        return Err(OptError::rejected("name the measurement (1-60 characters)").into());
    }
    let id = app.db.create_measurement(label, &profile_id, now_ms())?;
    telemetry.set_measuring(true);
    Ok(measurement_view(app.db.get_measurement(id)?.ok_or_else(|| "measurement vanished".to_string())?))
}

/// Ends the baseline and applies the profile; the "after" window starts now.
#[tauri::command(async)]
pub async fn optimizer_measurement_switch(app: State<'_, AppState>, state: State<'_, OptimizerState>, id: i64) -> CmdResult<MeasurementView> {
    let (db, backends) = (app.db.clone(), state.backends.clone());
    blocking(move || {
        let record = db.get_measurement(id)?.ok_or_else(|| OptError::new(ErrorKind::NotFound, "no such measurement"))?;
        if record.switch_ms.is_some() {
            return Err(OptError::new(ErrorKind::Conflict, "this measurement already switched profiles").into());
        }
        let switch_ms = now_ms();
        let ctx = Ctx { db: &db, backends: &backends };
        orchestrator::apply_profile(&ctx, &record.profile_id, Origin::User)?;
        db.update_measurement(id, Some(switch_ms), None, None)?;
        Ok(measurement_view(db.get_measurement(id)?.ok_or_else(|| "measurement vanished".to_string())?))
    })
    .await
}

#[tauri::command]
pub fn optimizer_measurement_finish(app: State<'_, AppState>, telemetry: State<'_, SharedTelemetry>, id: i64) -> CmdResult<MeasurementView> {
    let record = app.db.get_measurement(id)?.ok_or_else(|| OptError::new(ErrorKind::NotFound, "no such measurement"))?;
    let Some(switch_ms) = record.switch_ms else {
        return Err(OptError::rejected("apply the profile first, then let it run before finishing").into());
    };
    let end = now_ms();
    let samples = app.db.power_telemetry_between(record.baseline_start_ms, end, 20_000)?;
    let result = compare_windows(&samples, record.baseline_start_ms, switch_ms, end);
    app.db.update_measurement(id, None, Some(end), Some(&serde_json::to_string(&result).map_err(|e| e.to_string())?))?;
    // Stop the fast sampling when no other measurement is still open.
    let still_open = app.db.list_measurements(20)?.iter().any(|m| m.ended_ms.is_none());
    telemetry.set_measuring(still_open);
    Ok(measurement_view(app.db.get_measurement(id)?.ok_or_else(|| "measurement vanished".to_string())?))
}

#[tauri::command]
pub fn optimizer_measurement_list(app: State<'_, AppState>) -> CmdResult<Vec<MeasurementView>> {
    Ok(app.db.list_measurements(30)?.into_iter().map(measurement_view).collect())
}

// ---------------------------------------------------------------------------
// Hardware, capabilities
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub async fn optimizer_hardware_info(refresh: Option<bool>) -> CmdResult<HardwareInfo> {
    blocking(move || if refresh.unwrap_or(false) { hardware_info_refresh() } else { hardware_info() }.map_err(CommandError::from)).await
}

/// Windows ML execution-provider assessment and routing. `model_path` (a local
/// `.onnx` file) is optional and is only ever read, never loaded or executed.
#[tauri::command(async)]
pub async fn optimizer_capabilities(model_path: Option<String>) -> CmdResult<CapabilityReport> {
    blocking(move || capability_report(model_path.as_deref()).map_err(CommandError::from)).await
}

// ---------------------------------------------------------------------------
// Windows MCP integration (optional, preview)
// ---------------------------------------------------------------------------

fn mcp_error(f: mcp::service::McpFailure) -> CommandError {
    let code = match f.kind {
        mcp::service::FailureKind::NotAuthorised | mcp::service::FailureKind::IdentityNotVerified => ErrorKind::Rejected,
        _ => ErrorKind::Failed,
    };
    CommandError { code, message: f.message }
}

/// Copilot+ status by specification (never measured); gates semantic search.
fn copilot_plus_by_spec() -> bool {
    capability_report(None).map(|r| r.copilot_plus.status == super::capabilities::CopilotPlusKind::BySpec).unwrap_or(false)
}

fn audit_mcp(db: &Database, target: &str, action: &str, ok: bool, detail: &str) {
    // Paths and search text are user data and are deliberately not stored in the audit trail.
    let _ = db.insert_optimizer_audit("mcp", target, action, if ok { "completed" } else { "failed" }, Some(detail), None);
}

/// Windows MCP status. Discovery only: runs `odr.exe list` (read-only, cached
/// for 30 s unless `refresh`) and never connects to or starts a server.
#[tauri::command(async)]
pub async fn optimizer_mcp_status(state: State<'_, OptimizerState>, refresh: Option<bool>) -> CmdResult<mcp::McpStatus> {
    let (mcp, cancel) = (state.mcp.clone(), state.begin_mcp_operation());
    blocking(move || Ok(mcp.status(windows_build(), copilot_plus_by_spec(), refresh.unwrap_or(false), &cancel))).await
}

/// User-initiated: connect to the verified File Explorer connector, perform
/// the MCP handshake and read its live tool list and schemas. No tool is called.
#[tauri::command(async)]
pub async fn optimizer_mcp_verify(app: State<'_, AppState>, state: State<'_, OptimizerState>) -> CmdResult<mcp::McpStatus> {
    let (mcp, cancel, db) = (state.mcp.clone(), state.begin_mcp_operation(), app.db.clone());
    blocking(move || {
        let status = mcp.verify_connection(windows_build(), copilot_plus_by_spec(), &cancel);
        let (ok, detail) = match &status.file_explorer {
            Some(fe) => (fe.connection == mcp::service::ConnectionState::Connected, format!("{:?}", fe.connection)),
            None => (false, "connector not found or not verified".to_string()),
        };
        audit_mcp(&db, "file-explorer", "verify-connection", ok, &detail);
        Ok(status)
    })
    .await
}

/// User-initiated: asks Windows to create the agent user account that
/// contained MCP servers run as (`odr.exe mcp provision-agent-user`). This
/// changes the system, so it is never done automatically.
#[tauri::command(async)]
pub async fn optimizer_mcp_provision_agent_user(app: State<'_, AppState>, state: State<'_, OptimizerState>) -> CmdResult<String> {
    let (mcp, cancel, db) = (state.mcp.clone(), state.begin_mcp_operation(), app.db.clone());
    blocking(move || {
        let result = mcp.provision_agent_user(windows_build(), &cancel);
        audit_mcp(&db, "odr", "provision-agent-user", result.is_ok(), result.as_ref().map(|s| s.as_str()).unwrap_or_else(|e| e.message.as_str()));
        result.map_err(mcp_error)
    })
    .await
}

/// Stops whichever Windows MCP operation is running (the server process tree is killed).
#[tauri::command]
pub fn optimizer_mcp_cancel(state: State<'_, OptimizerState>) {
    state.mcp_cancel.lock().unwrap_or_else(|p| p.into_inner()).store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Folders the user has authorised for Windows MCP search (empty by default).
#[tauri::command]
pub fn optimizer_mcp_search_roots(app: State<'_, AppState>) -> Vec<String> {
    mcp::roots::list(&app.db)
}

#[tauri::command]
pub fn optimizer_mcp_add_search_root(app: State<'_, AppState>, path: String) -> CmdResult<Vec<String>> {
    let roots = mcp::roots::add(&app.db, &path, &mcp::roots::RealFs).map_err(|e| CommandError::from(OptError::rejected(e)))?;
    audit_mcp(&app.db, "search-roots", "authorise-folder", true, &format!("{} folder(s) authorised", roots.len()));
    Ok(roots)
}

#[tauri::command]
pub fn optimizer_mcp_remove_search_root(app: State<'_, AppState>, path: String) -> CmdResult<Vec<String>> {
    let roots = mcp::roots::remove(&app.db, &path).map_err(|e| CommandError::from(OptError::rejected(e)))?;
    audit_mcp(&app.db, "search-roots", "revoke-folder", true, &format!("{} folder(s) authorised", roots.len()));
    Ok(roots)
}

/// Asks the connector to list one authorised folder: proof that Windows granted
/// access and that a tool really executed. User-initiated.
#[tauri::command(async)]
pub async fn optimizer_mcp_verify_permission(app: State<'_, AppState>, state: State<'_, OptimizerState>, folder: String) -> CmdResult<mcp::McpStatus> {
    let (mcp, cancel, db) = (state.mcp.clone(), state.begin_mcp_operation(), app.db.clone());
    blocking(move || {
        let roots = mcp::roots::list(&db);
        let result = mcp.verify_permission(windows_build(), copilot_plus_by_spec(), &folder, &roots, &cancel);
        audit_mcp(&db, "file-explorer:get_directories", "verify-permission", result.is_ok(), result.as_ref().map(|_| "granted").unwrap_or_else(|e| e.message.as_str()));
        result.map_err(mcp_error)
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSearchArgs {
    pub starting_dir: String,
    pub patterns: String,
    #[serde(default)]
    pub file_types: Vec<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    #[serde(default)]
    pub semantic: bool,
    #[serde(default)]
    pub include_snippets: bool,
    pub limit: Option<usize>,
}

/// A user-initiated Windows MCP search inside an authorised folder. Results
/// are untrusted: validated, cleaned and bounded; snippets are off unless asked for.
#[tauri::command(async)]
pub async fn optimizer_mcp_search(app: State<'_, AppState>, state: State<'_, OptimizerState>, args: McpSearchArgs) -> CmdResult<mcp::service::SearchOutcome> {
    let (mcp, cancel, db) = (state.mcp.clone(), state.begin_mcp_operation(), app.db.clone());
    blocking(move || {
        let roots = mcp::roots::list(&db);
        let req = mcp::policy::SearchRequest {
            starting_dir: args.starting_dir,
            patterns: args.patterns,
            file_types: args.file_types,
            start_date: args.start_date,
            end_date: args.end_date,
            semantic: if args.semantic { mcp::policy::Semantic::Requested } else { mcp::policy::Semantic::Off },
        };
        let result = mcp.search(windows_build(), copilot_plus_by_spec(), &req, &roots, args.include_snippets, args.limit.unwrap_or(100), &cancel);
        audit_mcp(&db, "file-explorer:search_files", "search", result.is_ok(), result.as_ref().map(|r| format!("{} result(s)", r.hits.len())).unwrap_or_else(|e| e.message.clone()).as_str());
        result.map_err(mcp_error)
    })
    .await
}

/// Metadata for one file inside an authorised folder (`get_file_details`).
#[tauri::command(async)]
pub async fn optimizer_mcp_file_details(app: State<'_, AppState>, state: State<'_, OptimizerState>, path: String) -> CmdResult<serde_json::Value> {
    let (mcp, cancel, db) = (state.mcp.clone(), state.begin_mcp_operation(), app.db.clone());
    blocking(move || {
        let roots = mcp::roots::list(&db);
        let result = mcp.file_details(windows_build(), &path, &roots, &cancel);
        // The path is user data and is deliberately not stored in the audit trail.
        audit_mcp(&db, "file-explorer:get_file_details", "call", result.is_ok(), "get_file_details (metadata only)");
        result.map_err(mcp_error)
    })
    .await
}

// ---------------------------------------------------------------------------
// History, rollback, recovery
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizerHistory {
    pub audit: Vec<OptimizationAudit>,
    pub snapshots: Vec<OptimizationSnapshot>,
    pub profiles: Vec<PowerProfile>,
    pub active_profile: Option<ActiveProfile>,
}

#[tauri::command]
pub fn optimizer_history(app: State<'_, AppState>, limit: Option<u32>) -> CmdResult<OptimizerHistory> {
    let limit = limit.unwrap_or(100).clamp(1, 500);
    Ok(OptimizerHistory {
        audit: app.db.list_optimizer_audit(limit)?,
        snapshots: app.db.list_optimizer_snapshots(limit)?,
        profiles: profiles::list_profiles(&app.db),
        active_profile: profiles::get_active(&app.db),
    })
}

#[tauri::command(async)]
pub async fn optimizer_rollback(app: State<'_, AppState>, state: State<'_, OptimizerState>, snapshot_id: i64) -> CmdResult<String> {
    let (db, backends) = (app.db.clone(), state.backends.clone());
    blocking(move || {
        let ctx = Ctx { db: &db, backends: &backends };
        orchestrator::rollback(&ctx, snapshot_id).map_err(CommandError::from)
    })
    .await
}

#[tauri::command]
pub fn optimizer_recovery_report(holder: State<'_, Arc<RecoveryHolder>>) -> RecoveryReport {
    holder.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

// ---------------------------------------------------------------------------
// Native window integration
// ---------------------------------------------------------------------------

/// Whether the Windows 11 Mica backdrop is active on the main window.
pub struct NativeState {
    pub mica: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeInfo {
    pub windows_build: Option<u32>,
    pub mica: bool,
}

#[tauri::command]
pub fn native_info(state: State<'_, NativeState>) -> NativeInfo {
    NativeInfo { windows_build: windows_build(), mica: state.mica }
}

/// Applies Mica on Windows 11 (build 22621+). Failure is silent: the opaque
/// theme background is always the fallback.
pub fn apply_native_effects(window: &tauri::WebviewWindow) -> NativeState {
    #[cfg(windows)]
    {
        use tauri::utils::config::WindowEffectsConfig;
        use tauri::window::{Effect, EffectsBuilder};
        let supported = windows_build().map(|b| b >= 22621).unwrap_or(false);
        if supported {
            let effects: WindowEffectsConfig = EffectsBuilder::new().effect(Effect::Mica).build();
            return NativeState { mica: window.set_effects(effects).is_ok() };
        }
    }
    let _ = window;
    NativeState { mica: false }
}
