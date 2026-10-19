//! Deterministic validation of service start-type plans and the
//! recommendation engine that proposes them.
//!
//! Every path that can change a service — the Services view, the AI
//! assistant, a pasted plan — goes through [`validate_service_plan`]. The
//! elevated helper runs the same per-service checks again on its own fresh
//! view of the SCM, so a compromised caller cannot skip them.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::policy::{is_valid_service_name, knowledge, start_type_allowed, suggested_start_type, ServiceCategory, ServiceRisk};
use super::service_graph::active_dependents;
use super::services::{ServiceInfo, StartType};

/// Most services one plan may touch.
pub const MAX_PLAN_ITEMS: usize = 25;

/// A requested change as it arrives from the UI or the AI layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceChange {
    pub service_name: String,
    pub start_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub name: String,
    pub display_name: String,
    pub from: StartType,
    pub to: StartType,
    pub risk: ServiceRisk,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejection {
    pub service_name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicePlan {
    pub items: Vec<PlanItem>,
    pub rejected: Vec<Rejection>,
}

impl ServicePlan {
    /// Order-independent fingerprint of what the plan would change. The
    /// approval token binds to this, so the plan the user reviewed is the
    /// plan that runs.
    pub fn fingerprint(&self) -> String {
        let mut lines: Vec<String> = self.items.iter().map(|i| format!("{}:{}>{}", i.name.to_ascii_lowercase(), i.from.as_str(), i.to.as_str())).collect();
        lines.sort();
        let mut h = Sha256::new();
        for l in &lines {
            h.update(l.as_bytes());
            h.update(b"\n");
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Validates `changes` against the live service list.
///
/// Rejected (never applied): unknown names, malformed names, protected
/// services (including the dependency closure of protected services),
/// invalid or no-op start types, `Disabled` for services WinClean cannot
/// prove safe to disable, `Disabled` for a service that an enabled dependent
/// needs, duplicates, and anything beyond [`MAX_PLAN_ITEMS`].
pub fn validate_service_plan(changes: &[ServiceChange], services: &[ServiceInfo]) -> ServicePlan {
    let mut plan = ServicePlan::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut staged: Vec<PlanItem> = Vec::new();

    // Pass 1: everything that does not depend on the rest of the plan.
    for change in changes {
        let name = change.service_name.as_str();
        let rejection = |reason: String| Rejection { service_name: name.to_string(), reason };
        if staged.len() >= MAX_PLAN_ITEMS {
            plan.rejected.push(rejection(format!("a plan may change at most {MAX_PLAN_ITEMS} services")));
            continue;
        }
        if !is_valid_service_name(name) {
            plan.rejected.push(rejection("not a valid service name".into()));
            continue;
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            plan.rejected.push(rejection("listed more than once".into()));
            continue;
        }
        match check_one(change, services) {
            Ok(item) => staged.push(item),
            Err(reason) => plan.rejected.push(rejection(reason)),
        }
    }

    // Pass 2: a service may only be disabled when nothing that stays enabled
    // needs it. Disabling two services together is fine, but only services
    // that are themselves accepted count as "going away". Iterate to a
    // fixpoint because rejecting one disable can block another.
    let mut disabling: HashSet<String> = staged.iter().filter(|i| i.to == StartType::Disabled).map(|i| i.name.to_ascii_lowercase()).collect();
    loop {
        let blocked: Vec<String> = disabling.iter().filter(|n| !active_dependents(services, n, &disabling).is_empty()).cloned().collect();
        if blocked.is_empty() {
            break;
        }
        for b in blocked {
            disabling.remove(&b);
        }
    }
    for item in staged {
        if item.to == StartType::Disabled && !disabling.contains(&item.name.to_ascii_lowercase()) {
            let blocking = active_dependents(services, &item.name, &disabling);
            plan.rejected.push(Rejection { service_name: item.name.clone(), reason: format!("still needed by: {}", blocking.join(", ")) });
        } else {
            plan.items.push(item);
        }
    }
    plan
}

/// Per-service checks that need only the service itself.
fn check_one(change: &ServiceChange, services: &[ServiceInfo]) -> Result<PlanItem, String> {
    let name = change.service_name.as_str();
    let svc = services.iter().find(|s| s.name.eq_ignore_ascii_case(name)).ok_or_else(|| "unknown service".to_string())?;
    if svc.classification.risk == ServiceRisk::Protected || !svc.can_change {
        return Err(format!("protected: {}", svc.classification.reason));
    }
    let to = StartType::parse(&change.start_type).ok_or_else(|| format!("invalid start type '{}'", change.start_type))?;
    let from = svc
        .current_start_type()
        .ok_or_else(|| format!("current start type '{}' cannot be restored by rollback, so it will not be changed", svc.start_type))?;
    if from == to {
        return Err(format!("already {}", to.as_str()));
    }
    if !start_type_allowed(&svc.classification, to == StartType::Disabled) {
        return Err("WinClean cannot prove this service is safe to disable; Manual is the strongest change it will make".into());
    }
    let reason = knowledge(&svc.name, &svc.display_name).map(|k| k.why.to_string()).unwrap_or_else(|| svc.classification.reason.clone());
    Ok(PlanItem { name: svc.name.clone(), display_name: svc.display_name.clone(), from, to, risk: svc.classification.risk, reason })
}

// ---------------------------------------------------------------------------
// Recommendations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRecommendation {
    pub service_name: String,
    pub display_name: String,
    pub current_start_type: String,
    pub suggested_start_type: String,
    pub risk: ServiceRisk,
    pub category: ServiceCategory,
    pub rationale: String,
    /// "high" for curated Safe entries, "medium" for Caution ones.
    pub confidence: String,
    pub running: bool,
    /// Working set held right now; only for own-process services.
    pub memory_mb: Option<f64>,
    pub starts_at_boot: bool,
    pub requires_elevation: bool,
}

/// Proposes start-type reductions for services WinClean has curated
/// knowledge about. It only ever *lowers* startup cost (automatic → manual →
/// disabled), never touches protected services, and never invents numbers:
/// the only measurement shown is the service's real working set.
pub fn recommend_service_changes(services: &[ServiceInfo]) -> Vec<ServiceRecommendation> {
    let candidates: Vec<ServiceChange> = services
        .iter()
        .filter(|s| s.can_change && s.classification.risk != ServiceRisk::Protected)
        .filter_map(|s| {
            let suggested = StartType::parse(suggested_start_type(&s.name, &s.display_name)?)?;
            let current = s.current_start_type()?;
            (suggested.startup_cost() < current.startup_cost()).then(|| ServiceChange { service_name: s.name.clone(), start_type: suggested.as_str().to_string() })
        })
        .collect();
    // Reuse the full validation so a recommendation is never something the
    // apply path would reject.
    let plan = validate_service_plan(&candidates, services);
    let mut out: Vec<ServiceRecommendation> = plan
        .items
        .iter()
        .filter_map(|item| {
            let svc = services.iter().find(|s| s.name == item.name)?;
            Some(ServiceRecommendation {
                service_name: item.name.clone(),
                display_name: item.display_name.clone(),
                current_start_type: item.from.as_str().to_string(),
                suggested_start_type: item.to.as_str().to_string(),
                risk: item.risk,
                category: svc.classification.category,
                rationale: item.reason.clone(),
                confidence: if item.risk == ServiceRisk::Safe { "high".into() } else { "medium".into() },
                running: svc.state == "running",
                memory_mb: svc.memory_mb,
                starts_at_boot: matches!(item.from, StartType::Automatic | StartType::AutomaticDelayed),
                requires_elevation: true,
            })
        })
        .collect();
    // Biggest real benefit first: running services holding memory, then boot-time starters.
    out.sort_by(|a, b| {
        let key = |r: &ServiceRecommendation| (r.running, r.memory_mb.unwrap_or(0.0) as i64, r.starts_at_boot);
        key(b).cmp(&key(a)).then_with(|| a.display_name.cmp(&b.display_name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::service_graph::{finalize, testing::svc};

    fn fixture() -> Vec<ServiceInfo> {
        let mut v = vec![
            svc("WinDefend", "Microsoft Defender Antivirus Service", "automatic", "running", &["RpcSs"]),
            svc("RpcSs", "Remote Procedure Call (RPC)", "automatic", "running", &[]),
            svc("DiagTrack", "Connected User Experiences and Telemetry", "automatic", "running", &["RpcSs"]),
            svc("MapsBroker", "Downloaded Maps Manager", "automaticDelayed", "stopped", &[]),
            svc("Fax", "Fax", "manual", "stopped", &[]),
            svc("RemoteRegistry", "Remote Registry", "disabled", "stopped", &[]),
            svc("SysMain", "SysMain", "automatic", "running", &[]),
            svc("MysteryVendorSvc", "Mystery", "automatic", "running", &[]),
            svc("GoogleUpdaterService148", "Google Updater Service", "automatic", "stopped", &[]),
            svc("Needy", "Needs Fax", "manual", "stopped", &["Fax"]),
        ];
        v[8].classification = crate::optimizer::policy::classify_service("GoogleUpdaterService148", "Google Updater Service");
        finalize(&mut v);
        v
    }

    fn change(name: &str, to: &str) -> ServiceChange {
        ServiceChange { service_name: name.into(), start_type: to.into() }
    }

    #[test]
    fn protected_unknown_and_malformed_requests_are_rejected_with_reasons() {
        let services = fixture();
        let plan = validate_service_plan(
            &[
                change("WinDefend", "disabled"),
                change("RpcSs", "manual"),
                change("MysteryVendorSvc", "disabled"),
                change("Nope", "manual"),
                change("bad\\name", "manual"),
                change("DiagTrack", "sideways"),
                change("Fax", "manual"),
            ],
            &services,
        );
        assert!(plan.items.is_empty(), "{:?}", plan.items);
        let reason = |n: &str| plan.rejected.iter().find(|r| r.service_name == n).unwrap().reason.clone();
        assert!(reason("WinDefend").starts_with("protected"));
        assert!(reason("RpcSs").starts_with("protected"));
        assert!(reason("MysteryVendorSvc").contains("cannot prove"));
        assert_eq!(reason("Nope"), "unknown service");
        assert!(reason("bad\\name").contains("not a valid"));
        assert!(reason("DiagTrack").contains("invalid start type"));
        assert_eq!(reason("Fax"), "already manual");
    }

    #[test]
    fn valid_changes_pass_and_unknown_services_may_move_to_manual() {
        let services = fixture();
        let plan = validate_service_plan(&[change("DiagTrack", "manual"), change("MysteryVendorSvc", "manual")], &services);
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].from, StartType::Automatic);
        assert_eq!(plan.items[0].to, StartType::Manual);
        assert!(plan.rejected.is_empty());
    }

    #[test]
    fn disabling_a_service_that_an_enabled_dependent_needs_is_rejected_unless_both_go() {
        let services = fixture();
        let alone = validate_service_plan(&[change("Fax", "disabled")], &services);
        // Fax is manual and "Needy" (manual, enabled) depends on it.
        assert!(alone.items.is_empty());
        assert!(alone.rejected[0].reason.contains("still needed by: Needy"));
        let both = validate_service_plan(&[change("Needy", "disabled"), change("Fax", "disabled")], &services);
        // Needy is unknown (cannot be disabled); Fax is then still blocked by it.
        assert!(both.items.iter().all(|i| i.name != "Fax"));
    }

    #[test]
    fn two_disable_able_services_can_be_disabled_together_but_not_one_alone() {
        let mut services = vec![
            svc("UpdaterBase", "Base updater", "automatic", "stopped", &[]),
            svc("UpdaterTop", "Top updater", "automatic", "stopped", &["UpdaterBase"]),
        ];
        for s in services.iter_mut() {
            s.classification = crate::optimizer::policy::classify_service("GoogleUpdaterService1", "Google Updater");
        }
        finalize(&mut services);
        let alone = validate_service_plan(&[change("UpdaterBase", "disabled")], &services);
        assert!(alone.items.is_empty());
        assert!(alone.rejected[0].reason.contains("still needed by: UpdaterTop"));
        let together = validate_service_plan(&[change("UpdaterBase", "disabled"), change("UpdaterTop", "disabled")], &services);
        assert_eq!(together.items.len(), 2, "{:?}", together.rejected);
    }

    #[test]
    fn duplicates_and_oversized_plans_are_rejected() {
        let services = fixture();
        let plan = validate_service_plan(&[change("DiagTrack", "manual"), change("diagtrack", "disabled")], &services);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.rejected[0].reason, "listed more than once");

        let many: Vec<ServiceChange> = (0..MAX_PLAN_ITEMS + 3).map(|i| change(&format!("Svc{i}"), "manual")).collect();
        let big = validate_service_plan(&many, &services);
        // Unknown names are rejected as unknown; the cap message applies once items fill up,
        // so verify the cap with a list of valid services instead.
        assert!(big.items.is_empty());
        let mut crowded = services.clone();
        for i in 0..MAX_PLAN_ITEMS + 3 {
            crowded.push(svc(&format!("Updater{i}"), "Updater", "automatic", "stopped", &[]));
        }
        let changes: Vec<ServiceChange> = (0..MAX_PLAN_ITEMS + 3).map(|i| change(&format!("Updater{i}"), "manual")).collect();
        let capped = validate_service_plan(&changes, &crowded);
        assert_eq!(capped.items.len(), MAX_PLAN_ITEMS);
        assert_eq!(capped.rejected.len(), 3);
    }

    #[test]
    fn fingerprint_is_stable_under_reordering_and_changes_with_content() {
        let services = fixture();
        let a = validate_service_plan(&[change("DiagTrack", "manual"), change("MapsBroker", "manual")], &services);
        let b = validate_service_plan(&[change("MapsBroker", "manual"), change("DiagTrack", "manual")], &services);
        let c = validate_service_plan(&[change("DiagTrack", "disabled"), change("MapsBroker", "manual")], &services);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn recommendations_only_lower_startup_cost_and_skip_protected_and_unknown() {
        let services = fixture();
        let recs = recommend_service_changes(&services);
        let names: Vec<&str> = recs.iter().map(|r| r.service_name.as_str()).collect();
        assert!(names.contains(&"DiagTrack"));
        assert!(names.contains(&"MapsBroker"));
        assert!(names.contains(&"GoogleUpdaterService148"));
        // Already manual / already disabled / no suggestion / protected / unknown: never listed.
        for skipped in ["Fax", "RemoteRegistry", "SysMain", "WinDefend", "RpcSs", "MysteryVendorSvc"] {
            assert!(!names.contains(&skipped), "{skipped} must not be recommended");
        }
        let diag = recs.iter().find(|r| r.service_name == "DiagTrack").unwrap();
        assert_eq!((diag.current_start_type.as_str(), diag.suggested_start_type.as_str()), ("automatic", "manual"));
        assert!(diag.starts_at_boot && diag.running && diag.requires_elevation);
        // Every recommendation survives the same validation the apply path uses.
        let changes: Vec<ServiceChange> = recs.iter().map(|r| change(&r.service_name, &r.suggested_start_type)).collect();
        let plan = validate_service_plan(&changes, &services);
        assert_eq!(plan.items.len(), recs.len());
        assert!(plan.rejected.is_empty());
    }

    #[test]
    fn recommendations_report_real_memory_only() {
        let mut services = fixture();
        services.iter_mut().find(|s| s.name == "DiagTrack").unwrap().memory_mb = Some(48.5);
        let recs = recommend_service_changes(&services);
        assert_eq!(recs[0].service_name, "DiagTrack", "running + largest working set first");
        assert_eq!(recs[0].memory_mb, Some(48.5));
        assert_eq!(recs.iter().find(|r| r.service_name == "MapsBroker").unwrap().memory_mb, None);
    }
}
