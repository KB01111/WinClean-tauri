//! `odr.exe`, the Windows On-device Agent Registry command-line tool.
//!
//! * the executable is only ever taken from a Windows-protected location —
//!   never from the current directory or an arbitrary `PATH` entry;
//! * listing tries the documented spellings in turn (`odr.exe list`, then
//!   `odr.exe mcp list`) and keeps a diagnostic for each attempt;
//! * provisioning the agent user is a system change, so it exists only as an
//!   explicit, user-initiated operation;
//! * every run is bounded in time and output and can be cancelled.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::policy::norm_path;
use super::process::{read_capped, spawn, terminate};
use super::registry::{parse_registry, ParsedRegistry};

/// Lowest Windows build with the On-device Agent Registry (public preview).
pub const MIN_ODR_BUILD: u32 = 26220;
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_STDERR: usize = 16 * 1024;
pub const LIST_TIMEOUT: Duration = Duration::from_secs(8);
pub const PROVISION_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustedLocation {
    System32,
    WindowsDirectory,
    ProgramFiles,
    /// `%LOCALAPPDATA%\Microsoft\WindowsApps`: the directory Windows itself
    /// fills with app execution aliases. The only user-profile location accepted.
    WindowsAppsAlias,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedOdr {
    pub path: String,
    pub location: TrustedLocation,
}

/// The environment `odr.exe` is resolved from; injectable so the rules are testable anywhere.
#[derive(Clone, Debug, Default)]
pub struct OdrEnv {
    pub system_root: Option<String>,
    pub program_files: Vec<String>,
    pub local_app_data: Option<String>,
    pub path_dirs: Vec<String>,
}

impl OdrEnv {
    pub fn from_process() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            system_root: var("SystemRoot").or_else(|| var("windir")),
            program_files: ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(|k| var(k)).collect(),
            local_app_data: var("LOCALAPPDATA"),
            path_dirs: var("PATH").map(|p| p.split(';').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
        }
    }
}

pub trait FileProbe {
    fn is_file(&self, path: &str) -> bool;
}

pub struct RealFiles;
impl FileProbe for RealFiles {
    fn is_file(&self, path: &str) -> bool {
        std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
    }
}

fn join(dir: &str, file: &str) -> String {
    format!("{}\\{file}", dir.trim_end_matches(['\\', '/']))
}

fn location_of(dir: &str, env: &OdrEnv) -> Option<TrustedLocation> {
    let d = norm_path(dir);
    if d.split('/').any(|seg| seg == "..") {
        return None;
    }
    let under = |root: &str| {
        let r = norm_path(root);
        !r.is_empty() && (d == r || d.starts_with(&format!("{r}/")))
    };
    if let Some(sr) = &env.system_root {
        if d == format!("{}/system32", norm_path(sr)) {
            return Some(TrustedLocation::System32);
        }
        if d == norm_path(sr) {
            return Some(TrustedLocation::WindowsDirectory);
        }
    }
    if env.program_files.iter().any(|p| under(p)) {
        return Some(TrustedLocation::ProgramFiles);
    }
    if let Some(l) = &env.local_app_data {
        if d == format!("{}/microsoft/windowsapps", norm_path(l)) {
            return Some(TrustedLocation::WindowsAppsAlias);
        }
    }
    None
}

/// Finds `odr.exe` in a trusted location. A copy found elsewhere on `PATH`
/// (a user-writable folder, the current directory, a download) is ignored.
pub fn resolve_odr(env: &OdrEnv, files: &dyn FileProbe) -> Result<ResolvedOdr, String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(sr) = &env.system_root {
        candidates.push(join(&join(sr, "System32"), "odr.exe"));
        candidates.push(join(sr, "odr.exe"));
    }
    candidates.extend(env.path_dirs.iter().map(|d| join(d, "odr.exe")));
    let mut skipped = 0;
    for c in candidates {
        let dir = c.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        match location_of(dir, env) {
            Some(location) if files.is_file(&c) => return Ok(ResolvedOdr { path: c, location }),
            None if files.is_file(&c) => skipped += 1,
            _ => {}
        }
    }
    Err(if skipped > 0 {
        format!("odr.exe was found only in {skipped} folder(s) that Windows does not protect, so WinClean ignored it. The real tool lives in a Windows folder.")
    } else {
        "odr.exe was not found in a Windows-protected folder. The On-device Agent Registry needs Windows build 26220.7262 or later with the MCP-on-Windows preview.".to_string()
    })
}

#[derive(Debug)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub truncated: bool,
}

#[derive(Clone)]
pub struct RunOptions {
    pub timeout: Duration,
    pub cancel: Arc<AtomicBool>,
}

impl RunOptions {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout, cancel: Arc::new(AtomicBool::new(false)) }
    }
}

/// Runs short-lived `odr.exe` commands (injectable so discovery is testable).
pub trait OdrRunner: Send + Sync {
    fn run(&self, odr_path: &str, args: &[&str], opts: &RunOptions) -> Result<CmdOutput, String>;
}

pub struct SystemRunner;

impl OdrRunner for SystemRunner {
    fn run(&self, odr_path: &str, args: &[&str], opts: &RunOptions) -> Result<CmdOutput, String> {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut spawned = spawn(odr_path, &owned, false, &[])?;
        let out = spawned.child.stdout.take().ok_or("no stdout")?;
        let err = spawned.child.stderr.take().ok_or("no stderr")?;
        let (out_reader, err_reader) = (read_capped(out, MAX_OUTPUT), read_capped(err, MAX_STDERR));
        let deadline = Instant::now() + opts.timeout;
        let status = loop {
            if opts.cancel.load(Ordering::Relaxed) {
                terminate(&mut spawned);
                return Err("the operation was cancelled".into());
            }
            match spawned.child.try_wait().map_err(|e| e.to_string())? {
                Some(s) => break s,
                None if Instant::now() >= deadline => {
                    terminate(&mut spawned);
                    return Err(format!("odr.exe did not finish within {} s and was stopped", opts.timeout.as_secs()));
                }
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        };
        let (stdout, truncated) = out_reader.join().unwrap_or_default();
        let (stderr, _) = err_reader.join().unwrap_or_default();
        Ok(CmdOutput { stdout: String::from_utf8_lossy(&stdout).into_owned(), stderr: String::from_utf8_lossy(&stderr).into_owned(), success: status.success(), truncated })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAttempt {
    pub command: String,
    pub outcome: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub parsed: Option<ParsedRegistry>,
    pub attempts: Vec<ListAttempt>,
}

fn one_line(s: &str) -> String {
    super::registry::clean(&s.split_whitespace().collect::<Vec<_>>().join(" "), 160)
}

/// Lists registered servers. Read-only. The documented spellings are tried in
/// order; the first output that parses wins, and every attempt is reported.
pub fn list_servers(runner: &dyn OdrRunner, odr_path: &str, cancel: &Arc<AtomicBool>) -> Listing {
    let mut attempts = Vec::new();
    for args in [&["list"][..], &["mcp", "list"][..]] {
        let command = format!("odr.exe {}", args.join(" "));
        let opts = RunOptions { timeout: LIST_TIMEOUT, cancel: cancel.clone() };
        match runner.run(odr_path, args, &opts) {
            Err(e) => attempts.push(ListAttempt { command, outcome: e }),
            Ok(out) => {
                let text = out.stdout.trim();
                if text.is_empty() {
                    attempts.push(ListAttempt { command, outcome: format!("printed nothing{}", if out.success { "" } else { " (exit with an error)" }) + &stderr_hint(&out) });
                    continue;
                }
                match parse_registry(text) {
                    Ok(mut parsed) => {
                        if out.truncated {
                            parsed.notes.push("odr.exe printed more than 1 MiB; the listing may be incomplete.".into());
                        }
                        attempts.push(ListAttempt { command, outcome: format!("ok ({} server(s))", parsed.servers.len()) });
                        return Listing { parsed: Some(parsed), attempts };
                    }
                    Err(e) => attempts.push(ListAttempt { command, outcome: format!("{e}{}", stderr_hint(&out)) }),
                }
            }
        }
    }
    Listing { parsed: None, attempts }
}

fn stderr_hint(out: &CmdOutput) -> String {
    if out.stderr.trim().is_empty() {
        String::new()
    } else {
        format!(" — {}", one_line(&out.stderr))
    }
}

/// `odr.exe mcp provision-agent-user`: creates the agent user account that
/// contained MCP servers run as. This changes the system, so it is only ever
/// called from an explicit user action, never automatically.
pub fn provision_agent_user(runner: &dyn OdrRunner, odr_path: &str, cancel: &Arc<AtomicBool>) -> Result<String, String> {
    let opts = RunOptions { timeout: PROVISION_TIMEOUT, cancel: cancel.clone() };
    let out = runner.run(odr_path, &["mcp", "provision-agent-user"], &opts)?;
    if out.success {
        Ok(one_line(&format!("{} {}", out.stdout, out.stderr)))
    } else {
        Err(format!("odr.exe could not provision the agent user: {}", one_line(&format!("{} {}", out.stderr, out.stdout))))
    }
}

/// Arguments that start a contained session for a server, as in Microsoft's sample.
pub fn proxy_args(identifier: &str) -> Result<Vec<String>, String> {
    if !super::registry::valid_identifier(identifier) {
        return Err("the server identifier is not valid".into());
    }
    Ok(vec!["mcp".into(), "run".into(), "--proxy".into(), identifier.to_string()])
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    pub struct Files(pub HashSet<String>);
    impl FileProbe for Files {
        fn is_file(&self, p: &str) -> bool {
            self.0.contains(p)
        }
    }

    /// Replies per argument list; records every call.
    #[derive(Default)]
    pub struct ScriptedRunner {
        pub replies: Mutex<Vec<(Vec<String>, Result<CmdOutput, String>)>>,
        pub calls: Mutex<Vec<String>>,
    }
    impl ScriptedRunner {
        pub fn reply(self, args: &[&str], r: Result<CmdOutput, String>) -> Self {
            self.replies.lock().unwrap().push((args.iter().map(|a| a.to_string()).collect(), r));
            self
        }
    }
    pub fn ok(stdout: &str) -> Result<CmdOutput, String> {
        Ok(CmdOutput { stdout: stdout.into(), stderr: String::new(), success: true, truncated: false })
    }
    impl OdrRunner for ScriptedRunner {
        fn run(&self, odr_path: &str, args: &[&str], _o: &RunOptions) -> Result<CmdOutput, String> {
            self.calls.lock().unwrap().push(format!("{odr_path} {}", args.join(" ")));
            let want: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let mut replies = self.replies.lock().unwrap();
            match replies.iter().position(|(a, _)| *a == want) {
                Some(i) => replies.remove(i).1,
                None => Err("unexpected command".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;
    use crate::optimizer::mcp::registry::fixtures::structured_content;

    fn env() -> OdrEnv {
        OdrEnv {
            system_root: Some("C:\\Windows".into()),
            program_files: vec!["C:\\Program Files".into(), "C:\\Program Files (x86)".into()],
            local_app_data: Some("C:\\Users\\me\\AppData\\Local".into()),
            path_dirs: vec!["C:\\Users\\me\\Downloads".into(), "C:\\Users\\me\\bin".into(), "C:\\Windows\\System32".into()],
        }
    }
    fn files(list: &[&str]) -> Files {
        Files(list.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn odr_is_resolved_from_windows_protected_folders_only() {
        let r = resolve_odr(&env(), &files(&["C:\\Windows\\System32\\odr.exe"])).unwrap();
        assert_eq!((r.path.as_str(), r.location), ("C:\\Windows\\System32\\odr.exe", TrustedLocation::System32));
        let r = resolve_odr(&env(), &files(&["C:\\Program Files\\Tools\\odr.exe"])).map(|r| r.location);
        assert!(r.is_err(), "a Program Files subfolder is not on PATH in this environment");

        let mut e = env();
        e.path_dirs.push("C:\\Program Files\\Windows AI\\".into());
        assert_eq!(resolve_odr(&e, &files(&["C:\\Program Files\\Windows AI\\odr.exe"])).unwrap().location, TrustedLocation::ProgramFiles);
        e.path_dirs.push("c:/users/ME/appdata/local/microsoft/windowsapps".into());
        assert_eq!(resolve_odr(&e, &files(&["c:/users/ME/appdata/local/microsoft/windowsapps\\odr.exe"])).unwrap().location, TrustedLocation::WindowsAppsAlias);
    }

    #[test]
    fn a_planted_odr_in_a_user_writable_folder_is_ignored_even_if_it_comes_first_on_path() {
        let planted = files(&["C:\\Users\\me\\Downloads\\odr.exe", "C:\\Users\\me\\bin\\odr.exe", "C:\\Users\\me\\AppData\\Local\\Temp\\odr.exe"]);
        let err = resolve_odr(&env(), &planted).unwrap_err();
        assert!(err.contains("2 folder(s) that Windows does not protect"), "{err}");
        let both = files(&["C:\\Users\\me\\Downloads\\odr.exe", "C:\\Windows\\System32\\odr.exe"]);
        assert_eq!(resolve_odr(&env(), &both).unwrap().path, "C:\\Windows\\System32\\odr.exe");
        let mut sneaky = env();
        sneaky.path_dirs = vec!["C:\\Windows\\System32\\..\\..\\Users\\me\\bin".into(), "C:\\Windows\\System32.evil".into(), "C:\\Windows\\Temp".into()];
        sneaky.system_root = None;
        let p = files(&["C:\\Windows\\System32\\..\\..\\Users\\me\\bin\\odr.exe", "C:\\Windows\\System32.evil\\odr.exe", "C:\\Windows\\Temp\\odr.exe"]);
        assert!(resolve_odr(&sneaky, &p).is_err());
        assert!(resolve_odr(&env(), &files(&[])).unwrap_err().contains("was not found"));
    }

    #[test]
    fn listing_tries_both_documented_spellings_and_reports_each_attempt() {
        // `odr.exe list` fails, `odr.exe mcp list` works.
        let r = ScriptedRunner::default().reply(&["list"], Err("could not start".into())).reply(&["mcp", "list"], ok(&structured_content()));
        let l = list_servers(&r, "C:\\Windows\\System32\\odr.exe", &Arc::new(AtomicBool::new(false)));
        assert_eq!(l.parsed.unwrap().servers.len(), 2);
        assert_eq!(l.attempts.len(), 2);
        assert!(l.attempts[0].outcome.contains("could not start") && l.attempts[1].outcome.starts_with("ok"));

        // Usage text from the first spelling must not stop the second from being tried (previous behaviour).
        let r = ScriptedRunner::default().reply(&["list"], ok("Usage: odr [command]")).reply(&["mcp", "list"], ok("[]"));
        let l = list_servers(&r, "odr", &Arc::new(AtomicBool::new(false)));
        assert_eq!(l.parsed.unwrap().servers.len(), 0);
        assert!(l.attempts[0].outcome.contains("did not print JSON"));

        // The first spelling that parses is used; the second is never run.
        let r = ScriptedRunner::default().reply(&["list"], ok(&structured_content()));
        let l = list_servers(&r, "odr", &Arc::new(AtomicBool::new(false)));
        assert!(l.parsed.is_some());
        assert_eq!(r.calls.lock().unwrap().len(), 1);

        let r = ScriptedRunner::default().reply(&["list"], ok("")).reply(&["mcp", "list"], ok("{\"nope\":1}"));
        let l = list_servers(&r, "odr", &Arc::new(AtomicBool::new(false)));
        assert!(l.parsed.is_none() && l.attempts.len() == 2);
    }

    #[test]
    fn only_read_only_commands_are_used_for_listing() {
        let r = ScriptedRunner::default().reply(&["list"], ok("[]"));
        list_servers(&r, "odr", &Arc::new(AtomicBool::new(false)));
        assert!(r.calls.lock().unwrap().iter().all(|c| c.ends_with(" list")));
    }

    #[test]
    fn proxy_arguments_follow_the_official_sample_and_refuse_bad_identifiers() {
        assert_eq!(proxy_args("Microsoft.X_cw5n1h2txyewy").unwrap(), vec!["mcp", "run", "--proxy", "Microsoft.X_cw5n1h2txyewy"]);
        for bad in ["--help", "a b", "a;calc", ""] {
            assert!(proxy_args(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn provisioning_reports_success_and_failure_text() {
        let ok_r = ScriptedRunner::default().reply(&["mcp", "provision-agent-user"], Ok(CmdOutput { stdout: "done".into(), stderr: String::new(), success: true, truncated: false }));
        assert_eq!(provision_agent_user(&ok_r, "odr", &Arc::new(AtomicBool::new(false))).unwrap(), "done");
        let bad = ScriptedRunner::default().reply(&["mcp", "provision-agent-user"], Ok(CmdOutput { stdout: String::new(), stderr: "access denied".into(), success: false, truncated: false }));
        assert!(provision_agent_user(&bad, "odr", &Arc::new(AtomicBool::new(false))).unwrap_err().contains("access denied"));
    }

    // --- the real runner against a real child process ---

    #[cfg(unix)]
    #[test]
    fn the_system_runner_bounds_time_and_output_and_honours_cancellation() {
        let r = SystemRunner;
        let quick = r.run("/bin/sh", &["-c", "echo hello"], &RunOptions::new(Duration::from_secs(5))).unwrap();
        assert_eq!(quick.stdout.trim(), "hello");
        assert!(quick.success);
        let started = Instant::now();
        let err = r.run("/bin/sh", &["-c", "sleep 30"], &RunOptions::new(Duration::from_millis(300))).unwrap_err();
        assert!(err.contains("did not finish") && started.elapsed() < Duration::from_secs(5), "{err}");
        let big = r.run("/bin/sh", &["-c", "head -c 3000000 /dev/zero | tr '\\0' 'a'"], &RunOptions::new(Duration::from_secs(10))).unwrap();
        assert!(big.truncated && big.stdout.len() == MAX_OUTPUT);
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            c2.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let err = r.run("/bin/sh", &["-c", "sleep 30"], &RunOptions { timeout: Duration::from_secs(20), cancel }).unwrap_err();
        assert!(err.contains("cancelled") && started.elapsed() < Duration::from_secs(5));
        assert!(r.run("/definitely/not/here", &[], &RunOptions::new(Duration::from_secs(1))).unwrap_err().contains("could not start"));
    }
}
