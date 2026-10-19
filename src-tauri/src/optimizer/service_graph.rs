//! Service dependency graph: reverse dependencies, protection closure, and
//! the "would this break something that is running" checks. Pure data in,
//! pure data out — the same code runs in the unprivileged app and in the
//! elevated helper.

use std::collections::{HashMap, HashSet, VecDeque};

use super::policy::{ServiceCategory, ServiceRisk};
use super::services::{ServiceInfo, StartType};

fn key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Fills `dependents`, applies the protection closure and recomputes
/// `can_change`.
///
/// Protection closure: a service that a protected service (transitively)
/// depends on is itself protected. Disabling `RpcSs`'s dependency would break
/// Defender even if the dependency has no protected name, so the closure is
/// the single rule that stops such a change.
pub fn finalize(services: &mut [ServiceInfo]) {
    let index: HashMap<String, usize> = services.iter().enumerate().map(|(i, s)| (key(&s.name), i)).collect();

    for s in services.iter_mut() {
        s.dependents.clear();
    }
    let mut reverse: Vec<Vec<String>> = vec![Vec::new(); services.len()];
    for s in services.iter() {
        for dep in &s.dependencies {
            if let Some(&j) = index.get(&key(dep)) {
                reverse[j].push(s.name.clone());
            }
        }
    }
    for (s, dependents) in services.iter_mut().zip(reverse) {
        s.dependents = dependents;
    }

    let mut queue: VecDeque<usize> = services
        .iter()
        .enumerate()
        .filter(|(_, s)| s.classification.risk == ServiceRisk::Protected)
        .map(|(i, _)| i)
        .collect();
    let mut seen: HashSet<usize> = queue.iter().copied().collect();
    while let Some(i) = queue.pop_front() {
        let deps: Vec<String> = services[i].dependencies.clone();
        let parent = services[i].display_name.clone();
        for dep in deps {
            let Some(&j) = index.get(&key(&dep)) else { continue };
            if !seen.insert(j) {
                continue;
            }
            let c = &mut services[j].classification;
            c.risk = ServiceRisk::Protected;
            c.can_disable = false;
            if c.category == ServiceCategory::Unknown {
                c.category = ServiceCategory::WindowsCore;
            }
            c.known = true;
            c.reason = format!("Required by protected service '{parent}'.");
            c.recommendation = "Leave unchanged. WinClean will never modify this service.".to_string();
            queue.push_back(j);
        }
    }
    for s in services.iter_mut() {
        s.can_change = s.classification.risk != ServiceRisk::Protected;
    }
}

/// Dependents that are not disabled — the ones that would break if `name`
/// were disabled. Services in `also_disabled` (being disabled in the same
/// plan) do not count.
pub fn active_dependents(services: &[ServiceInfo], name: &str, also_disabled: &HashSet<String>) -> Vec<String> {
    let n = key(name);
    services
        .iter()
        .find(|s| key(&s.name) == n)
        .map(|s| {
            s.dependents
                .iter()
                .filter(|d| !also_disabled.contains(&key(d)))
                .filter(|d| {
                    services
                        .iter()
                        .find(|x| key(&x.name) == key(d))
                        .map(|x| x.current_start_type() != Some(StartType::Disabled))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::optimizer::policy::classify_service;

    /// Builds a minimal service row for tests.
    pub fn svc(name: &str, display: &str, start: &str, state: &str, deps: &[&str]) -> ServiceInfo {
        let classification = classify_service(name, display);
        ServiceInfo {
            name: name.to_string(),
            display_name: display.to_string(),
            state: state.to_string(),
            start_type: start.to_string(),
            delayed_auto_start: start == "automaticDelayed",
            pid: None,
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            dependents: Vec::new(),
            can_change: classification.risk != ServiceRisk::Protected,
            classification,
            own_process: true,
            trigger_start: false,
            memory_mb: None,
            binary_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::svc;
    use super::*;

    #[test]
    fn dependents_are_the_reverse_of_dependencies() {
        let mut v = vec![
            svc("Base", "Base", "manual", "stopped", &[]),
            svc("Mid", "Mid", "manual", "stopped", &["Base"]),
            svc("Top", "Top", "manual", "stopped", &["mid", "Missing"]),
        ];
        finalize(&mut v);
        assert_eq!(v[0].dependents, vec!["Mid"]);
        assert_eq!(v[1].dependents, vec!["Top"]);
        assert!(v[2].dependents.is_empty());
    }

    #[test]
    fn dependencies_of_protected_services_are_protected_transitively() {
        let mut v = vec![
            svc("WinDefend", "Microsoft Defender Antivirus Service", "automatic", "running", &["WdHelper"]),
            svc("WdHelper", "Helper", "manual", "running", &["DeepDep"]),
            svc("DeepDep", "Deep dependency", "manual", "stopped", &[]),
            svc("Unrelated", "Unrelated", "manual", "stopped", &[]),
        ];
        finalize(&mut v);
        let by = |n: &str| v.iter().find(|s| s.name == n).unwrap();
        for n in ["WinDefend", "WdHelper", "DeepDep"] {
            assert_eq!(by(n).classification.risk, ServiceRisk::Protected, "{n}");
            assert!(!by(n).can_change, "{n}");
        }
        assert!(by("Unrelated").can_change);
        assert!(by("WdHelper").classification.reason.contains("Required by protected service"));
    }

    #[test]
    fn protection_closure_terminates_on_dependency_cycles() {
        let mut v = vec![
            svc("WinDefend", "Windows Defender", "automatic", "running", &["A"]),
            svc("A", "A", "manual", "stopped", &["B"]),
            svc("B", "B", "manual", "stopped", &["A", "WinDefend"]),
        ];
        finalize(&mut v);
        assert!(v.iter().all(|s| !s.can_change));
    }

    #[test]
    fn only_non_disabled_dependents_block_a_disable() {
        let mut v = vec![
            svc("Base", "Base", "manual", "stopped", &[]),
            svc("OnDemand", "OnDemand", "manual", "stopped", &["Base"]),
            svc("Off", "Off", "disabled", "stopped", &["Base"]),
        ];
        finalize(&mut v);
        let none = HashSet::new();
        assert_eq!(active_dependents(&v, "Base", &none), vec!["OnDemand"]);
        let mut also = HashSet::new();
        also.insert("ondemand".to_string());
        assert!(active_dependents(&v, "Base", &also).is_empty());
    }
}
