//! Workload detection: what is the user actually doing right now?
//!
//! The classifier is pure (inputs in, verdict out) so it is unit-tested on
//! any OS. The Windows sampler feeds it from a cheap ToolHelp snapshot, the
//! foreground window and `SHQueryUserNotificationState` — no GPU API is ever
//! touched, so detecting a game never wakes the RTX 4070.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WorkloadKind {
    #[default]
    General,
    Coding,
    Gaming,
    /// Local model inference is actively using the CPU.
    AiInference,
    /// An editor/agent in front with a local inference server running.
    AiDevelopment,
}

impl WorkloadKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkloadKind::General => "general",
            WorkloadKind::Coding => "coding",
            WorkloadKind::Gaming => "gaming",
            WorkloadKind::AiInference => "aiInference",
            WorkloadKind::AiDevelopment => "aiDevelopment",
        }
    }
}

/// A process as the classifier sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkloadProc {
    pub name: String,
    /// Recent CPU share of one core; `None` = not measured (never "busy").
    pub cpu_percent: Option<f32>,
    pub in_foreground_tree: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WorkloadInput {
    pub foreground_name: Option<String>,
    /// `SHQueryUserNotificationState` reports a Direct3D full-screen app.
    pub fullscreen_d3d: bool,
    /// The foreground app descends from a game launcher (Steam, Epic, …).
    pub game_launcher_ancestor: bool,
    pub procs: Vec<WorkloadProc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadSnapshot {
    pub kind: WorkloadKind,
    /// Plain-language reasons, shown in the UI and given to the AI assistant.
    pub evidence: Vec<String>,
    pub foreground: Option<String>,
}

pub const EDITORS: &[&str] = &[
    "code.exe", "cursor.exe", "windsurf.exe", "zed.exe", "devenv.exe", "idea64.exe", "pycharm64.exe", "webstorm64.exe",
    "rustrover64.exe", "clion64.exe", "claude.exe", "codex.exe", "opencode.exe", "windowsterminal.exe", "wt.exe",
];
pub const TOOLCHAINS: &[&str] = &["rustc.exe", "cargo.exe", "cl.exe", "link.exe", "msbuild.exe", "node.exe", "bun.exe", "tsc.exe", "clang.exe", "gcc.exe", "ninja.exe", "rust-analyzer.exe"];
pub const INFERENCE: &[&str] = &["ollama_llama_server.exe", "llama-server.exe", "ollama.exe", "ollama app.exe", "lmstudio.exe", "lm studio.exe", "lms.exe", "lemonade-server.exe", "koboldcpp.exe"];
pub const GAME_LAUNCHERS: &[&str] = &["steam.exe", "epicgameslauncher.exe", "battle.net.exe", "gamelaunchhelper.exe", "riotclientservices.exe", "eadesktop.exe", "upc.exe", "galaxyclient.exe", "xboxpcapp.exe"];

/// Toolchain/inference processes above this CPU share count as "active".
const ACTIVE_CPU: f32 = 10.0;
const INFERENCE_ACTIVE_CPU: f32 = 15.0;

fn lower(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn is_in(list: &[&str], name: &str) -> bool {
    let n = lower(name);
    list.iter().any(|x| *x == n)
}

pub fn is_game_launcher(name: &str) -> bool {
    is_in(GAME_LAUNCHERS, name)
}

pub fn detect(input: &WorkloadInput) -> WorkloadSnapshot {
    let fg = input.foreground_name.as_deref();
    let mut evidence = Vec::new();

    if input.fullscreen_d3d {
        evidence.push("A Direct3D full-screen application is in the foreground.".to_string());
        return snap(WorkloadKind::Gaming, evidence, fg);
    }
    if input.game_launcher_ancestor {
        evidence.push(format!("{} was started by a game launcher.", fg.unwrap_or("The foreground app")));
        return snap(WorkloadKind::Gaming, evidence, fg);
    }

    let inference_busy: Vec<&WorkloadProc> = input.procs.iter().filter(|p| is_in(INFERENCE, &p.name) && p.cpu_percent.unwrap_or(0.0) >= INFERENCE_ACTIVE_CPU).collect();
    let inference_present = input.procs.iter().any(|p| is_in(INFERENCE, &p.name));
    let editor_front = fg.map(|f| is_in(EDITORS, f)).unwrap_or(false);
    let toolchain_busy: Vec<&WorkloadProc> = input.procs.iter().filter(|p| is_in(TOOLCHAINS, &p.name) && p.cpu_percent.unwrap_or(0.0) >= ACTIVE_CPU).collect();

    if !inference_busy.is_empty() && !editor_front {
        evidence.push(format!("{} is using the CPU for local inference.", inference_busy[0].name));
        return snap(WorkloadKind::AiInference, evidence, fg);
    }
    if inference_present && (editor_front || !toolchain_busy.is_empty()) {
        evidence.push("A local inference server is running while you work in an editor or toolchain.".to_string());
        if let Some(f) = fg {
            evidence.push(format!("Foreground: {f}."));
        }
        return snap(WorkloadKind::AiDevelopment, evidence, fg);
    }
    if !inference_busy.is_empty() {
        evidence.push(format!("{} is busy with local inference.", inference_busy[0].name));
        return snap(WorkloadKind::AiInference, evidence, fg);
    }
    if editor_front {
        evidence.push(format!("{} (an editor/terminal) is in the foreground.", fg.unwrap_or("")));
        return snap(WorkloadKind::Coding, evidence, fg);
    }
    if !toolchain_busy.is_empty() {
        evidence.push(format!("{} is compiling or running a build.", toolchain_busy[0].name));
        return snap(WorkloadKind::Coding, evidence, fg);
    }
    evidence.push("No gaming, coding, or inference activity detected.".to_string());
    snap(WorkloadKind::General, evidence, fg)
}

fn snap(kind: WorkloadKind, evidence: Vec<String>, fg: Option<&str>) -> WorkloadSnapshot {
    WorkloadSnapshot { kind, evidence, foreground: fg.map(|s| s.to_string()) }
}

/// Tracks per-process CPU time between samples so CPU shares need no sleeping.
#[derive(Default)]
pub struct CpuTracker {
    last: std::collections::HashMap<u32, (u64, std::time::Instant)>,
}

impl CpuTracker {
    /// CPU share of one core since the previous call for this PID; the first
    /// observation returns `None` (never "idle").
    pub fn share(&mut self, pid: u32, ticks_100ns: u64, now: std::time::Instant) -> Option<f32> {
        let prev = self.last.insert(pid, (ticks_100ns, now));
        let (prev_ticks, prev_at) = prev?;
        let elapsed = now.saturating_duration_since(prev_at).as_secs_f64();
        if elapsed < 0.05 {
            return None;
        }
        let used = ticks_100ns.saturating_sub(prev_ticks) as f64 / 10_000_000.0;
        Some((used / elapsed * 100.0) as f32)
    }
    /// Forget PIDs that are gone.
    pub fn retain(&mut self, alive: &std::collections::HashSet<u32>) {
        self.last.retain(|pid, _| alive.contains(pid));
    }
}

/// Samples the machine and classifies the workload (Windows only).
#[cfg(windows)]
pub fn sample(tracker: &mut CpuTracker) -> WorkloadSnapshot {
    use super::processes::{cpu_ticks_for_pid, foreground_process, light_snapshot};
    use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};

    let procs = light_snapshot();
    let fg = foreground_process();
    let mut state = 0i32;
    let fullscreen_d3d = unsafe { SHQueryUserNotificationState(&mut state) } == 0 && state == QUNS_RUNNING_D3D_FULL_SCREEN;

    let name_of = |pid: u32| procs.iter().find(|p| p.pid == pid).map(|p| p.name.clone());
    let game_launcher_ancestor = fg.as_ref().map(|(pid, _)| {
        let mut cur = *pid;
        for _ in 0..8 {
            let Some(p) = procs.iter().find(|p| p.pid == cur) else { return false };
            if p.pid != *pid && is_game_launcher(&p.name) {
                return true;
            }
            if p.parent_pid == 0 || p.parent_pid == cur {
                return false;
            }
            cur = p.parent_pid;
        }
        false
    });
    let _ = name_of;

    // Only processes the classifier cares about are measured.
    let interesting: Vec<_> = procs.iter().filter(|p| is_in(EDITORS, &p.name) || is_in(TOOLCHAINS, &p.name) || is_in(INFERENCE, &p.name)).collect();
    let now = std::time::Instant::now();
    let mut seen = std::collections::HashSet::new();
    let mut input_procs = Vec::new();
    for p in interesting {
        seen.insert(p.pid);
        let cpu = cpu_ticks_for_pid(p.pid).and_then(|t| tracker.share(p.pid, t, now));
        input_procs.push(WorkloadProc { name: p.name.clone(), cpu_percent: cpu, in_foreground_tree: false });
    }
    tracker.retain(&seen);
    detect(&WorkloadInput { foreground_name: fg.map(|(_, n)| n), fullscreen_d3d, game_launcher_ancestor: game_launcher_ancestor.unwrap_or(false), procs: input_procs })
}

#[cfg(not(windows))]
pub fn sample(_tracker: &mut CpuTracker) -> WorkloadSnapshot {
    WorkloadSnapshot { kind: WorkloadKind::General, evidence: vec!["Workload detection is only available on Windows.".into()], foreground: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, cpu: Option<f32>) -> WorkloadProc {
        WorkloadProc { name: name.into(), cpu_percent: cpu, in_foreground_tree: false }
    }
    fn input(fg: &str, procs: Vec<WorkloadProc>) -> WorkloadInput {
        WorkloadInput { foreground_name: Some(fg.into()), procs, ..Default::default() }
    }

    #[test]
    fn fullscreen_d3d_or_launcher_descendant_means_gaming() {
        let mut i = input("game.exe", vec![]);
        i.fullscreen_d3d = true;
        assert_eq!(detect(&i).kind, WorkloadKind::Gaming);
        let mut i = input("game.exe", vec![]);
        i.game_launcher_ancestor = true;
        assert_eq!(detect(&i).kind, WorkloadKind::Gaming);
    }

    #[test]
    fn gaming_wins_over_everything_else() {
        let mut i = input("Code.exe", vec![p("ollama_llama_server.exe", Some(90.0)), p("rustc.exe", Some(90.0))]);
        i.fullscreen_d3d = true;
        assert_eq!(detect(&i).kind, WorkloadKind::Gaming);
    }

    #[test]
    fn editor_in_front_is_coding_and_matching_is_case_insensitive() {
        assert_eq!(detect(&input("Code.EXE", vec![])).kind, WorkloadKind::Coding);
        assert_eq!(detect(&input("Cursor.exe", vec![])).kind, WorkloadKind::Coding);
        assert_eq!(detect(&input("claude.exe", vec![])).kind, WorkloadKind::Coding);
    }

    #[test]
    fn a_busy_compiler_is_coding_even_with_the_editor_in_the_background() {
        assert_eq!(detect(&input("chrome.exe", vec![p("rustc.exe", Some(80.0))])).kind, WorkloadKind::Coding);
        // An unmeasured or idle toolchain process is not evidence of a build.
        assert_eq!(detect(&input("chrome.exe", vec![p("rustc.exe", None), p("node.exe", Some(1.0))])).kind, WorkloadKind::General);
    }

    #[test]
    fn busy_inference_without_an_editor_is_ai_inference() {
        let i = input("chrome.exe", vec![p("ollama_llama_server.exe", Some(400.0))]);
        assert_eq!(detect(&i).kind, WorkloadKind::AiInference);
        // A resident but idle server is not active inference.
        let idle = input("chrome.exe", vec![p("ollama.exe", Some(0.5))]);
        assert_eq!(detect(&idle).kind, WorkloadKind::General);
    }

    #[test]
    fn editor_plus_inference_server_is_ai_development() {
        let i = input("Code.exe", vec![p("ollama.exe", Some(0.5))]);
        let s = detect(&i);
        assert_eq!(s.kind, WorkloadKind::AiDevelopment);
        assert!(!s.evidence.is_empty());
        let busy = input("cursor.exe", vec![p("ollama_llama_server.exe", Some(300.0))]);
        assert_eq!(detect(&busy).kind, WorkloadKind::AiDevelopment);
    }

    #[test]
    fn nothing_special_is_general() {
        assert_eq!(detect(&input("notepad.exe", vec![])).kind, WorkloadKind::General);
        assert_eq!(detect(&WorkloadInput::default()).kind, WorkloadKind::General);
    }

    #[test]
    fn cpu_tracker_needs_two_observations_and_computes_a_share() {
        let mut t = CpuTracker::default();
        let t0 = std::time::Instant::now();
        assert_eq!(t.share(1, 0, t0), None, "first observation is never idle");
        let t1 = t0 + std::time::Duration::from_secs(2);
        // 1 s of CPU in 2 s wall = 50% of one core.
        let share = t.share(1, 10_000_000, t1).unwrap();
        assert!((share - 50.0).abs() < 0.5, "{share}");
        let mut alive = std::collections::HashSet::new();
        alive.insert(2);
        t.retain(&alive);
        assert_eq!(t.share(1, 20_000_000, t1 + std::time::Duration::from_secs(1)), None, "forgotten PIDs restart");
    }
}
