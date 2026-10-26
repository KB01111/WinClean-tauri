//! Transactional optimizer operations.
//!
//! Every mutation follows the same contract:
//! policy check → privilege preflight → snapshot → pending audit → apply →
//! verify by re-reading → completed audit. A failure in the middle of a batch
//! rolls the already-applied steps back, so a plan is all-or-nothing, and a
//! crash leaves `pending` audit rows that `recovery` reconciles on the next
//! start.

use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use serde_json::json;

use crate::db::Database;

use super::backend::{Backends, BackendError, WriteCapability};
use super::ecoqos;
use super::policy::{EcoQosMode, PowerProfile};
use super::profiles::{self, ActiveProfile};
use super::service_plan::{validate_service_plan, ServiceChange};
use super::services::{snapshot_payload, start_type_from_snapshot, ServiceInfo, StartType};

/// Serializes optimizer mutations (profiles, rollbacks, EcoQoS, services) so
/// overlapping requests cannot snapshot a stale scheme or complete their
/// audit rows out of order. Reads never take it.
static MUTATION: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    MUTATION.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct Ctx<'a> {
    pub db: &'a Database,
    pub backends: &'a Backends,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Auto,
    /// Proposed by the AI assistant and explicitly approved by the user.
    AiApproved,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Auto => "auto",
            Origin::AiApproved => "ai-approved",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    ElevationRequired,
    Rejected,
    Conflict,
    NotFound,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptError {
    pub code: ErrorKind,
    pub message: String,
}

impl OptError {
    pub fn new(code: ErrorKind, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Failed, message)
    }
    pub fn rejected(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Rejected, message)
    }
}

impl From<BackendError> for OptError {
    fn from(e: BackendError) -> Self {
        let code = match &e {
            BackendError::ElevationRequired(_) => ErrorKind::ElevationRequired,
            BackendError::Rejected(_) => ErrorKind::Rejected,
            BackendError::Conflict(_) => ErrorKind::Conflict,
            BackendError::Failed(_) => ErrorKind::Failed,
        };
        Self::new(code, e.message().to_string())
    }
}

impl std::fmt::Display for OptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn db_err(e: anyhow::Error) -> OptError {
    OptError::failed(e.to_string())
}

const ELEVATION_MESSAGE: &str = "Changing services needs administrator rights. Enable the administrator helper (one UAC prompt) and try again.";

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedService {
    pub name: String,
    pub display_name: String,
    pub from: StartType,
    pub to: StartType,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceApplyOutcome {
    pub snapshot_id: i64,
    pub applied: Vec<AppliedService>,
    pub summary: String,
}

fn find<'a>(services: &'a [ServiceInfo], name: &str) -> Option<&'a ServiceInfo> {
    services.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// Re-reads a service and checks it has `want`.
fn verify_start_type(b: &Backends, name: &str, want: StartType) -> Result<(), String> {
    let services = b.services.list().map_err(|e| format!("could not re-read services to verify: {e}"))?;
    match find(&services, name).and_then(|s| s.current_start_type()) {
        Some(got) if got == want => Ok(()),
        Some(got) => Err(format!("SCM reports {} after the change, expected {}", got.as_str(), want.as_str())),
        None => Err(format!("could not read the start type of {name} to verify the change")),
    }
}

/// Applies a validated set of start-type changes as one transaction.
///
/// `reviewed_fingerprint` is the fingerprint of the plan the user reviewed
/// (or approved, for AI proposals). If the live state no longer produces the
/// same plan, nothing is changed.
pub fn apply_service_changes(ctx: &Ctx, changes: &[ServiceChange], reviewed_fingerprint: Option<&str>, origin: Origin) -> Result<ServiceApplyOutcome, OptError> {
    let _guard = lock();
    let b = ctx.backends;

    // 1. Privilege preflight: nothing is recorded or changed without a write path.
    if b.services.write_capability() == WriteCapability::Unavailable {
        return Err(OptError::new(ErrorKind::ElevationRequired, ELEVATION_MESSAGE));
    }

    // 2. Re-validate against the live SCM (never trust a stale or forged plan).
    let services = b.services.list().map_err(OptError::failed)?;
    let plan = validate_service_plan(changes, &services);
    if let Some(first) = plan.rejected.first() {
        return Err(OptError::rejected(format!("{}: {}", first.service_name, first.reason)));
    }
    if plan.items.is_empty() {
        return Err(OptError::rejected("nothing to change"));
    }
    if let Some(expected) = reviewed_fingerprint {
        if expected != plan.fingerprint() {
            return Err(OptError::new(ErrorKind::Conflict, "Services changed since you reviewed this plan. Review it again."));
        }
    }

    // 3. Snapshot (the "before" state of every service) and pending audit rows.
    let mut snapshot_items = Vec::new();
    for item in &plan.items {
        let current = find(&services, &item.name).ok_or_else(|| OptError::failed(format!("{} disappeared", item.name)))?;
        snapshot_items.push(snapshot_payload(current));
    }
    let label = if plan.items.len() == 1 { plan.items[0].name.clone() } else { format!("{} services", plan.items.len()) };
    let payload = json!({ "items": snapshot_items, "origin": origin.as_str(), "fingerprint": plan.fingerprint() });
    let snapshot_id = ctx.db.insert_optimizer_snapshot("serviceBatch", &label, &payload.to_string()).map_err(db_err)?;
    let mut audit_ids = Vec::new();
    for item in &plan.items {
        let id = ctx
            .db
            .insert_optimizer_audit("service", &item.name, &format!("setStartType:{}", item.to.as_str()), "pending", Some(&format!("{} → {} ({})", item.from.as_str(), item.to.as_str(), origin.as_str())), Some(snapshot_id))
            .map_err(db_err)?;
        audit_ids.push(id);
    }

    // 4. Apply one by one, verifying each; the first failure undoes the rest.
    let mut applied: Vec<usize> = Vec::new();
    for (idx, item) in plan.items.iter().enumerate() {
        let result = b.services.set_start_type(&item.name, item.to, Some(item.from)).map_err(OptError::from).and_then(|_| verify_start_type(b, &item.name, item.to).map_err(OptError::failed));
        match result {
            Ok(()) => applied.push(idx),
            Err(failure) => {
                let undo = undo_items(b, &plan.items, &applied);
                let mut all_restored = true;
                for (idx_applied, outcome) in &undo {
                    match outcome {
                        Ok(()) => {
                            let _ = ctx.db.finalize_pending_audit(audit_ids[*idx_applied], "rolledBack", "undone because a later change in the same plan failed");
                        }
                        Err(e) => {
                            all_restored = false;
                            let _ = ctx.db.finalize_pending_audit(audit_ids[*idx_applied], "partial", &format!("applied, but could not be undone automatically: {e}. Use Roll back in Optimization History."));
                        }
                    }
                }
                let _ = ctx.db.finalize_pending_audit(audit_ids[idx], "failed", failure.message.as_str());
                for later in idx + 1..plan.items.len() {
                    let _ = ctx.db.finalize_pending_audit(audit_ids[later], "skipped", "not attempted because an earlier change failed");
                }
                if all_restored {
                    let _ = ctx.db.mark_snapshot_rolled_back(snapshot_id);
                    return Err(OptError::new(failure.code, format!("{}. Nothing was changed: the steps already applied were undone.", failure.message)));
                }
                return Err(OptError::new(
                    failure.code,
                    format!("{}. Some applied changes could not be undone automatically; use Roll back in Optimization History to restore them.", failure.message),
                ));
            }
        }
    }

    // 5. Success.
    for (idx, item) in plan.items.iter().enumerate() {
        let _ = ctx.db.finalize_pending_audit(audit_ids[idx], "completed", &format!("verified startType={}", item.to.as_str()));
    }
    let applied_items: Vec<AppliedService> = plan.items.iter().map(|i| AppliedService { name: i.name.clone(), display_name: i.display_name.clone(), from: i.from, to: i.to }).collect();
    let summary = format!("{} service(s) changed and verified. Snapshot #{snapshot_id} can undo this.", applied_items.len());
    Ok(ServiceApplyOutcome { snapshot_id, applied: applied_items, summary })
}

/// Restores the pre-change start type of `applied` items, newest first.
fn undo_items(b: &Backends, items: &[super::service_plan::PlanItem], applied: &[usize]) -> Vec<(usize, Result<(), String>)> {
    applied
        .iter()
        .rev()
        .map(|&idx| {
            let item = &items[idx];
            let r = b.services.set_start_type(&item.name, item.from, None).map_err(|e| e.message().to_string()).and_then(|_| verify_start_type(b, &item.name, item.from));
            (idx, r)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Power profiles
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOutcome {
    pub snapshot_id: i64,
    pub profile_id: String,
    pub detail: String,
}

/// Switches the Windows power scheme and applies the profile's EcoQoS policy.
pub fn apply_profile(ctx: &Ctx, profile_id: &str, origin: Origin) -> Result<ProfileOutcome, OptError> {
    let _guard = lock();
    let b = ctx.backends;
    let profile: PowerProfile = profiles::find_profile(ctx.db, profile_id).ok_or_else(|| OptError::new(ErrorKind::NotFound, format!("unknown power profile: {profile_id}")))?;
    let guid = profile.scheme_guid.clone().ok_or_else(|| OptError::rejected("profile has no Windows scheme GUID"))?;

    // A restorable snapshot is mandatory: refuse when the current scheme
    // cannot be identified.
    let before = b.power.status().map_err(OptError::failed)?;
    let previous_guid = before
        .active_scheme_guid
        .clone()
        .ok_or_else(|| OptError::failed("could not determine the active power scheme; refusing to change it without a rollback snapshot"))?;
    let previous = profiles::get_active(ctx.db);
    let mut payload = json!({
        "profileId": profile.id,
        "schemeGuid": previous_guid,
        "schemeName": before.active_scheme_name.clone(),
        "ecoQosForBackground": profile.eco_qos_for_background,
        "ecoQosMode": profile.eco_qos_mode,
        "previousProfileId": previous.as_ref().map(|p| p.profile_id.clone()),
        "origin": origin.as_str(),
    });
    let snapshot_id = ctx.db.insert_optimizer_snapshot("powerScheme", &profile.label, &payload.to_string()).map_err(db_err)?;
    let audit_id = ctx.db.insert_optimizer_audit("power", &profile.label, "applyProfile", "pending", Some(origin.as_str()), Some(snapshot_id)).map_err(db_err)?;
    if let Err(e) = b.power.set_scheme(&guid) {
        let _ = ctx.db.complete_optimizer_audit(audit_id, "failed", Some(&e));
        let _ = ctx.db.mark_snapshot_rolled_back(snapshot_id);
        return Err(OptError::failed(e));
    }
    // The scheme is applied from here on; a failed verification re-read is
    // reported as applied-unverified, never as a failed change.
    let scheme_detail = match b.power.status() {
        Ok(after) => {
            let verified = after.active_scheme_guid.as_deref().map(|g| g.eq_ignore_ascii_case(&guid)).unwrap_or(false);
            format!("active scheme now {} ({})", after.active_scheme_name.clone().unwrap_or_default(), if verified { "verified" } else { "differs from requested GUID" })
        }
        Err(e) => format!("scheme applied; verification re-read failed: {e}"),
    };
    // Apply the profile's EcoQoS promise: throttle eligible background
    // processes, or restore the ones WinClean throttled earlier.
    let eco = if profile.eco_qos_mode != EcoQosMode::Off {
        ecoqos::throttle_eligible(ctx.db, b, profile.eco_qos_mode, snapshot_id)
    } else {
        match ecoqos::owned_candidates(ctx.db) {
            Ok(candidates) => ecoqos::restore(ctx.db, b, candidates, "via power profile", Some(snapshot_id)),
            Err(e) => ecoqos::EcoQosOutcome { summary: format!("could not read throttled processes: {e}"), touched: Vec::new(), incomplete: 0 },
        }
    };
    // Record exactly which processes this profile changed so rollback can
    // undo them (the scheme GUID alone cannot).
    let key = if profile.eco_qos_mode != EcoQosMode::Off { "ecoQosEnabled" } else { "ecoQosRestored" };
    payload[key] = json!(eco.touched);
    let snapshot_note = match ctx.db.update_optimizer_snapshot_payload(snapshot_id, &payload.to_string()) {
        Ok(()) => String::new(),
        Err(e) => format!("; WARNING: could not record processes for rollback: {e}"),
    };
    let detail = format!("{scheme_detail}; EcoQoS: {}{snapshot_note}", eco.summary);
    let _ = ctx.db.complete_optimizer_audit(audit_id, "completed", Some(&detail));
    let _ = profiles::set_active(ctx.db, Some(&ActiveProfile { profile_id: profile.id.clone(), applied_at_ms: ecoqos::now_ms(), applied_by: origin.as_str().to_string(), snapshot_id }));
    Ok(ProfileOutcome { snapshot_id, profile_id: profile.id, detail })
}

// ---------------------------------------------------------------------------
// Single-process EcoQoS (manual toggle in the Processes view)
// ---------------------------------------------------------------------------

pub fn set_process_ecoqos(ctx: &Ctx, pid: u32, name: &str, enable: bool) -> Result<(), OptError> {
    let _guard = lock();
    let action = if enable { "ecoqos:on" } else { "ecoqos:off" };
    let audit_id = ctx.db.insert_optimizer_audit("process", &super::processes::process_target(name, pid), action, "pending", None, None).map_err(db_err)?;
    // The setter revalidates the PID's real image name, foreground state and
    // live policy verdict before enabling; restore (off) always passes
    // identity revalidation only.
    match ctx.backends.processes.set_ecoqos(pid, name, None, enable, EcoQosMode::Conservative) {
        Ok(real) => {
            let _ = ctx.db.complete_optimizer_audit(audit_id, "completed", Some(&format!("verified process: {real}")));
            Ok(())
        }
        Err(e) => {
            let _ = ctx.db.complete_optimizer_audit(audit_id, "failed", Some(&e));
            Err(OptError::failed(e))
        }
    }
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

/// Restores the state recorded in a snapshot. Partial rollbacks stay
/// retryable: the snapshot is only marked rolled back when everything was
/// restored and verified.
pub fn rollback(ctx: &Ctx, snapshot_id: i64) -> Result<String, OptError> {
    let _guard = lock();
    let b = ctx.backends;
    let snapshot = ctx.db.get_optimizer_snapshot(snapshot_id).map_err(db_err)?.ok_or_else(|| OptError::new(ErrorKind::NotFound, format!("snapshot {snapshot_id} not found")))?;
    if snapshot.rolled_back_at_ms.is_some() {
        return Err(OptError::new(ErrorKind::Conflict, "snapshot was already rolled back"));
    }
    let payload: serde_json::Value = serde_json::from_str(&snapshot.payload_json).map_err(|e| OptError::failed(e.to_string()))?;
    // Steps that should have been restored but could not be; the snapshot then
    // stays retryable instead of being marked rolled back.
    let mut incomplete = 0usize;
    let result: Result<String, OptError> = match snapshot.kind.as_str() {
        "service" | "serviceBatch" => rollback_services(b, &payload, snapshot.kind == "serviceBatch").map(|(msg, missed)| {
            incomplete = missed;
            msg
        }),
        "powerScheme" => rollback_profile(ctx, &payload, snapshot_id, snapshot.created_at_ms).map(|(msg, missed)| {
            incomplete = missed;
            msg
        }),
        other => Err(OptError::rejected(format!("rollback is not supported for snapshot kind '{other}'"))),
    };
    match result {
        Ok(msg) if incomplete > 0 => {
            let msg = format!("{msg}. {incomplete} item(s) could not be restored; this rollback stays available to retry.");
            let _ = ctx.db.insert_optimizer_audit(&snapshot.kind, &snapshot.target, "rollback", "partial", Some(&msg), Some(snapshot_id));
            Ok(msg)
        }
        Ok(msg) => {
            ctx.db.mark_snapshot_rolled_back(snapshot_id).map_err(db_err)?;
            let _ = ctx.db.insert_optimizer_audit(&snapshot.kind, &snapshot.target, "rollback", "completed", Some(&msg), Some(snapshot_id));
            Ok(msg)
        }
        Err(e) => {
            let _ = ctx.db.insert_optimizer_audit(&snapshot.kind, &snapshot.target, "rollback", "failed", Some(&e.message), Some(snapshot_id));
            Err(e)
        }
    }
}

fn rollback_services(b: &Backends, payload: &serde_json::Value, batch: bool) -> Result<(String, usize), OptError> {
    if b.services.write_capability() == WriteCapability::Unavailable {
        return Err(OptError::new(ErrorKind::ElevationRequired, ELEVATION_MESSAGE));
    }
    let items: Vec<serde_json::Value> = if batch { payload.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default() } else { vec![payload.clone()] };
    if items.is_empty() {
        return Err(OptError::failed("snapshot contains no services"));
    }
    // Validate that every item is restorable before changing anything.
    let mut targets = Vec::new();
    for item in &items {
        let name = item.get("name").and_then(|v| v.as_str()).ok_or_else(|| OptError::failed("snapshot item has no service name"))?;
        let start = start_type_from_snapshot(item).ok_or_else(|| OptError::failed(format!("snapshot has no restorable start type for {name}")))?;
        targets.push((name.to_string(), start));
    }
    let mut restored = 0usize;
    let mut missed = 0usize;
    let mut notes = Vec::new();
    for (name, start) in targets.iter().rev() {
        let services = b.services.list().map_err(OptError::failed)?;
        if find(&services, name).and_then(|s| s.current_start_type()) == Some(*start) {
            restored += 1; // already at the recorded state
            continue;
        }
        match b.services.set_start_type(name, *start, None).map_err(|e| e.message().to_string()).and_then(|_| verify_start_type(b, name, *start)) {
            Ok(()) => restored += 1,
            Err(e) => {
                missed += 1;
                notes.push(format!("{name}: {e}"));
            }
        }
    }
    let mut msg = format!("{restored} of {} service(s) restored", targets.len());
    if !notes.is_empty() {
        msg.push_str(&format!(" ({})", notes.join("; ")));
    }
    Ok((msg, missed))
}

fn rollback_profile(ctx: &Ctx, payload: &serde_json::Value, snapshot_id: i64, created_ms: i64) -> Result<(String, usize), OptError> {
    let b = ctx.backends;
    let guid = payload.get("schemeGuid").and_then(|v| v.as_str()).ok_or_else(|| OptError::failed("snapshot has no previous scheme GUID"))?;
    b.power.set_scheme(guid).map_err(OptError::failed)?;
    let scheme = format!("power scheme restored to {guid}");
    let mut incomplete = 0usize;
    let msg = match ecoqos::rollback_profile(ctx.db, b, payload, snapshot_id, created_ms) {
        Some((eco, missed)) => {
            incomplete = missed;
            format!("{scheme}; EcoQoS: {eco}")
        }
        None => scheme,
    };
    // If this profile is still the active one, the previous profile is active again.
    if profiles::get_active(ctx.db).map(|a| a.snapshot_id == snapshot_id).unwrap_or(false) {
        let previous = payload.get("previousProfileId").and_then(|v| v.as_str());
        let restored = previous.map(|id| ActiveProfile { profile_id: id.to_string(), applied_at_ms: ecoqos::now_ms(), applied_by: "rollback".into(), snapshot_id: 0 });
        let _ = profiles::set_active(ctx.db, restored.as_ref());
    }
    Ok((msg, incomplete))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::backend::fakes::{proc, FakePower, FakeProcesses, FakeServices, Fakes};
    use crate::optimizer::policy::{SCHEME_BALANCED, SCHEME_HIGH_PERFORMANCE, SCHEME_POWER_SAVER};
    use crate::optimizer::backend::ServiceBackend;
    use crate::optimizer::privileged::ScmAccess;
    use crate::optimizer::service_graph::testing::svc;
    use std::sync::Arc;

    struct Rig {
        _dir: tempfile::TempDir,
        db: Database,
        fakes: Fakes,
    }

    fn rig(capability: WriteCapability, procs: Vec<crate::optimizer::backend::fakes::FakeProc>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        let services = vec![
            svc("WinDefend", "Microsoft Defender Antivirus Service", "automatic", "running", &["RpcSs"]),
            svc("RpcSs", "Remote Procedure Call (RPC)", "automatic", "running", &[]),
            svc("AsusOptimization", "ASUS Optimization", "automatic", "running", &[]),
            svc("DiagTrack", "Connected User Experiences and Telemetry", "automatic", "running", &[]),
            svc("MapsBroker", "Downloaded Maps Manager", "automaticDelayed", "stopped", &[]),
            svc("Fax", "Fax", "manual", "stopped", &[]),
            svc("MysteryVendorSvc", "Mystery", "automatic", "running", &[]),
        ];
        let fakes = Fakes {
            services: Arc::new(FakeServices::new(services, capability)),
            processes: Arc::new(FakeProcesses::new(procs)),
            power: Arc::new(FakePower::new(SCHEME_BALANCED)),
        };
        Rig { _dir: dir, db, fakes }
    }

    fn change(name: &str, to: &str) -> ServiceChange {
        ServiceChange { service_name: name.into(), start_type: to.into() }
    }

    fn with<R>(r: &Rig, f: impl FnOnce(&Ctx) -> R) -> R {
        let backends = r.fakes.backends();
        f(&Ctx { db: &r.db, backends: &backends })
    }

    fn statuses(db: &Database) -> Vec<(String, String)> {
        let mut rows: Vec<_> = db.list_optimizer_audit(100).unwrap().into_iter().filter(|a| a.kind == "service").map(|a| (a.target, a.status)).collect();
        rows.reverse();
        rows
    }

    // ---- service transactions -------------------------------------------------

    #[test]
    fn a_service_batch_is_snapshotted_applied_verified_and_audited() {
        let r = rig(WriteCapability::Helper, vec![]);
        let out = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual"), change("MapsBroker", "manual")], None, Origin::User)).unwrap();
        assert_eq!(out.applied.len(), 2);
        assert_eq!(r.fakes.services.scm.start_type("DiagTrack"), "manual");
        assert_eq!(r.fakes.services.scm.start_type("MapsBroker"), "manual");
        let snap = r.db.get_optimizer_snapshot(out.snapshot_id).unwrap().unwrap();
        assert_eq!(snap.kind, "serviceBatch");
        assert!(snap.payload_json.contains("\"automaticDelayed\"") || snap.payload_json.contains("automaticDelayed"));
        assert_eq!(statuses(&r.db), vec![("DiagTrack".into(), "completed".into()), ("MapsBroker".into(), "completed".into())]);
    }

    #[test]
    fn without_a_privileged_path_nothing_is_recorded_or_changed() {
        let r = rig(WriteCapability::Unavailable, vec![]);
        let err = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual")], None, Origin::User)).unwrap_err();
        assert_eq!(err.code, ErrorKind::ElevationRequired);
        assert!(r.fakes.services.scm.writes().is_empty());
        assert!(r.db.list_optimizer_snapshots(10).unwrap().is_empty(), "no snapshot before the privilege preflight passes");
        assert!(r.db.list_optimizer_audit(10).unwrap().is_empty());
    }

    #[test]
    fn protected_unknown_and_vendor_services_are_rejected_before_any_state_changes() {
        let r = rig(WriteCapability::Helper, vec![]);
        for bad in [change("WinDefend", "disabled"), change("RpcSs", "manual"), change("AsusOptimization", "manual"), change("MysteryVendorSvc", "disabled"), change("Nope", "manual")] {
            let err = with(&r, |c| apply_service_changes(c, &[bad.clone()], None, Origin::User)).unwrap_err();
            assert_eq!(err.code, ErrorKind::Rejected, "{bad:?}");
        }
        // A bad item anywhere in the batch rejects the whole batch.
        let err = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual"), change("WinDefend", "disabled")], None, Origin::User)).unwrap_err();
        assert_eq!(err.code, ErrorKind::Rejected);
        assert!(r.fakes.services.scm.writes().is_empty());
        assert!(r.db.list_optimizer_snapshots(10).unwrap().is_empty());
    }

    #[test]
    fn a_plan_reviewed_earlier_does_not_apply_when_the_system_moved() {
        let r = rig(WriteCapability::Helper, vec![]);
        let services = r.fakes.services.list().unwrap();
        let reviewed = validate_service_plan(&[change("DiagTrack", "manual")], &services).fingerprint();
        // Someone else changes the service after review.
        r.fakes.services.scm.write_start_type("DiagTrack", StartType::Disabled).unwrap();
        let before = r.fakes.services.scm.writes().len();
        let err = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual")], Some(&reviewed), Origin::AiApproved)).unwrap_err();
        assert_eq!(err.code, ErrorKind::Conflict);
        assert_eq!(r.fakes.services.scm.writes().len(), before, "no further write after the conflict");
    }

    #[test]
    fn a_failure_part_way_undoes_the_steps_already_applied() {
        let r = rig(WriteCapability::Helper, vec![]);
        // Fax is manual; asking for automatic on it is valid. Make the second write fail by
        // removing the helper after the first change via a failing second service.
        struct Flaky {
            inner: Arc<FakeServices>,
            fail_on: &'static str,
        }
        impl crate::optimizer::backend::ServiceBackend for Flaky {
            fn list(&self) -> Result<Vec<ServiceInfo>, String> {
                self.inner.list()
            }
            fn write_capability(&self) -> WriteCapability {
                WriteCapability::Helper
            }
            fn set_start_type(&self, name: &str, target: StartType, expect: Option<StartType>) -> Result<(), BackendError> {
                if name == self.fail_on && target != StartType::Automatic {
                    return Err(BackendError::Failed("Access is denied. (os error 5)".into()));
                }
                crate::optimizer::backend::ServiceBackend::set_start_type(self.inner.as_ref(), name, target, expect)
            }
        }
        let mut backends = r.fakes.backends();
        backends.services = Arc::new(Flaky { inner: r.fakes.services.clone(), fail_on: "MapsBroker" });
        let ctx = Ctx { db: &r.db, backends: &backends };
        let err = apply_service_changes(&ctx, &[change("DiagTrack", "manual"), change("MapsBroker", "manual")], None, Origin::User).unwrap_err();
        assert!(err.message.contains("Nothing was changed"), "{}", err.message);
        assert_eq!(r.fakes.services.scm.start_type("DiagTrack"), "automatic", "the first change must be undone");
        assert_eq!(r.fakes.services.scm.start_type("MapsBroker"), "automaticDelayed");
        assert_eq!(statuses(&r.db), vec![("DiagTrack".into(), "rolledBack".into()), ("MapsBroker".into(), "failed".into())]);
        let snap = &r.db.list_optimizer_snapshots(10).unwrap()[0];
        assert!(snap.rolled_back_at_ms.is_some(), "a fully undone transaction is closed");
    }

    #[test]
    fn a_write_that_does_not_stick_is_a_failure_not_a_success() {
        let mut r = rig(WriteCapability::Helper, vec![]);
        let services = r.fakes.services.list().unwrap();
        r.fakes.services = Arc::new({
            let mut f = FakeServices::new(services, WriteCapability::Helper);
            f.scm.swallow_writes = true;
            f
        });
        let err = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual")], None, Origin::User)).unwrap_err();
        assert_eq!(err.code, ErrorKind::Failed);
        assert_eq!(statuses(&r.db)[0].1, "failed");
    }

    #[test]
    fn rollback_restores_every_service_and_is_not_repeatable() {
        let r = rig(WriteCapability::Helper, vec![]);
        let out = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "disabled"), change("MapsBroker", "manual")], None, Origin::User)).unwrap();
        let msg = with(&r, |c| rollback(c, out.snapshot_id)).unwrap();
        assert!(msg.starts_with("2 of 2"), "{msg}");
        assert_eq!(r.fakes.services.scm.start_type("DiagTrack"), "automatic");
        assert_eq!(r.fakes.services.scm.start_type("MapsBroker"), "automaticDelayed");
        let again = with(&r, |c| rollback(c, out.snapshot_id)).unwrap_err();
        assert_eq!(again.code, ErrorKind::Conflict);
    }

    #[test]
    fn rollback_without_the_helper_asks_for_elevation_and_stays_available() {
        let r = rig(WriteCapability::Helper, vec![]);
        let out = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual")], None, Origin::User)).unwrap();
        *r.fakes.services.capability.lock().unwrap() = WriteCapability::Unavailable;
        let err = with(&r, |c| rollback(c, out.snapshot_id)).unwrap_err();
        assert_eq!(err.code, ErrorKind::ElevationRequired);
        assert!(r.db.get_optimizer_snapshot(out.snapshot_id).unwrap().unwrap().rolled_back_at_ms.is_none());
        *r.fakes.services.capability.lock().unwrap() = WriteCapability::Helper;
        assert!(with(&r, |c| rollback(c, out.snapshot_id)).is_ok());
    }

    #[test]
    fn legacy_single_service_snapshots_still_roll_back() {
        let r = rig(WriteCapability::Helper, vec![]);
        r.fakes.services.scm.write_start_type("DiagTrack", StartType::Disabled).unwrap();
        let id = r.db.insert_optimizer_snapshot("service", "DiagTrack", r#"{"name":"DiagTrack","displayName":"x","startType":"automatic","delayedAutoStart":false,"state":"running"}"#).unwrap();
        with(&r, |c| rollback(c, id)).unwrap();
        assert_eq!(r.fakes.services.scm.start_type("DiagTrack"), "automatic");
    }

    #[test]
    fn a_partial_rollback_stays_retryable() {
        let r = rig(WriteCapability::Helper, vec![]);
        let out = with(&r, |c| apply_service_changes(c, &[change("DiagTrack", "manual"), change("MapsBroker", "manual")], None, Origin::User)).unwrap();
        // The SCM starts refusing writes (e.g. the helper's token lost rights).
        let services = r.fakes.services.list().unwrap();
        let mut broken = FakeServices::new(services, WriteCapability::Helper);
        broken.scm.fail_writes = Some("Access is denied.".into());
        let backends = Backends { services: Arc::new(broken), processes: r.fakes.processes.clone(), power: r.fakes.power.clone() };
        let ctx = Ctx { db: &r.db, backends: &backends };
        let msg = rollback(&ctx, out.snapshot_id).unwrap();
        assert!(msg.contains("stays available to retry"), "{msg}");
        assert!(r.db.get_optimizer_snapshot(out.snapshot_id).unwrap().unwrap().rolled_back_at_ms.is_none());
    }

    // ---- profiles and EcoQoS ----------------------------------------------------

    fn eco_rig() -> Rig {
        r_with_procs()
    }

    fn r_with_procs() -> Rig {
        rig(
            WriteCapability::Helper,
            vec![
                proc(10, "GoogleUpdate.exe", 0.5, false, 1_000),
                proc(11, "OneDrive.exe", 0.0, false, 1_000),
                proc(12, "Code.exe", 1.0, false, 1_000),
                proc(13, "unknown.exe", 0.0, false, 1_000),
                proc(14, "SearchIndexer.exe", 80.0, false, 1_000),
                proc(15, "Dropbox.exe", 0.0, false, 1_000),
            ],
        )
    }

    #[test]
    fn a_conservative_profile_throttles_only_eligible_background_work() {
        let r = eco_rig();
        let out = with(&r, |c| apply_profile(c, "balanced", Origin::User)).unwrap();
        assert!(r.fakes.processes.throttled(10) && r.fakes.processes.throttled(11), "updater and OneDrive are eligible");
        assert!(!r.fakes.processes.throttled(12), "editors are never throttled");
        assert!(!r.fakes.processes.throttled(13), "unknown processes are never throttled");
        assert!(!r.fakes.processes.throttled(14), "a busy indexer is not throttled");
        assert!(!r.fakes.processes.throttled(15), "Dropbox needs the battery mode");
        assert!(out.detail.contains("2 process(es) throttled"), "{}", out.detail);
    }

    #[test]
    fn battery_saver_widens_eligibility_but_still_spares_protected_processes() {
        let r = eco_rig();
        with(&r, |c| apply_profile(c, "batterySaver", Origin::User)).unwrap();
        assert_eq!(r.fakes.power.scheme(), SCHEME_POWER_SAVER);
        assert!(r.fakes.processes.throttled(15), "Dropbox is throttled in battery mode");
        assert!(!r.fakes.processes.throttled(12), "Code.exe stays protected in every mode");
    }

    #[test]
    fn the_foreground_app_is_never_throttled() {
        let r = rig(WriteCapability::Helper, vec![proc(20, "OneDrive.exe", 0.0, true, 1_000)]);
        with(&r, |c| apply_profile(c, "batterySaver", Origin::User)).unwrap();
        assert!(!r.fakes.processes.throttled(20));
    }

    #[test]
    fn switching_to_gaming_restores_exactly_what_winclean_throttled() {
        let r = eco_rig();
        with(&r, |c| apply_profile(c, "batterySaver", Origin::User)).unwrap();
        // Another app throttles Notepad-like process 13 on its own; WinClean does not own it.
        r.fakes.processes.procs.lock().unwrap().iter_mut().find(|p| p.info.pid == 13).unwrap().info.eco_qos_applied = true;
        with(&r, |c| apply_profile(c, "gaming", Origin::User)).unwrap();
        assert_eq!(r.fakes.power.scheme(), SCHEME_HIGH_PERFORMANCE);
        assert!(!r.fakes.processes.throttled(10) && !r.fakes.processes.throttled(11) && !r.fakes.processes.throttled(15), "WinClean's throttles are released");
        assert!(r.fakes.processes.throttled(13), "EcoQoS set by another app is left alone");
        assert_eq!(profiles::get_active(&r.db).unwrap().profile_id, "gaming");
    }

    #[test]
    fn rolling_back_a_profile_restores_the_scheme_the_processes_and_the_previous_profile() {
        let r = eco_rig();
        let first = with(&r, |c| apply_profile(c, "balanced", Origin::User)).unwrap();
        let second = with(&r, |c| apply_profile(c, "batterySaver", Origin::User)).unwrap();
        assert!(r.fakes.processes.throttled(15));
        let msg = with(&r, |c| rollback(c, second.snapshot_id)).unwrap();
        assert!(msg.contains("power scheme restored"), "{msg}");
        assert_eq!(r.fakes.power.scheme(), SCHEME_BALANCED);
        assert!(!r.fakes.processes.throttled(15), "the throttle added by the saver is undone");
        assert!(r.fakes.processes.throttled(10), "the earlier balanced profile's throttle remains");
        assert_eq!(profiles::get_active(&r.db).unwrap().profile_id, "balanced");
        assert!(first.snapshot_id < second.snapshot_id);
    }

    #[test]
    fn rollback_does_not_clear_a_throttle_winclean_no_longer_owns() {
        let r = eco_rig();
        let out = with(&r, |c| apply_profile(c, "balanced", Origin::User)).unwrap();
        // The user restores process 10 by hand; later another app throttles it again.
        with(&r, |c| set_process_ecoqos(c, 10, "GoogleUpdate.exe", false)).unwrap();
        r.fakes.processes.procs.lock().unwrap().iter_mut().find(|p| p.info.pid == 10).unwrap().info.eco_qos_applied = true;
        with(&r, |c| rollback(c, out.snapshot_id)).unwrap();
        assert!(r.fakes.processes.throttled(10), "an old profile rollback must not clear someone else's EcoQoS");
        assert!(!r.fakes.processes.throttled(11), "but the throttle WinClean still owns is released");
    }

    #[test]
    fn a_reused_pid_is_never_restored() {
        let r = eco_rig();
        let out = with(&r, |c| apply_profile(c, "balanced", Origin::User)).unwrap();
        {
            // PID 11 exits and the OS hands the number to a brand-new process.
            let mut procs = r.fakes.processes.procs.lock().unwrap();
            let p = procs.iter_mut().find(|p| p.info.pid == 11).unwrap();
            p.started_ms = ecoqos::now_ms() + 60_000;
            p.info.eco_qos_applied = true;
        }
        with(&r, |c| rollback(c, out.snapshot_id)).unwrap();
        assert!(r.fakes.processes.throttled(11), "the newer process is not WinClean's to touch");
    }

    #[test]
    fn a_failed_scheme_switch_changes_nothing_else() {
        let r = eco_rig();
        *r.fakes.power.fail_set.lock().unwrap() = true;
        let err = with(&r, |c| apply_profile(c, "batterySaver", Origin::User)).unwrap_err();
        assert_eq!(err.code, ErrorKind::Failed);
        assert!(!r.fakes.processes.throttled(10), "no EcoQoS change when the scheme switch failed");
        assert!(profiles::get_active(&r.db).is_none());
        let audit = r.db.list_optimizer_audit(5).unwrap();
        assert_eq!(audit[0].status, "failed");
    }

    #[test]
    fn unknown_profiles_are_not_found() {
        let r = eco_rig();
        assert_eq!(with(&r, |c| apply_profile(c, "turbo", Origin::User)).unwrap_err().code, ErrorKind::NotFound);
        assert!(r.fakes.power.set_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn profile_switching_cycles_cleanly_through_every_builtin() {
        let r = eco_rig();
        for id in ["batterySaver", "balanced", "coding", "gaming", "aiDevelopment", "balanced"] {
            with(&r, |c| apply_profile(c, id, Origin::Auto)).unwrap();
            assert_eq!(profiles::get_active(&r.db).unwrap().profile_id, id);
            assert!(!r.fakes.processes.throttled(12), "Code.exe must never be throttled in {id}");
        }
        assert_eq!(r.fakes.power.scheme(), SCHEME_BALANCED);
    }
}
