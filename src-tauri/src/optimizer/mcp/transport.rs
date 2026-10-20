//! The byte-level side of an MCP connection, behind a trait so the protocol
//! client is testable without a process and the process handling is testable
//! without a protocol.

use std::io::Write;
use std::process::ChildStdin;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::process::{join_briefly, spawn, spawn_line_reader, terminate, LineEvent, SpawnedChild};

/// Longest single JSON-RPC message accepted from a server.
pub const MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const QUEUE_DEPTH: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum Recv {
    Line(String),
    Timeout,
    Closed,
    /// The server sent a message larger than [`MAX_MESSAGE_BYTES`].
    TooLong,
}

pub trait McpTransport: Send {
    fn send_line(&mut self, line: &str) -> Result<(), String>;
    fn recv_line(&mut self, wait: Duration) -> Recv;
    /// Releases everything: after this returns no process or thread is left.
    fn close(&mut self);
}

/// A server reached through a child process speaking newline-delimited JSON-RPC.
pub struct StdioTransport {
    child: Option<SpawnedChild>,
    stdin: Option<ChildStdin>,
    rx: Receiver<LineEvent>,
    reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<(Vec<u8>, bool)>>,
}

impl StdioTransport {
    pub fn spawn(program: &str, args: &[String], envs: &[(&str, &str)]) -> Result<Self, String> {
        let mut spawned = spawn(program, args, true, envs)?;
        let stdin = spawned.child.stdin.take();
        let stdout = spawned.child.stdout.take().ok_or("no stdout")?;
        // stderr is drained and discarded so a chatty server can never block on a full pipe.
        let stderr_reader = spawned.child.stderr.take().map(|e| super::process::read_capped(e, 4096));
        let (rx, reader) = spawn_line_reader(stdout, MAX_MESSAGE_BYTES, QUEUE_DEPTH);
        Ok(Self { child: Some(spawned), stdin, rx, reader: Some(reader), stderr_reader })
    }

    pub fn is_running(&mut self) -> bool {
        self.child.as_mut().map(|c| matches!(c.child.try_wait(), Ok(None))).unwrap_or(false)
    }
}

impl McpTransport for StdioTransport {
    fn send_line(&mut self, line: &str) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("the connection is closed")?;
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        stdin.write_all(&data).and_then(|_| stdin.flush()).map_err(|e| format!("could not write to the server: {e}"))
    }

    fn recv_line(&mut self, wait: Duration) -> Recv {
        match self.rx.recv_timeout(wait) {
            Ok(LineEvent::Line(l)) => Recv::Line(l),
            Ok(LineEvent::TooLong) => Recv::TooLong,
            Err(RecvTimeoutError::Timeout) => Recv::Timeout,
            Err(RecvTimeoutError::Disconnected) => Recv::Closed,
        }
    }

    fn close(&mut self) {
        self.stdin = None; // a well-behaved server exits when its stdin closes
        if let Some(mut c) = self.child.take() {
            terminate(&mut c);
        }
        // With the process (tree) gone the pipes close and the readers end.
        self.rx = std::sync::mpsc::sync_channel(1).1;
        if let Some(r) = self.reader.take() {
            join_briefly(r, Duration::from_secs(2));
        }
        if let Some(r) = self.stderr_reader.take() {
            let start = std::time::Instant::now();
            while !r.is_finished() && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
            }
            if r.is_finished() {
                let _ = r.join();
            }
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
pub(crate) mod scripted {
    use super::*;
    use serde_json::Value;
    use std::collections::VecDeque;

    type Responder = Box<dyn FnMut(&Value) -> Vec<String> + Send>;

    /// An in-memory server: every message the client sends is passed to the
    /// responder, whose lines are queued for the client to read.
    pub struct ScriptedTransport {
        pub sent: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
        queue: VecDeque<String>,
        respond: Responder,
        pub closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl ScriptedTransport {
        pub fn new(respond: impl FnMut(&Value) -> Vec<String> + Send + 'static) -> Self {
            Self { sent: Default::default(), queue: VecDeque::new(), respond: Box::new(respond), closed: Default::default() }
        }
    }

    impl McpTransport for ScriptedTransport {
        fn send_line(&mut self, line: &str) -> Result<(), String> {
            let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            self.sent.lock().unwrap().push(v.clone());
            self.queue.extend((self.respond)(&v));
            Ok(())
        }
        fn recv_line(&mut self, wait: Duration) -> Recv {
            match self.queue.pop_front() {
                Some(l) if l == "<TOO LONG>" => Recv::TooLong,
                Some(l) if l == "<CLOSED>" => Recv::Closed,
                Some(l) => Recv::Line(l),
                None => {
                    std::thread::sleep(wait.min(Duration::from_millis(5)));
                    Recv::Timeout
                }
            }
        }
        fn close(&mut self) {
            self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::BufRead;
    use std::time::Instant;

    /// Entry point of a mock MCP server: runs only when spawned by these tests
    /// (this test binary re-runs itself with `WINCLEAN_MCP_MOCK` set).
    #[test]
    fn mock_server_child() {
        let Ok(mode) = std::env::var("WINCLEAN_MCP_MOCK") else { return };
        let mut out = std::io::stdout();
        // The harness prints "test <name> ... " without a newline right before
        // running this function; end that line so JSON replies start cleanly.
        let _ = writeln!(out);
        let _ = out.flush();
        if mode == "flood" {
            // One enormous line, never terminated: the reader must cut it off.
            let chunk = vec![b'x'; 64 * 1024];
            loop {
                if out.write_all(&chunk).and_then(|_| out.flush()).is_err() {
                    std::process::exit(0);
                }
            }
        }
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(id) = msg.get("id").cloned() else { continue };
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => json!({ "protocolVersion": "2025-06-18", "serverInfo": { "name": "mock", "version": "1" }, "capabilities": { "tools": {} } }),
                "tools/list" => json!({ "tools": [ { "name": "get_file_details", "inputSchema": { "type": "object", "properties": { "path": { "type": "string" } } } } ] }),
                "ignore" => continue,
                _ => json!({}),
            };
            let _ = writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            let _ = out.flush();
        }
        std::process::exit(0);
    }

    pub fn spawn_mock(mode: &str) -> StdioTransport {
        let exe = std::env::current_exe().unwrap();
        StdioTransport::spawn(
            exe.to_str().unwrap(),
            &["optimizer::mcp::transport::tests::mock_server_child".into(), "--exact".into(), "--nocapture".into(), "--test-threads=1".into()],
            &[("WINCLEAN_MCP_MOCK", mode)],
        )
        .unwrap()
    }

    fn exchange(t: &mut StdioTransport, v: Value) -> Value {
        t.send_line(&v.to_string()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match t.recv_line(Duration::from_millis(200)) {
                Recv::Line(l) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&l) {
                        return v;
                    }
                }
                Recv::Timeout if Instant::now() < deadline => {}
                other => panic!("no reply: {other:?}"),
            }
        }
    }

    #[test]
    fn a_real_child_process_answers_and_is_gone_after_close() {
        let mut t = spawn_mock("echo");
        let r = exchange(&mut t, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }));
        assert_eq!(r["result"]["serverInfo"]["name"], "mock");
        assert!(t.is_running());
        t.close();
        assert!(!t.is_running(), "closing must leave no child process behind");
        assert!(t.send_line("{}").is_err());
        t.close(); // idempotent
    }

    #[test]
    fn dropping_the_transport_stops_the_child() {
        let t = spawn_mock("echo");
        let pid = t.child.as_ref().unwrap().child.id();
        drop(t);
        #[cfg(unix)]
        {
            let alive = std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false);
            assert!(!alive, "child {pid} survived the drop");
        }
        let _ = pid;
    }

    #[test]
    fn an_endless_line_from_a_hostile_server_is_cut_off_and_the_process_is_still_cleaned_up() {
        let mut t = spawn_mock("flood");
        let deadline = Instant::now() + Duration::from_secs(20);
        let got = loop {
            match t.recv_line(Duration::from_millis(200)) {
                Recv::TooLong => break true,
                Recv::Timeout if Instant::now() < deadline => {}
                Recv::Line(_) => {}
                _ => break false,
            }
        };
        assert!(got, "the oversized message must be reported");
        t.close();
        assert!(!t.is_running());
    }

    #[test]
    fn a_missing_program_is_a_clear_error() {
        assert!(StdioTransport::spawn("/definitely/not/here", &[], &[]).err().unwrap().contains("could not start"));
    }
}
