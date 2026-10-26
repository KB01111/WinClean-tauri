//! Automatic profile switching: a pure decision function.
//!
//! The runtime (event-driven: power-setting callbacks + foreground changes)
//! gathers the inputs and calls [`decide`]; this module never touches the OS.
//! Defaults are conservative: auto-switching is **off**, a manual choice is
//! respected for a while, flapping is prevented with a dwell time and a
//! hysteresis band, and nothing happens while G-Helper manages power.

use serde::{Deserialize, Serialize};

use super::workload::WorkloadKind;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoSwitchConfig {
    pub enabled: bool,
    pub on_ac_profile: String,
    pub on_battery_profile: String,
    pub low_battery_percent: u8,
    pub low_battery_profile: String,
    /// Profile to use while gaming / AI development / coding. Empty = no rule.
    pub gaming_profile: String,
    pub ai_development_profile: String,
    pub coding_profile: String,
    /// Pause automatic switching while G-Helper is running so the two tools
    /// never fight over the Windows power plan.
    pub respect_ghelper: bool,
    pub min_dwell_secs: u32,
    pub hysteresis_percent: u8,
    /// After a manual profile choice, automatic switching stays quiet this long.
    pub manual_override_minutes: u32,
}

impl Default for AutoSwitchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            on_ac_profile: "balanced".into(),
            on_battery_profile: "balanced".into(),
            low_battery_percent: 20,
            low_battery_profile: "batterySaver".into(),
            gaming_profile: "gaming".into(),
            ai_development_profile: "aiDevelopment".into(),
            coding_profile: "coding".into(),
            respect_ghelper: true,
            min_dwell_secs: 90,
            hysteresis_percent: 3,
            manual_override_minutes: 30,
        }
    }
}

impl AutoSwitchConfig {
    /// Clamps values into sane ranges; rejects absurd input rather than guessing.
    pub fn validated(mut self) -> Result<Self, String> {
        if !(5..=50).contains(&self.low_battery_percent) {
            return Err("low battery threshold must be between 5% and 50%".into());
        }
        if self.hysteresis_percent > 10 {
            return Err("hysteresis must be at most 10%".into());
        }
        self.min_dwell_secs = self.min_dwell_secs.clamp(15, 3600);
        self.manual_override_minutes = self.manual_override_minutes.clamp(0, 720);
        for p in [&self.on_ac_profile, &self.on_battery_profile, &self.low_battery_profile] {
            if p.trim().is_empty() {
                return Err("AC, battery and low-battery profiles must all be set".into());
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct AutoSwitchInput {
    pub on_ac: bool,
    /// False when Windows reports an unknown power source (never drive rules).
    pub ac_known: bool,
    pub percent: Option<u8>,
    pub workload: WorkloadKind,
    pub ghelper_running: bool,
    pub now_ms: i64,
    pub last_switch_ms: Option<i64>,
    /// The profile WinClean currently has applied, if any.
    pub current_profile: Option<String>,
    /// When the user last chose a profile by hand.
    pub last_manual_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoDecision {
    Stay(String),
    Switch { profile_id: String, reason: String },
}

pub fn decide(config: &AutoSwitchConfig, input: &AutoSwitchInput) -> AutoDecision {
    let stay = |why: &str| AutoDecision::Stay(why.to_string());
    if !config.enabled {
        return stay("Automatic switching is off.");
    }
    if config.respect_ghelper && input.ghelper_running {
        return stay("G-Helper is running; automatic switching is paused so the two tools do not fight over the power plan.");
    }
    if !input.ac_known {
        return stay("Windows did not report the power source.");
    }
    if let Some(manual) = input.last_manual_ms {
        let window = config.manual_override_minutes as i64 * 60_000;
        if window > 0 && input.now_ms.saturating_sub(manual) < window {
            return stay("You chose a profile recently; automatic switching waits.");
        }
    }
    if let Some(last) = input.last_switch_ms {
        if input.now_ms.saturating_sub(last) < config.min_dwell_secs as i64 * 1000 {
            return stay("Switched very recently; waiting to avoid flapping.");
        }
    }

    let low = !input.on_ac && input.percent.map(|p| p <= config.low_battery_percent).unwrap_or(false);
    // Hysteresis: once in the low-battery profile, stay until clearly above the threshold.
    let in_low_band = !input.on_ac
        && input.current_profile.as_deref() == Some(config.low_battery_profile.as_str())
        && input.percent.map(|p| p <= config.low_battery_percent.saturating_add(config.hysteresis_percent)).unwrap_or(false);

    let workload_profile = match input.workload {
        WorkloadKind::Gaming => Some((&config.gaming_profile, "gaming")),
        WorkloadKind::AiDevelopment | WorkloadKind::AiInference => Some((&config.ai_development_profile, "AI development")),
        WorkloadKind::Coding => Some((&config.coding_profile, "coding")),
        WorkloadKind::General => None,
    }
    .filter(|(p, _)| !p.trim().is_empty());

    let (target, reason) = if low || in_low_band {
        (config.low_battery_profile.clone(), format!("Battery is at or below {}%.", config.low_battery_percent))
    } else if let Some((profile, label)) = workload_profile {
        (profile.clone(), format!("Detected {label} workload."))
    } else if input.on_ac {
        (config.on_ac_profile.clone(), "Plugged in.".to_string())
    } else {
        (config.on_battery_profile.clone(), "Running on battery.".to_string())
    };

    if input.current_profile.as_deref() == Some(target.as_str()) {
        return AutoDecision::Stay(format!("Already using {target}."));
    }
    AutoDecision::Switch { profile_id: target, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AutoSwitchConfig {
        AutoSwitchConfig { enabled: true, ..Default::default() }
    }
    fn input() -> AutoSwitchInput {
        AutoSwitchInput { on_ac: true, ac_known: true, percent: Some(80), workload: WorkloadKind::General, ghelper_running: false, now_ms: 10_000_000, last_switch_ms: None, current_profile: None, last_manual_ms: None }
    }
    fn switch(d: AutoDecision) -> String {
        match d {
            AutoDecision::Switch { profile_id, .. } => profile_id,
            AutoDecision::Stay(w) => panic!("expected switch, stayed: {w}"),
        }
    }

    #[test]
    fn off_by_default() {
        assert!(matches!(decide(&AutoSwitchConfig::default(), &input()), AutoDecision::Stay(_)));
    }

    #[test]
    fn ac_and_battery_use_their_profiles() {
        assert_eq!(switch(decide(&cfg(), &input())), "balanced");
        let mut c = cfg();
        c.on_ac_profile = "performance-ish".into();
        assert_eq!(switch(decide(&c, &input())), "performance-ish");
        let mut i = input();
        i.on_ac = false;
        c.on_battery_profile = "batterySaver".into();
        assert_eq!(switch(decide(&c, &i)), "batterySaver");
    }

    #[test]
    fn low_battery_beats_workload_rules() {
        let mut i = input();
        i.on_ac = false;
        i.percent = Some(15);
        i.workload = WorkloadKind::Gaming;
        assert_eq!(switch(decide(&cfg(), &i)), "batterySaver");
    }

    #[test]
    fn workload_profiles_apply_when_not_low() {
        let mut i = input();
        i.workload = WorkloadKind::Gaming;
        assert_eq!(switch(decide(&cfg(), &i)), "gaming");
        i.workload = WorkloadKind::AiDevelopment;
        assert_eq!(switch(decide(&cfg(), &i)), "aiDevelopment");
        i.workload = WorkloadKind::Coding;
        i.on_ac = false;
        i.percent = Some(70);
        assert_eq!(switch(decide(&cfg(), &i)), "coding");
        let mut c = cfg();
        c.coding_profile = String::new();
        assert_eq!(switch(decide(&c, &i)), "balanced", "an empty rule means no rule");
    }

    #[test]
    fn hysteresis_keeps_the_saver_until_clearly_above_the_threshold() {
        let mut i = input();
        i.on_ac = false;
        i.current_profile = Some("batterySaver".into());
        i.percent = Some(22); // threshold 20 + hysteresis 3 = 23
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Stay(_)));
        i.percent = Some(24);
        assert_eq!(switch(decide(&cfg(), &i)), "balanced");
        // Plugging in leaves the saver immediately.
        i.on_ac = true;
        i.percent = Some(10);
        assert_eq!(switch(decide(&cfg(), &i)), "balanced");
    }

    #[test]
    fn dwell_time_prevents_flapping() {
        let mut i = input();
        i.last_switch_ms = Some(i.now_ms - 30_000);
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Stay(_)));
        i.last_switch_ms = Some(i.now_ms - 120_000);
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Switch { .. }));
    }

    #[test]
    fn a_manual_choice_is_respected_for_the_override_window() {
        let mut i = input();
        i.last_manual_ms = Some(i.now_ms - 5 * 60_000);
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Stay(_)));
        i.last_manual_ms = Some(i.now_ms - 31 * 60_000);
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Switch { .. }));
        let mut c = cfg();
        c.manual_override_minutes = 0;
        i.last_manual_ms = Some(i.now_ms - 1000);
        assert!(matches!(decide(&c, &i), AutoDecision::Switch { .. }), "0 disables the override window");
    }

    #[test]
    fn g_helper_pauses_switching_unless_the_user_opted_out() {
        let mut i = input();
        i.ghelper_running = true;
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Stay(_)));
        let mut c = cfg();
        c.respect_ghelper = false;
        assert!(matches!(decide(&c, &i), AutoDecision::Switch { .. }));
    }

    #[test]
    fn unknown_power_source_never_drives_rules() {
        let mut i = input();
        i.ac_known = false;
        i.on_ac = false;
        i.percent = Some(5);
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Stay(_)));
    }

    #[test]
    fn staying_in_the_right_profile_is_a_no_op() {
        let mut i = input();
        i.current_profile = Some("balanced".into());
        assert!(matches!(decide(&cfg(), &i), AutoDecision::Stay(_)));
    }

    #[test]
    fn config_validation_rejects_nonsense() {
        let mut c = cfg();
        c.low_battery_percent = 1;
        assert!(c.validated().is_err());
        let mut c = cfg();
        c.hysteresis_percent = 50;
        assert!(c.validated().is_err());
        let mut c = cfg();
        c.on_ac_profile = " ".into();
        assert!(c.validated().is_err());
        let mut c = cfg();
        c.min_dwell_secs = 1;
        assert_eq!(c.validated().unwrap().min_dwell_secs, 15);
    }
}
