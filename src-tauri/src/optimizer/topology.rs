//! CPU topology from `GetLogicalProcessorInformationEx(RelationAll)`.
//!
//! Windows exposes per-core `EfficiencyClass` and the logical processors
//! sharing each cache. WinClean derives clusters from both and reports what
//! the OS really says. The expected Ryzen AI 9 HX 370 layout (4× Zen 5 with a
//! 16 MB L3 + 8× Zen 5c with an 8 MB L3, SMT on) is only *named* when the
//! observed layout matches it — otherwise the clusters stay anonymous and a
//! note says why. WinClean never sets affinity or CPU sets: Windows'
//! scheduler and EcoQoS decide placement.

use serde::{Deserialize, Serialize};

/// One physical core as reported by `RelationProcessorCore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawCore {
    pub efficiency_class: u8,
    pub group: u16,
    /// Logical processors of this core within `group`.
    pub mask: u64,
}

/// One cache as reported by `RelationCache`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawCache {
    pub level: u8,
    pub size_bytes: u32,
    pub group: u16,
    pub mask: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClusterRole {
    Performance,
    Efficiency,
    /// Same efficiency class as the rest; separated only by cache domain.
    Uniform,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreCluster {
    pub role: ClusterRole,
    pub label: String,
    pub physical_cores: u32,
    pub logical_processors: u32,
    pub efficiency_class: u8,
    pub l3_kb: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L3Domain {
    pub size_kb: u32,
    pub physical_cores: u32,
    pub logical_processors: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuTopology {
    pub physical_cores: u32,
    pub logical_processors: u32,
    pub smt_enabled: bool,
    /// Windows reports more than one efficiency class.
    pub hybrid_by_efficiency_class: bool,
    pub clusters: Vec<CoreCluster>,
    pub l3_domains: Vec<L3Domain>,
    /// Set only when the observed layout matches the published HX 370 layout.
    pub zen_layout_confirmed: bool,
    pub notes: Vec<String>,
}

fn popcount(mask: u64) -> u32 {
    mask.count_ones()
}

fn core_in_cache(core: &RawCore, cache: &RawCache) -> bool {
    core.group == cache.group && core.mask & cache.mask != 0
}

fn is_hx370(cpu_name: &str) -> bool {
    cpu_name.to_ascii_lowercase().contains("ryzen ai 9 hx 370")
}

pub fn summarize_topology(cores: &[RawCore], caches: &[RawCache], cpu_name: &str) -> CpuTopology {
    let physical_cores = cores.len() as u32;
    let logical_processors: u32 = cores.iter().map(|c| popcount(c.mask)).sum();
    let smt_enabled = cores.iter().any(|c| popcount(c.mask) > 1);
    let mut notes = Vec::new();

    // L3 domains (distinct caches, deduplicated by group+mask).
    let mut l3: Vec<&RawCache> = Vec::new();
    for c in caches.iter().filter(|c| c.level == 3) {
        if !l3.iter().any(|x| x.group == c.group && x.mask == c.mask) {
            l3.push(c);
        }
    }
    let l3_domains: Vec<L3Domain> = l3
        .iter()
        .map(|c| {
            let members: Vec<&RawCore> = cores.iter().filter(|core| core_in_cache(core, c)).collect();
            L3Domain { size_kb: c.size_bytes / 1024, physical_cores: members.len() as u32, logical_processors: members.iter().map(|m| popcount(m.mask & c.mask)).sum() }
        })
        .collect();
    let l3_for = |members: &[&RawCore]| -> Option<u32> {
        let mut sizes: Vec<u32> = l3.iter().filter(|c| members.iter().any(|m| core_in_cache(m, c))).map(|c| c.size_bytes / 1024).collect();
        sizes.sort_unstable();
        sizes.dedup();
        (sizes.len() == 1).then(|| sizes[0])
    };

    let mut classes: Vec<u8> = cores.iter().map(|c| c.efficiency_class).collect();
    classes.sort_unstable();
    classes.dedup();
    let hybrid = classes.len() > 1;

    let mut clusters: Vec<CoreCluster> = Vec::new();
    if hybrid {
        for (i, class) in classes.iter().enumerate() {
            let members: Vec<&RawCore> = cores.iter().filter(|c| c.efficiency_class == *class).collect();
            let role = if i + 1 == classes.len() { ClusterRole::Performance } else { ClusterRole::Efficiency };
            clusters.push(CoreCluster {
                role,
                label: format!("{} cores (efficiency class {class})", if role == ClusterRole::Performance { "Performance" } else { "Efficiency" }),
                physical_cores: members.len() as u32,
                logical_processors: members.iter().map(|m| popcount(m.mask)).sum(),
                efficiency_class: *class,
                l3_kb: l3_for(&members),
            });
        }
        clusters.reverse(); // performance first
    } else if l3_domains.len() > 1 && l3_domains.iter().any(|d| d.size_kb != l3_domains[0].size_kb) {
        notes.push("Windows reports one efficiency class for every core; the clusters below are separated by their L3 cache domain.".to_string());
        let mut domains: Vec<(&&RawCache, Vec<&RawCore>)> = l3.iter().map(|c| (c, cores.iter().filter(|core| core_in_cache(core, c)).collect())).collect();
        domains.sort_by(|a, b| (b.0.size_bytes / b.1.len().max(1) as u32).cmp(&(a.0.size_bytes / a.1.len().max(1) as u32)));
        for (cache, members) in domains {
            clusters.push(CoreCluster {
                role: ClusterRole::Uniform,
                label: format!("{} MB L3 domain", cache.size_bytes / (1024 * 1024)),
                physical_cores: members.len() as u32,
                logical_processors: members.iter().map(|m| popcount(m.mask)).sum(),
                efficiency_class: members.first().map(|m| m.efficiency_class).unwrap_or(0),
                l3_kb: Some(cache.size_bytes / 1024),
            });
        }
    } else if !cores.is_empty() {
        notes.push("Windows reports a uniform set of cores (one efficiency class, one cache layout).".to_string());
        clusters.push(CoreCluster {
            role: ClusterRole::Uniform,
            label: "All cores".to_string(),
            physical_cores,
            logical_processors,
            efficiency_class: cores[0].efficiency_class,
            l3_kb: l3_for(&cores.iter().collect::<Vec<_>>()),
        });
    }

    // Name the Zen 5 / Zen 5c clusters only when the layout matches AMD's published one.
    let mut zen_layout_confirmed = false;
    if is_hx370(cpu_name) {
        let big = clusters.iter().position(|c| c.physical_cores == 4 && c.l3_kb == Some(16 * 1024));
        let compact = clusters.iter().position(|c| c.physical_cores == 8 && c.l3_kb == Some(8 * 1024));
        if clusters.len() == 2 && big.is_some() && compact.is_some() && smt_enabled {
            zen_layout_confirmed = true;
            clusters[big.unwrap()].label = "Zen 5 (4 cores, 16 MB L3)".to_string();
            clusters[compact.unwrap()].label = "Zen 5c (8 cores, 8 MB L3)".to_string();
            if clusters[big.unwrap()].role == ClusterRole::Uniform {
                notes.push("Zen 5 / Zen 5c are named because the observed 4 + 8 core split and 16 MB / 8 MB L3 domains match AMD's published Ryzen AI 9 HX 370 layout.".to_string());
            }
        } else {
            notes.push("The CPU model is a Ryzen AI 9 HX 370 but the observed layout does not match the published 4× Zen 5 + 8× Zen 5c split, so the clusters are not named.".to_string());
        }
    }
    notes.push("WinClean never sets CPU affinity or CPU sets. EcoQoS lets Windows steer background work to the most efficient cores on its own.".to_string());

    CpuTopology { physical_cores, logical_processors, smt_enabled, hybrid_by_efficiency_class: hybrid, clusters, l3_domains, zen_layout_confirmed, notes }
}

/// Reads the topology from Windows.
#[cfg(windows)]
pub fn read_topology(cpu_name: &str) -> Result<CpuTopology, String> {
    use windows_sys::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationAll, RelationCache, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};

    let mut needed = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RelationAll, std::ptr::null_mut(), &mut needed) };
    if needed == 0 {
        return Err(format!("GetLogicalProcessorInformationEx sizing failed: {}", std::io::Error::last_os_error()));
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    if unsafe { GetLogicalProcessorInformationEx(RelationAll, buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, &mut needed) } == 0 {
        return Err(format!("GetLogicalProcessorInformationEx failed: {}", std::io::Error::last_os_error()));
    }
    let base = buf.as_ptr() as *const u8;
    let (mut cores, mut caches) = (Vec::new(), Vec::new());
    let mut offset = 0usize;
    while offset + std::mem::size_of::<u32>() * 2 <= needed as usize {
        let info = unsafe { &*(base.add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
        if info.Size == 0 {
            break;
        }
        unsafe {
            if info.Relationship == RelationProcessorCore {
                let p = &info.Anonymous.Processor;
                let masks = std::slice::from_raw_parts(p.GroupMask.as_ptr(), p.GroupCount.max(1) as usize);
                for m in masks {
                    cores.push(RawCore { efficiency_class: p.EfficiencyClass, group: m.Group, mask: m.Mask as u64 });
                }
            } else if info.Relationship == RelationCache {
                let c = &info.Anonymous.Cache;
                // One mask per group; a cache normally lives in a single group.
                let first = &c.Anonymous.GroupMask as *const windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
                let masks = std::slice::from_raw_parts(first, c.GroupCount.max(1) as usize);
                for m in masks {
                    caches.push(RawCache { level: c.Level, size_bytes: c.CacheSize, group: m.Group, mask: m.Mask as u64 });
                }
            }
        }
        offset += info.Size as usize;
    }
    if cores.is_empty() {
        return Err("Windows reported no processor cores".into());
    }
    Ok(summarize_topology(&cores, &caches, cpu_name))
}

#[cfg(not(windows))]
pub fn read_topology(_cpu_name: &str) -> Result<CpuTopology, String> {
    Err("CPU topology is only available on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HX370: &str = "AMD Ryzen AI 9 HX 370 w/ Radeon 890M";

    /// 4 Zen 5 cores (SMT: 8 threads, 16 MB L3) + 8 Zen 5c cores (16 threads, 8 MB L3).
    fn strix_uniform_class() -> (Vec<RawCore>, Vec<RawCache>) {
        let mut cores = Vec::new();
        for i in 0..4 {
            cores.push(RawCore { efficiency_class: 0, group: 0, mask: 0b11 << (i * 2) });
        }
        for i in 0..8 {
            cores.push(RawCore { efficiency_class: 0, group: 0, mask: 0b11 << (8 + i * 2) });
        }
        let caches = vec![
            RawCache { level: 3, size_bytes: 16 * 1024 * 1024, group: 0, mask: 0xFF },
            RawCache { level: 3, size_bytes: 8 * 1024 * 1024, group: 0, mask: 0xFFFF00 },
            RawCache { level: 2, size_bytes: 1024 * 1024, group: 0, mask: 0b11 },
        ];
        (cores, caches)
    }

    #[test]
    fn hx370_layout_is_named_only_when_the_observed_layout_matches() {
        let (cores, caches) = strix_uniform_class();
        let t = summarize_topology(&cores, &caches, HX370);
        assert_eq!((t.physical_cores, t.logical_processors, t.smt_enabled), (12, 24, true));
        assert!(!t.hybrid_by_efficiency_class);
        assert!(t.zen_layout_confirmed);
        assert_eq!(t.clusters.len(), 2);
        assert_eq!(t.clusters[0].label, "Zen 5 (4 cores, 16 MB L3)");
        assert_eq!((t.clusters[0].physical_cores, t.clusters[0].logical_processors), (4, 8));
        assert_eq!(t.clusters[1].label, "Zen 5c (8 cores, 8 MB L3)");
        assert_eq!((t.clusters[1].physical_cores, t.clusters[1].logical_processors), (8, 16));
        assert!(t.notes.iter().any(|n| n.contains("published Ryzen AI 9 HX 370 layout")));
        assert_eq!(t.l3_domains.iter().map(|d| d.size_kb).collect::<Vec<_>>(), vec![16384, 8192]);
    }

    #[test]
    fn the_same_layout_on_another_cpu_is_not_named() {
        let (cores, caches) = strix_uniform_class();
        let t = summarize_topology(&cores, &caches, "AMD Ryzen 9 7940HS");
        assert!(!t.zen_layout_confirmed);
        assert!(t.clusters.iter().all(|c| c.label.contains("L3 domain")));
    }

    #[test]
    fn a_different_layout_on_an_hx370_is_called_out_not_labelled() {
        // Windows hides one cluster: 12 uniform cores, one L3.
        let cores: Vec<RawCore> = (0..12).map(|i| RawCore { efficiency_class: 0, group: 0, mask: 0b11 << (i * 2) }).collect();
        let caches = vec![RawCache { level: 3, size_bytes: 24 * 1024 * 1024, group: 0, mask: 0xFFFFFF }];
        let t = summarize_topology(&cores, &caches, HX370);
        assert!(!t.zen_layout_confirmed);
        assert!(t.notes.iter().any(|n| n.contains("does not match")));
        assert_eq!(t.clusters.len(), 1);
    }

    #[test]
    fn when_windows_reports_efficiency_classes_they_drive_the_clusters() {
        let mut cores = Vec::new();
        for i in 0..4 {
            cores.push(RawCore { efficiency_class: 1, group: 0, mask: 0b11 << (i * 2) });
        }
        for i in 0..8 {
            cores.push(RawCore { efficiency_class: 0, group: 0, mask: 0b11 << (8 + i * 2) });
        }
        let t = summarize_topology(&cores, &[], "Some Hybrid CPU");
        assert!(t.hybrid_by_efficiency_class);
        assert_eq!(t.clusters[0].role, ClusterRole::Performance);
        assert_eq!(t.clusters[0].physical_cores, 4);
        assert_eq!(t.clusters[1].role, ClusterRole::Efficiency);
        assert_eq!(t.clusters[1].physical_cores, 8);
    }

    #[test]
    fn a_plain_cpu_is_uniform_and_smt_is_detected_from_the_masks() {
        let cores: Vec<RawCore> = (0..4).map(|i| RawCore { efficiency_class: 0, group: 0, mask: 1 << i }).collect();
        let t = summarize_topology(&cores, &[RawCache { level: 3, size_bytes: 8 << 20, group: 0, mask: 0xF }], "Generic CPU");
        assert!(!t.smt_enabled);
        assert_eq!((t.physical_cores, t.logical_processors), (4, 4));
        assert_eq!(t.clusters.len(), 1);
        assert_eq!(t.clusters[0].l3_kb, Some(8192));
        assert!(t.notes.iter().any(|n| n.contains("uniform")));
    }

    #[test]
    fn empty_input_does_not_panic() {
        let t = summarize_topology(&[], &[], "x");
        assert_eq!((t.physical_cores, t.logical_processors), (0, 0));
        assert!(t.clusters.is_empty());
    }

    #[test]
    fn affinity_is_never_part_of_the_plan() {
        let (cores, caches) = strix_uniform_class();
        let t = summarize_topology(&cores, &caches, HX370);
        assert!(t.notes.iter().any(|n| n.contains("never sets CPU affinity")));
    }
}
