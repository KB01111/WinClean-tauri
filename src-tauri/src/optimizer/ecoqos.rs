//! EcoQoS ownership ledger: throttle eligible background work, and restore
//! exactly — and only — what WinClean throttled.
//!
//! Ownership is derived from the audit trail (`Database::ecoqos_owned_processes`):
//! a process is "owned" while its latest completed EcoQoS row is `ecoqos:on`.
//! Restore never touches a process that exited, a PID that was reused by a
//! newer process, or EcoQoS that another app set. Restoring writes
//! `ControlMask = 0` (the documented "system-managed default"), never an
//! explicit HighQoS pin.

use serde::{Deserialize, Serialize};

use crate::db::Database;

use super::backend::Backends;
use super::policy::{classify_process_for_ecoqos_mode, EcoQosDecision, EcoQosMode};
use super::processes::{parse_process_target, process_target, ProcessLiveness};

/// A process whose EcoQoS state a power profile changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EcoQosTouched {
    pub pid: u32,
    pub name: String,
    /// When WinClean changed this process. Rollback treats a process created
    /// after this moment as a PID reuse; absent in older snapshots, where the
    /// snapshot time is used instead.
    #[serde(default, rename = "sinceMs")]
    pub since_ms: Option<i64>,
}

pub struct EcoQosOutcome {
    pub summary: String,
    pub touched: Vec<EcoQosTouched>,
    /// Processes that should have been restored but could not be (liveness
    /// unknown, access denied). A rollback with any of these stays retryable.
    pub incomplete: usize,
}

/// A process WinClean may own EcoQoS throttling for: `(name, pid, since_ms)`.
pub type EcoQosCandidate = (String, u32, i64);

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `snapshot_id` links a real change to the profile snapshot that can undo
/// it; releases and rollback rows pass `None` so they are never mistaken for
/// changes when a snapshot is rolled back again.
pub fn record_audit(db: &Database, name: &str, pid: u32, enable: bool, detail: &str, snapshot_id: Option<i64>) {
    let action = if enable { "ecoqos:on" } else { "ecoqos:off" };
    let _ = db.insert_optimizer_audit("process", &process_target(name, pid), action, "completed", Some(detail), snapshot_id);
}

/// Enables EcoQoS for every currently eligible process under `mode`.
pub fn throttle_eligible(db: &Database, b: &Backends, mode: EcoQosMode, snapshot_id: i64) -> EcoQosOutcome {
    let processes = match b.processes.list_all() {
        Ok(p) => p,
        Err(e) => return EcoQosOutcome { summary: format!("process listing failed: {e}"), touched: Vec::new(), incomplete: 0 },
    };
    let mut touched = Vec::new();
    let mut skipped = 0usize;
    for proc in processes.iter().filter(|p| !p.eco_qos_applied) {
        // The listing classifies conservatively; re-classify for this profile's mode.
        let decision = classify_process_for_ecoqos_mode(&proc.name, proc.is_foreground || proc.in_foreground_tree, proc.cpu_percent_recent, mode);
        if decision != EcoQosDecision::Eligible {
            continue;
        }
        match b.processes.set_ecoqos(proc.pid, &proc.name, None, true, mode) {
            Ok(real) => {
                record_audit(db, &real, proc.pid, true, "via power profile", Some(snapshot_id));
                touched.push(EcoQosTouched { pid: proc.pid, name: real, since_ms: Some(now_ms()) });
            }
            Err(_) => skipped += 1,
        }
    }
    EcoQosOutcome { summary: format!("{} process(es) throttled, {skipped} skipped", touched.len()), touched, incomplete: 0 }
}

/// Processes the audit trail shows WinClean throttled and never restored.
/// Independent of the UI listing cap and of EcoQoS set by other apps.
pub fn owned_candidates(db: &Database) -> Result<Vec<EcoQosCandidate>, String> {
    let rows = db.ecoqos_owned_processes().map_err(|e| e.to_string())?;
    Ok(rows.into_iter().filter_map(|(target, at_ms)| parse_process_target(&target).map(|(name, pid)| (name, pid, at_ms))).collect())
}

/// Restores EcoQoS for candidates that still identify the process WinClean
/// throttled. Exited processes and reused PIDs are released from ownership
/// without touching anything.
pub fn restore(db: &Database, b: &Backends, candidates: Vec<EcoQosCandidate>, detail: &str, snapshot_id: Option<i64>) -> EcoQosOutcome {
    let (mut touched, mut released, mut skipped) = (Vec::new(), 0usize, 0usize);
    for (name, pid, since_ms) in candidates {
        match b.processes.liveness(pid) {
            ProcessLiveness::Gone => {
                record_audit(db, &name, pid, false, "process had already exited", None);
                released += 1;
            }
            ProcessLiveness::Alive { started_ms } if started_ms > since_ms => {
                record_audit(db, &name, pid, false, "PID now belongs to a newer process; not throttled by WinClean", None);
                released += 1;
            }
            ProcessLiveness::Unknown => skipped += 1,
            // Another app (or an earlier restore) already cleared it: nothing
            // to undo, and it must not be recorded as a change WinClean made.
            ProcessLiveness::Alive { .. } if b.processes.ecoqos_state(pid) == Some(false) => {
                record_audit(db, &name, pid, false, "EcoQoS was already off; left untouched", None);
                released += 1;
            }
            ProcessLiveness::Alive { .. } => match b.processes.set_ecoqos(pid, &name, Some(since_ms), false, EcoQosMode::Off) {
                Ok(real) => {
                    record_audit(db, &real, pid, false, detail, snapshot_id);
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
pub fn rethrottle(db: &Database, b: &Backends, candidates: Vec<EcoQosCandidate>, detail: &str, mode: EcoQosMode) -> EcoQosOutcome {
    let (mut touched, mut skipped) = (Vec::new(), 0usize);
    for (name, pid, since_ms) in candidates {
        match b.processes.liveness(pid) {
            ProcessLiveness::Alive { started_ms } if started_ms <= since_ms && b.processes.ecoqos_state(pid) == Some(true) => {}
            ProcessLiveness::Alive { started_ms } if started_ms <= since_ms => match b.processes.set_ecoqos(pid, &name, Some(since_ms), true, mode) {
                Ok(real) => {
                    record_audit(db, &real, pid, true, detail, None);
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
/// Returns `(summary, incomplete)` or `None` when the profile changed nothing.
pub fn rollback_profile(db: &Database, b: &Backends, payload: &serde_json::Value, snapshot_id: i64, snapshot_created_ms: i64) -> Option<(String, usize)> {
    // The payload list is written once after the process loop; the per-process
    // audit rows are written as each process changes. Use both so a failed or
    // interrupted payload write cannot make the changes un-undoable.
    let list = |key: &str, action: &str| -> Result<Vec<EcoQosCandidate>, String> {
        let mut out: Vec<EcoQosCandidate> = payload
            .get(key)
            .and_then(|v| serde_json::from_value::<Vec<EcoQosTouched>>(v.clone()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.name, t.pid, t.since_ms.unwrap_or(snapshot_created_ms)))
            .collect();
        for (target, at_ms) in db.ecoqos_changes_for_snapshot(snapshot_id, action).map_err(|e| e.to_string())? {
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
    if enabled.is_empty() && restored.is_empty() {
        return None;
    }
    // Only undo throttles WinClean still owns. A later manual Restore, or
    // EcoQoS another app set afterwards, must not be cleared by rolling back
    // an old profile. Unowned entries are skipped without holding the
    // rollback open; an unreadable ownership record does hold it open.
    let enabled = if enabled.is_empty() {
        enabled
    } else {
        match owned_candidates(db) {
            Ok(owned) => enabled.into_iter().filter(|(n, p, _)| owned.iter().any(|(on, op, _)| op == p && on.eq_ignore_ascii_case(n))).collect(),
            Err(e) => return Some((format!("could not read which throttles WinClean still owns ({e}); EcoQoS left unchanged"), enabled.len())),
        }
    };
    if enabled.is_empty() && restored.is_empty() {
        return Some(("no throttles from this profile are still owned by WinClean".to_string(), 0));
    }
    let (mut parts, mut incomplete) = (Vec::new(), 0usize);
    if !enabled.is_empty() {
        let outcome = restore(db, b, enabled, "rollback of power profile", None);
        incomplete += outcome.incomplete;
        parts.push(outcome.summary);
    }
    if !restored.is_empty() {
        parts.push(rethrottle(db, b, restored, "rollback of power profile", EcoQosMode::Conservative).summary);
    }
    Some((parts.join("; "), incomplete))
}

#[cfg(test)]
mod tests {
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
}
