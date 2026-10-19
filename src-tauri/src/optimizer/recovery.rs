//! Crash recovery: reconcile `pending` audit rows left by an interrupted app
//! or helper with what the system actually looks like now.
//!
//! Nothing is changed here — recovery only *classifies* the leftovers so the
//! history is truthful and so the ledger (which counts only completed rows)
//! knows about changes that did take effect. Anything that needs a decision
//! is left for the user as an available rollback.

use serde::Serialize;

use super::ecoqos::now_ms;
use super::orchestrator::Ctx;
use super::processes::{parse_process_target, ProcessLiveness};
use super::profiles;
use super::services::{start_type_from_snapshot, StartType};
use super::OptimizationAudit;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReport {
    pub examined: usize,
    /// Interrupted changes that turned out to have taken effect.
    pub completed: usize,
    /// Interrupted changes that did not take effect, or could not be judged.
    pub interrupted: usize,
    pub notes: Vec<String>,
}

pub fn recover_interrupted(ctx: &Ctx) -> RecoveryReport {
    let mut report = RecoveryReport::default();
    let pending = match ctx.db.pending_optimizer_audit() {
        Ok(p) => p,
        Err(e) => {
            report.notes.push(format!("could not read pending audit rows: {e}"));
            return report;
        }
    };
    // Read the live service list once; it can fail off-Windows.
    let services = ctx.backends.services.list().ok();
    for row in pending {
        report.examined += 1;
        let (status, detail) = judge(ctx, &row, services.as_deref());
        let _ = ctx.db.finalize_pending_audit(row.id, status, &detail);
        if status == "completed" {
            report.completed += 1;
        } else {
            report.interrupted += 1;
        }
        report.notes.push(format!("#{} {} {}: {status} ({detail})", row.id, row.kind, row.target));
    }
    report
}

fn judge(ctx: &Ctx, row: &OptimizationAudit, services: Option<&[super::services::ServiceInfo]>) -> (&'static str, String) {
    match row.kind.as_str() {
        "service" => judge_service(ctx, row, services),
        "process" => judge_process(ctx, row),
        "power" => judge_power(ctx, row),
        other => ("interrupted", format!("unfinished {other} operation found at startup")),
    }
}

fn judge_service(ctx: &Ctx, row: &OptimizationAudit, services: Option<&[super::services::ServiceInfo]>) -> (&'static str, String) {
    let Some(services) = services else {
        return ("interrupted", "interrupted; the service list could not be read to verify the outcome".into());
    };
    let to = row.action.strip_prefix("setStartType:").and_then(StartType::parse);
    let live = services.iter().find(|s| s.name.eq_ignore_ascii_case(&row.target)).and_then(|s| s.current_start_type());
    let before = row.snapshot_id.and_then(|id| ctx.db.get_optimizer_snapshot(id).ok().flatten()).and_then(|snap| {
        let payload: serde_json::Value = serde_json::from_str(&snap.payload_json).ok()?;
        let items = payload.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_else(|| vec![payload.clone()]);
        items.iter().find(|i| i.get("name").and_then(|n| n.as_str()).map(|n| n.eq_ignore_ascii_case(&row.target)).unwrap_or(false)).and_then(start_type_from_snapshot)
    });
    match (live, to, before) {
        (Some(l), Some(t), _) if l == t => ("completed", format!("change took effect before WinClean stopped (now {}); roll back from history if unwanted", l.as_str())),
        (Some(l), _, Some(b)) if l == b => ("interrupted", "interrupted before the change took effect; the service is unchanged".into()),
        (Some(l), _, _) => ("interrupted", format!("interrupted; the service is now {} which matches neither the old nor the new setting. Review it in Windows Services", l.as_str())),
        (None, _, _) => ("interrupted", "interrupted; the service no longer exists or cannot be read".into()),
    }
}

fn judge_process(ctx: &Ctx, row: &OptimizationAudit) -> (&'static str, String) {
    let Some((_, pid)) = parse_process_target(&row.target) else {
        return ("interrupted", "interrupted; unreadable process target".into());
    };
    let want_on = row.action == "ecoqos:on";
    match ctx.backends.processes.liveness(pid) {
        ProcessLiveness::Gone => ("interrupted", "the process exited before the change could be confirmed".into()),
        ProcessLiveness::Unknown => ("interrupted", "the process could not be inspected".into()),
        ProcessLiveness::Alive { started_ms } if started_ms > row.created_at_ms => ("interrupted", "the PID now belongs to a newer process".into()),
        ProcessLiveness::Alive { .. } => match ctx.backends.processes.ecoqos_state(pid) {
            Some(state) if state == want_on => ("completed", format!("EcoQoS is {} as requested", if want_on { "on" } else { "off" })),
            Some(_) => ("interrupted", "EcoQoS did not reach the requested state".into()),
            None => ("interrupted", "the EcoQoS state could not be read".into()),
        },
    }
}

fn judge_power(ctx: &Ctx, row: &OptimizationAudit) -> (&'static str, String) {
    let wanted = row.snapshot_id.and_then(|id| ctx.db.get_optimizer_snapshot(id).ok().flatten()).and_then(|snap| serde_json::from_str::<serde_json::Value>(&snap.payload_json).ok()).and_then(|p| p.get("profileId").and_then(|v| v.as_str()).map(|s| s.to_string()));
    let target_guid = wanted.and_then(|id| profiles::find_profile(ctx.db, &id)).and_then(|p| p.scheme_guid);
    let active = ctx.backends.power.status().ok().and_then(|s| s.active_scheme_guid);
    match (target_guid, active) {
        (Some(t), Some(a)) if t.eq_ignore_ascii_case(&a) => ("completed", "the power scheme was already switched when WinClean stopped".into()),
        (Some(_), Some(_)) => ("interrupted", "interrupted before the power scheme changed".into()),
        _ => ("interrupted", "interrupted; the power scheme could not be verified".into()),
    }
}

/// Convenience used at startup: recover, and remember when it ran.
pub fn recover_at_startup(ctx: &Ctx) -> RecoveryReport {
    let report = recover_interrupted(ctx);
    if report.examined > 0 {
        let _ = ctx.db.set_optimizer_setting("recovery.lastRunMs", &now_ms().to_string());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::optimizer::backend::fakes::{proc, FakePower, FakeProcesses, FakeServices, Fakes};
    use crate::optimizer::backend::WriteCapability;
    use crate::optimizer::policy::SCHEME_BALANCED;
    use crate::optimizer::privileged::ScmAccess;
    use crate::optimizer::service_graph::testing::svc;
    use std::sync::Arc;

    fn rig() -> (tempfile::TempDir, Database, Fakes) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        let fakes = Fakes {
            services: Arc::new(FakeServices::new(vec![svc("DiagTrack", "Connected User Experiences and Telemetry", "automatic", "running", &[])], WriteCapability::Helper)),
            processes: Arc::new(FakeProcesses::new(vec![proc(10, "GoogleUpdate.exe", 0.0, false, 1_000)])),
            power: Arc::new(FakePower::new(SCHEME_BALANCED)),
        };
        (dir, db, fakes)
    }

    fn pending_service(db: &Database, to: &str) -> i64 {
        let snap = db.insert_optimizer_snapshot("serviceBatch", "DiagTrack", r#"{"items":[{"name":"DiagTrack","startType":"automatic","delayedAutoStart":false}]}"#).unwrap();
        db.insert_optimizer_audit("service", "DiagTrack", &format!("setStartType:{to}"), "pending", None, Some(snap)).unwrap()
    }

    fn status_of(db: &Database, id: i64) -> String {
        db.list_optimizer_audit(50).unwrap().into_iter().find(|a| a.id == id).unwrap().status
    }

    #[test]
    fn an_interrupted_service_change_that_took_effect_is_completed() {
        let (_d, db, fakes) = rig();
        let id = pending_service(&db, "manual");
        fakes.services.scm.write_start_type("DiagTrack", StartType::Manual).unwrap();
        let b = fakes.backends();
        let report = recover_interrupted(&Ctx { db: &db, backends: &b });
        assert_eq!((report.examined, report.completed, report.interrupted), (1, 1, 0));
        assert_eq!(status_of(&db, id), "completed");
    }

    #[test]
    fn an_interrupted_service_change_that_never_happened_is_marked_interrupted() {
        let (_d, db, fakes) = rig();
        let id = pending_service(&db, "manual");
        let b = fakes.backends();
        let report = recover_interrupted(&Ctx { db: &db, backends: &b });
        assert_eq!((report.completed, report.interrupted), (0, 1));
        assert_eq!(status_of(&db, id), "interrupted");
        assert!(db.pending_optimizer_audit().unwrap().is_empty(), "no row stays pending forever");
    }

    #[test]
    fn a_service_in_an_unexpected_state_is_flagged_for_review() {
        let (_d, db, fakes) = rig();
        let id = pending_service(&db, "manual");
        fakes.services.scm.write_start_type("DiagTrack", StartType::Disabled).unwrap();
        let b = fakes.backends();
        recover_interrupted(&Ctx { db: &db, backends: &b });
        let row = db.list_optimizer_audit(10).unwrap().into_iter().find(|a| a.id == id).unwrap();
        assert_eq!(row.status, "interrupted");
        assert!(row.detail.unwrap().contains("matches neither"));
    }

    #[test]
    fn an_interrupted_ecoqos_change_becomes_owned_only_if_it_really_happened() {
        let (_d, db, fakes) = rig();
        let id = db.insert_optimizer_audit("process", "GoogleUpdate.exe (10)", "ecoqos:on", "pending", None, None).unwrap();
        fakes.processes.procs.lock().unwrap()[0].info.eco_qos_applied = true;
        let b = fakes.backends();
        recover_interrupted(&Ctx { db: &db, backends: &b });
        assert_eq!(status_of(&db, id), "completed");
        assert_eq!(db.ecoqos_owned_processes().unwrap().len(), 1, "the throttle is now restorable");
    }

    #[test]
    fn exited_or_reused_processes_are_not_claimed() {
        let (_d, db, fakes) = rig();
        let gone = db.insert_optimizer_audit("process", "Gone.exe (99)", "ecoqos:on", "pending", None, None).unwrap();
        fakes.processes.procs.lock().unwrap()[0].started_ms = ecoqos_future();
        fakes.processes.procs.lock().unwrap()[0].info.eco_qos_applied = true;
        let reused = db.insert_optimizer_audit("process", "GoogleUpdate.exe (10)", "ecoqos:on", "pending", None, None).unwrap();
        let b = fakes.backends();
        recover_interrupted(&Ctx { db: &db, backends: &b });
        assert_eq!(status_of(&db, gone), "interrupted");
        assert_eq!(status_of(&db, reused), "interrupted");
        assert!(db.ecoqos_owned_processes().unwrap().is_empty());
    }

    fn ecoqos_future() -> i64 {
        now_ms() + 3_600_000
    }

    #[test]
    fn an_interrupted_profile_switch_is_judged_by_the_active_scheme() {
        let (_d, db, fakes) = rig();
        let snap = db.insert_optimizer_snapshot("powerScheme", "Battery Saver", r#"{"profileId":"batterySaver","schemeGuid":"x"}"#).unwrap();
        let a = db.insert_optimizer_audit("power", "Battery Saver", "applyProfile", "pending", None, Some(snap)).unwrap();
        let b = fakes.backends();
        recover_interrupted(&Ctx { db: &db, backends: &b });
        assert_eq!(status_of(&db, a), "interrupted", "scheme is still Balanced");

        let a2 = db.insert_optimizer_audit("power", "Battery Saver", "applyProfile", "pending", None, Some(snap)).unwrap();
        *fakes.power.scheme.lock().unwrap() = crate::optimizer::policy::SCHEME_POWER_SAVER.to_string();
        recover_interrupted(&Ctx { db: &db, backends: &b });
        assert_eq!(status_of(&db, a2), "completed");
    }

    #[test]
    fn nothing_pending_means_nothing_to_do() {
        let (_d, db, fakes) = rig();
        let b = fakes.backends();
        assert_eq!(recover_interrupted(&Ctx { db: &db, backends: &b }), RecoveryReport::default());
    }
}
