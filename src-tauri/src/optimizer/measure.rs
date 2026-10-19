//! Measuring what an optimization actually did to battery power draw.
//!
//! WinClean never claims a saving from theory. A measurement session records
//! real `BATTERY_STATUS.Rate` samples before and after a profile is applied
//! and compares them honestly:
//!
//! * only discharge samples count (AC time says nothing about draw);
//! * the first minute after the switch is discarded as settling time;
//! * samples are averaged per minute before the test, because consecutive
//!   power readings are strongly correlated and a naive per-sample test would
//!   claim far more certainty than the data supports;
//! * the verdict is `improved`/`worse` only when the difference exceeds its
//!   own 95% confidence interval, otherwise `inconclusive`;
//! * windows that ran different workloads are flagged, not silently compared.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::battery::{discharge_stats, discharge_watts_from_rate, TelemetrySample, WattStats};

/// Seconds ignored right after the switch.
pub const SETTLE_SECS: i64 = 60;
/// Minimum per-minute buckets in each window.
pub const MIN_BUCKETS: usize = 3;
/// Minimum raw discharge samples in each window.
pub const MIN_SAMPLES: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Improved,
    Worse,
    Inconclusive,
    InsufficientData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementResult {
    pub baseline: Option<WattStats>,
    pub after: Option<WattStats>,
    /// `after - baseline`; negative means the profile saved power.
    pub delta_w: Option<f64>,
    pub delta_percent: Option<f64>,
    /// Half-width of the 95% confidence interval of `delta_w`.
    pub ci95_w: Option<f64>,
    pub verdict: Verdict,
    pub notes: Vec<String>,
}

/// Student t critical values (two-sided 95%) for small degrees of freedom.
fn t_crit(df: f64) -> f64 {
    match df {
        d if d < 1.5 => 12.71,
        d if d < 2.5 => 4.30,
        d if d < 3.5 => 3.18,
        d if d < 4.5 => 2.78,
        d if d < 5.5 => 2.57,
        d if d < 7.5 => 2.36,
        d if d < 10.5 => 2.23,
        d if d < 20.5 => 2.09,
        _ => 1.96,
    }
}

/// Mean discharge watts per whole minute: autocorrelation-robust units for the test.
fn minute_means(samples: &[TelemetrySample]) -> Vec<f64> {
    let mut buckets: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for s in samples.iter().filter(|s| !s.on_ac) {
        if let Some(w) = discharge_watts_from_rate(s.rate_mw) {
            buckets.entry(s.at_ms.div_euclid(60_000)).or_default().push(w);
        }
    }
    buckets.values().map(|v| v.iter().sum::<f64>() / v.len() as f64).collect()
}

fn mean_var(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = if v.len() > 1 { v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0) } else { 0.0 };
    (mean, var)
}

fn dominant_workload(samples: &[TelemetrySample]) -> Option<(String, f64)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut total = 0usize;
    for s in samples.iter().filter(|s| !s.on_ac) {
        if let Some(w) = &s.workload {
            *counts.entry(w.as_str()).or_default() += 1;
            total += 1;
        }
    }
    let (name, n) = counts.into_iter().max_by_key(|(_, n)| *n)?;
    Some((name.to_string(), n as f64 / total as f64))
}

/// Compares the baseline window `[baseline_start, switch)` with the window
/// `[switch + settle, end]`.
pub fn compare_windows(samples: &[TelemetrySample], baseline_start_ms: i64, switch_ms: i64, end_ms: i64) -> MeasurementResult {
    let baseline: Vec<TelemetrySample> = samples.iter().filter(|s| s.at_ms >= baseline_start_ms && s.at_ms < switch_ms).cloned().collect();
    let after: Vec<TelemetrySample> = samples.iter().filter(|s| s.at_ms >= switch_ms + SETTLE_SECS * 1000 && s.at_ms <= end_ms).cloned().collect();

    let mut notes = Vec::new();
    let (b_stats, a_stats) = (discharge_stats(&baseline), discharge_stats(&after));
    let (b_min, a_min) = (minute_means(&baseline), minute_means(&after));

    let enough = |stats: &Option<WattStats>, mins: &[f64]| stats.map(|s| s.samples >= MIN_SAMPLES).unwrap_or(false) && mins.len() >= MIN_BUCKETS;
    if !enough(&b_stats, &b_min) || !enough(&a_stats, &a_min) {
        notes.push(format!(
            "Needs at least {MIN_SAMPLES} discharge samples over {MIN_BUCKETS} minutes in both windows (baseline: {}, after: {}). Keep the laptop on battery and measure for longer.",
            b_stats.map(|s| s.samples).unwrap_or(0),
            a_stats.map(|s| s.samples).unwrap_or(0)
        ));
        return MeasurementResult { baseline: b_stats, after: a_stats, delta_w: None, delta_percent: None, ci95_w: None, verdict: Verdict::InsufficientData, notes };
    }

    if let (Some((bw, bshare)), Some((aw, ashare))) = (dominant_workload(&baseline), dominant_workload(&after)) {
        if bw != aw {
            notes.push(format!("The workload changed between windows ({bw} → {aw}); the comparison is not like-for-like."));
        } else if bshare < 0.7 || ashare < 0.7 {
            notes.push("The workload was mixed in at least one window; treat the result with caution.".to_string());
        }
    } else {
        notes.push("Workload labels were not recorded for both windows; make sure you did the same kind of work in both.".to_string());
    }

    let (bm, bv) = mean_var(&b_min);
    let (am, av) = mean_var(&a_min);
    let delta = am - bm;
    let se = (bv / b_min.len() as f64 + av / a_min.len() as f64).sqrt();
    // Welch–Satterthwaite degrees of freedom.
    let df = if se > 0.0 {
        let num = (bv / b_min.len() as f64 + av / a_min.len() as f64).powi(2);
        let den = (bv / b_min.len() as f64).powi(2) / (b_min.len() as f64 - 1.0) + (av / a_min.len() as f64).powi(2) / (a_min.len() as f64 - 1.0);
        if den > 0.0 { num / den } else { 1.0 }
    } else {
        f64::INFINITY
    };
    let ci = t_crit(df) * se;
    let verdict = if delta.abs() <= ci {
        Verdict::Inconclusive
    } else if delta < 0.0 {
        Verdict::Improved
    } else {
        Verdict::Worse
    };
    if verdict == Verdict::Inconclusive {
        notes.push("The difference is within the measurement noise. Measure longer or with a steadier workload.".to_string());
    }
    let round = |v: f64| (v * 100.0).round() / 100.0;
    MeasurementResult {
        baseline: b_stats,
        after: a_stats,
        delta_w: Some(round(delta)),
        delta_percent: (bm > 0.0).then(|| round(delta / bm * 100.0)),
        ci95_w: Some(round(ci)),
        verdict,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sample every 10 s with a deterministic wobble around `watts`.
    fn window(start_ms: i64, minutes: i64, watts: f64, workload: &str) -> Vec<TelemetrySample> {
        (0..minutes * 6)
            .map(|i| {
                let wobble = ((i * 7919) % 11) as f64 / 10.0 - 0.5; // ±0.5 W
                TelemetrySample {
                    at_ms: start_ms + i * 10_000,
                    percent: Some(70),
                    on_ac: false,
                    rate_mw: Some(-(((watts + wobble) * 1000.0) as i32)),
                    voltage_mv: None,
                    capacity_mwh: None,
                    profile_id: None,
                    workload: Some(workload.to_string()),
                }
            })
            .collect()
    }

    fn session(base_w: f64, after_w: f64, base_workload: &str, after_workload: &str) -> MeasurementResult {
        let t0 = 1_000_000_000;
        let switch = t0 + 10 * 60_000;
        let mut v = window(t0, 10, base_w, base_workload);
        v.extend(window(switch, 10, after_w, after_workload));
        compare_windows(&v, t0, switch, switch + 10 * 60_000)
    }

    #[test]
    fn a_real_saving_is_reported_with_its_size_and_interval() {
        let r = session(14.0, 11.0, "coding", "coding");
        assert_eq!(r.verdict, Verdict::Improved);
        assert!((r.delta_w.unwrap() + 3.0).abs() < 0.3, "{:?}", r.delta_w);
        assert!(r.delta_percent.unwrap() < -15.0);
        assert!(r.ci95_w.unwrap() < 1.0);
        assert!(r.notes.is_empty(), "{:?}", r.notes);
    }

    #[test]
    fn a_regression_is_reported_as_worse() {
        assert_eq!(session(10.0, 13.0, "coding", "coding").verdict, Verdict::Worse);
    }

    #[test]
    fn identical_draw_is_inconclusive_not_improved() {
        let r = session(12.0, 12.0, "coding", "coding");
        assert_eq!(r.verdict, Verdict::Inconclusive);
        assert!(r.notes.iter().any(|n| n.contains("noise")));
    }

    #[test]
    fn too_little_data_is_never_turned_into_a_verdict() {
        let t0 = 5_000_000;
        let v = window(t0, 1, 12.0, "coding");
        let r = compare_windows(&v, t0, t0 + 60_000, t0 + 120_000);
        assert_eq!(r.verdict, Verdict::InsufficientData);
        assert_eq!(r.delta_w, None);
        assert!(r.notes[0].contains("Needs at least"));
    }

    #[test]
    fn ac_samples_are_ignored() {
        let t0 = 1_000_000_000;
        let switch = t0 + 10 * 60_000;
        let mut v = window(t0, 10, 14.0, "coding");
        for s in window(switch, 10, 11.0, "coding") {
            v.push(TelemetrySample { on_ac: true, ..s });
        }
        let r = compare_windows(&v, t0, switch, switch + 10 * 60_000);
        assert_eq!(r.verdict, Verdict::InsufficientData, "a window made only of AC samples cannot be measured");
    }

    #[test]
    fn the_settling_period_after_the_switch_is_discarded() {
        let t0 = 1_000_000_000;
        let switch = t0 + 10 * 60_000;
        let mut v = window(t0, 10, 12.0, "coding");
        // A 30 s transient spike right after the switch, then steady 12 W.
        v.extend((0..3).map(|i| TelemetrySample { at_ms: switch + i * 10_000, percent: Some(70), on_ac: false, rate_mw: Some(-40_000), voltage_mv: None, capacity_mwh: None, profile_id: None, workload: Some("coding".into()) }));
        v.extend(window(switch + 60_000, 9, 12.0, "coding"));
        let r = compare_windows(&v, t0, switch, switch + 10 * 60_000);
        assert_eq!(r.verdict, Verdict::Inconclusive);
        assert!(r.delta_w.unwrap().abs() < 0.3);
    }

    #[test]
    fn different_workloads_are_called_out() {
        let r = session(10.0, 8.0, "coding", "gaming");
        assert!(r.notes.iter().any(|n| n.contains("not like-for-like")), "{:?}", r.notes);
    }

    #[test]
    fn t_critical_values_shrink_with_more_data() {
        assert!(t_crit(2.0) > t_crit(10.0));
        assert!(t_crit(10.0) > t_crit(100.0));
    }
}
