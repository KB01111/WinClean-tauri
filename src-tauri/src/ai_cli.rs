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
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }
    let read = |mut stream: Box<dyn Read + Send>| {
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
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = read(Box::new(child.stdout.take().ok_or("no stdout")?));
    let err = read(Box::new(child.stderr.take().ok_or("no stderr")?));

    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (stdout, stderr) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    match status {
        Some(status) => Ok((stdout, stderr, status.success())),
        None => Err("OpenCode did not answer in time and was stopped.".to_string()),
    }
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
    tauri::async_runtime::spawn_blocking(move || {
        let (bin, _) = find_binary().ok_or("The `opencode` command was not found. Install OpenCode and sign in with `opencode auth login`.")?;
        let mut cmd = Command::new(bin);
        cmd.current_dir(&scratch).args(["run", "--model", &model, "--format", "json", &instruction]);
        let (out, err, ok) = run_limited(cmd, Some(data.as_bytes()), RUN_TIMEOUT)?;
        if !ok && out.trim().is_empty() {
            return Err(format!("OpenCode failed: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error")));
        }
        let text = extract_text(&out);
        if text.trim().is_empty() {
            return Err("OpenCode returned no answer.".to_string());
        }
        Ok(text)
    })
    .await
    .map_err(|e| e.to_string())?
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
}
