//! Bridge to the user's own `opencode` command-line client.
//!
//! OpenCode's free models are offered to the OpenCode client only, so WinClean
//! does not call them over HTTP. Instead it runs `opencode run` — OpenCode itself
//! is the client — in an empty scratch folder with no auto-approved permissions,
//! hands it the already-computed storage summary on stdin, and reads back text.
//! The model's answer is never trusted: it goes through the same proposal
//! validation as every other provider.

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::Duration,
};

use serde::Serialize;
use tauri::State;

use crate::commands::AppState;

const RUN_TIMEOUT: Duration = Duration::from_secs(240);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_INPUT_BYTES: usize = 200_000;
const MAX_OUTPUT_BYTES: usize = 400_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub detail: String,
}

/// A model id like `opencode/big-pickle`: provider and model, no shell or option characters.
pub fn valid_model(model: &str) -> bool {
    let Some((provider, name)) = model.split_once('/') else { return false };
    let ok = |s: &str| !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/')) && !s.starts_with('-');
    ok(provider) && ok(name) && !model.contains("..")
}

fn candidates() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["opencode.exe", "opencode.cmd", "opencode"]
    } else {
        vec!["opencode"]
    }
}

/// Run a command with a time limit, capturing bounded stdout. Returns (stdout, stderr, success).
fn run_limited(mut command: Command, stdin: Option<&[u8]>, limit: Duration) -> Result<(String, String, bool), String> {
    command.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0); // so the whole tree can be stopped together
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }
    let read = |mut stream: Box<dyn Read + Send>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if buf.len() < MAX_OUTPUT_BYTES {
                    buf.extend_from_slice(&chunk[..n.min(MAX_OUTPUT_BYTES - buf.len())]);
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
        });
        rx
    };
    let out = read(Box::new(child.stdout.take().ok_or("no stdout")?));
    let err = read(Box::new(child.stderr.take().ok_or("no stderr")?));

    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() >= deadline => {
                kill_tree(&mut child);
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    // A grandchild that outlives the process can keep a pipe open; never wait on it for long.
    let grace = if status.is_some() { Duration::from_secs(5) } else { Duration::from_millis(500) };
    let (stdout, stderr) = (out.recv_timeout(grace).unwrap_or_default(), err.recv_timeout(grace).unwrap_or_default());
    match status {
        Some(status) => Ok((stdout, stderr, status.success())),
        None => Err("OpenCode did not answer in time and was stopped.".to_string()),
    }
}

/// Stop the process and everything it started (OpenCode launches helper processes).
fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill").args(["/PID", &pid, "/T", "/F"]).creation_flags(0x0800_0000).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-KILL", &format!("-{pid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn find_binary() -> Option<(String, String)> {
    for name in candidates() {
        let mut cmd = Command::new(name);
        cmd.arg("--version");
        if let Ok((out, _, true)) = run_limited(cmd, None, LIST_TIMEOUT) {
            let version = out.lines().next().unwrap_or("").trim().to_string();
            return Some((name.to_string(), version));
        }
    }
    None
}

#[tauri::command]
pub async fn opencode_status() -> OpenCodeStatus {
    tauri::async_runtime::spawn_blocking(|| match find_binary() {
        Some((_, version)) => OpenCodeStatus { installed: true, version: Some(version), detail: "OpenCode is installed.".to_string() },
        None => OpenCodeStatus {
            installed: false,
            version: None,
            detail: "The `opencode` command was not found. Install OpenCode (opencode.ai) and sign in with `opencode auth login`.".to_string(),
        },
    })
    .await
    .unwrap_or(OpenCodeStatus { installed: false, version: None, detail: "Could not check for OpenCode.".to_string() })
}

/// Models OpenCode offers (`opencode models`), one `provider/model` per entry.
#[tauri::command]
pub async fn opencode_models() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (bin, _) = find_binary().ok_or("The `opencode` command was not found.")?;
        let mut cmd = Command::new(bin);
        cmd.arg("models");
        let (out, err, ok) = run_limited(cmd, None, LIST_TIMEOUT)?;
        if !ok {
            return Err(format!("`opencode models` failed: {}", err.lines().next().unwrap_or("unknown error")));
        }
        let mut models: Vec<String> = out.lines().map(|l| l.trim().to_string()).filter(|l| valid_model(l)).collect();
        models.sort();
        models.dedup();
        Ok(models)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Pull the assistant text out of `--format json` event lines; fall back to the raw output.
pub fn extract_text(raw: &str) -> String {
    let mut text = String::new();
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
        let part = value.get("part").unwrap_or(&value);
        let is_text = part.get("type").and_then(|t| t.as_str()).map(|t| t == "text").unwrap_or(false) || value.get("type").and_then(|t| t.as_str()) == Some("text");
        if is_text {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()).or_else(|| value.get("text").and_then(|t| t.as_str())) {
                text.push_str(t);
                text.push('\n');
            }
        }
    }
    if text.trim().is_empty() {
        raw.to_string()
    } else {
        text
    }
}

/// Ask an OpenCode model one question. `instruction` is a short, fixed-shape line;
/// `data` (already-computed storage metadata) goes through stdin.
#[tauri::command]
pub async fn opencode_run(state: State<'_, AppState>, model: String, instruction: String, data: String) -> Result<String, String> {
    if !valid_model(&model) {
        return Err("Choose an OpenCode model such as opencode/big-pickle.".to_string());
    }
    let instruction: String = instruction.chars().filter(|c| !c.is_control() && !matches!(*c, '"' | '%' | '^' | '&' | '|' | '<' | '>')).take(600).collect();
    if instruction.trim().is_empty() {
        return Err("The instruction is empty.".to_string());
    }
    if data.len() > MAX_INPUT_BYTES {
        return Err("The storage summary is too large to send.".to_string());
    }
    // An empty folder, so the agent has nothing of the user's to look at.
    let scratch = state.app_data_dir.join("opencode-scratch");
    std::fs::create_dir_all(&scratch).map_err(|e| format!("create scratch folder: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || run_model(&scratch, &model, &instruction, &data, RUN_TIMEOUT))
        .await
        .map_err(|e| e.to_string())?
}

/// Ask OpenCode to deny every tool permission for runs started in this folder (its documented
/// project config). OpenCode still runs with the user's own privileges; this relies on it honouring the setting.
pub fn write_deny_all_config(scratch: &std::path::Path) -> Result<(), String> {
    let config = "{\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": { \"*\": \"deny\" }\n}\n";
    std::fs::write(scratch.join("opencode.json"), config).map_err(|e| format!("write OpenCode config: {e}"))
}

/// One `opencode run` in `scratch` (an empty folder): `data` on stdin, a short fixed instruction as the message.
pub fn run_model(scratch: &std::path::Path, model: &str, instruction: &str, data: &str, limit: Duration) -> Result<String, String> {
    let (bin, _) = find_binary().ok_or("The `opencode` command was not found. Install OpenCode and sign in with `opencode auth login`.")?;
    write_deny_all_config(scratch)?;
    let mut cmd = Command::new(bin);
    cmd.current_dir(scratch).args(["run", "--model", model, "--format", "json", instruction]);
    let (out, err, ok) = run_limited(cmd, Some(data.as_bytes()), limit)?;
    if !ok && out.trim().is_empty() {
        return Err(format!("OpenCode failed: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error")));
    }
    let text = extract_text(&out);
    if text.trim().is_empty() {
        return Err("OpenCode returned no answer.".to_string());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_are_strictly_validated() {
        assert!(valid_model("opencode/big-pickle"));
        assert!(valid_model("openrouter/meta-llama/llama-3.3-70b-instruct:free"));
        for bad in ["big-pickle", "", "opencode/", "/x", "opencode/a b", "opencode/--auto", "a/../b", "opencode/x;rm", "-x/y"] {
            assert!(!valid_model(bad), "{bad}");
        }
    }

    #[test]
    fn text_is_extracted_from_json_events_or_raw_output() {
        let events = "{\"type\":\"step_start\"}\n{\"type\":\"text\",\"part\":{\"type\":\"text\",\"text\":\"{\\\"summary\\\":\\\"hi\\\"}\"}}\n";
        assert_eq!(extract_text(events).trim(), "{\"summary\":\"hi\"}");
        assert_eq!(extract_text("plain answer"), "plain answer");
    }

    /// A stand-in `opencode` executable that records how it was called, so the real
    /// spawn / stdin / timeout / parsing code runs end to end.
    #[cfg(unix)]
    mod stub {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        fn install(script: &str) -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
            let guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("opencode");
            std::fs::write(&bin, script).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            let old = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}:{old}", dir.path().display()));
            (dir, guard)
        }

        const OK: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "1.2.3" ;;
  models) printf 'opencode/big-pickle\nopencode/glm-free\nnot a model line\n--flag/x\n' ;;
  run)
    input=$(cat | tr '"' "'")
    printf '{"type":"step_start"}\n'
    printf '{"type":"text","part":{"type":"text","text":"args=%s stdin=%s cwd=%s"}}\n' "$*" "$input" "$(basename "$PWD")"
    ;;
esac
"#;

        #[test]
        fn status_models_and_a_run_go_through_the_real_process_code() {
            let (_dir, _guard) = install(OK);
            let (_, version) = find_binary().expect("found on PATH");
            assert_eq!(version, "1.2.3");

            let scratch = tempfile::tempdir().unwrap();
            let text = run_model(scratch.path(), "opencode/big-pickle", "Reply with JSON only.", "{\"items\":[1,2]}", Duration::from_secs(20)).unwrap();
            let summary = text.trim();
            assert!(summary.contains("run --model opencode/big-pickle --format json Reply with JSON only."), "{summary}");
            assert!(summary.contains("stdin={'items':[1,2]}"), "data arrives on stdin, not in the arguments: {summary}");
            assert!(!summary.contains("--auto"), "permissions are never auto-approved");
            let config = std::fs::read_to_string(scratch.path().join("opencode.json")).unwrap();
            assert!(config.contains("\"*\": \"deny\""), "the scratch folder carries a deny-all permission config: {config}");
            assert!(summary.contains(&format!("cwd={}", scratch.path().file_name().unwrap().to_string_lossy())), "runs inside the scratch folder: {summary}");
        }

        #[test]
        fn a_hung_opencode_is_stopped_and_a_failing_one_reports_its_error() {
            let (_dir, _guard) = install("#!/bin/sh\ncase \"$1\" in --version) echo 1 ;; run) sleep 30 ;; esac\n");
            let scratch = tempfile::tempdir().unwrap();
            let started = std::time::Instant::now();
            let err = run_model(scratch.path(), "opencode/x", "go", "data", Duration::from_millis(600)).unwrap_err();
            assert!(err.contains("did not answer in time"), "{err}");
            assert!(started.elapsed() < Duration::from_secs(10), "the process is killed at the limit, not waited for");

            let (_dir2, _guard2) = {
                drop(_guard);
                install("#!/bin/sh\ncase \"$1\" in --version) echo 1 ;; run) echo 'Error: not signed in' >&2; exit 3 ;; esac\n")
            };
            let err = run_model(scratch.path(), "opencode/x", "go", "data", Duration::from_secs(10)).unwrap_err();
            assert_eq!(err, "OpenCode failed: Error: not signed in");
        }
    }
}
