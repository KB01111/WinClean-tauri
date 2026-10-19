//! Child-process plumbing shared by the `odr.exe` runner and the MCP stdio
//! transport: bounded output, timeouts, cancellation, and a guarantee that the
//! whole process tree is gone when the owner lets go.
//!
//! On Windows every child is assigned to a job object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so even grandchildren started by
//! `odr.exe` die with it. Elsewhere (tests) the child itself is killed.

use std::io::{BufRead, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct SpawnedChild {
    pub child: Child,
    _job: Option<Job>,
}

pub fn spawn(program: &str, args: &[String], stdin: bool, envs: &[(&str, &str)]) -> Result<SpawnedChild, String> {
    let mut cmd = Command::new(program);
    cmd.args(args).envs(envs.iter().copied()).stdin(if stdin { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", display_name(program)))?;
    let job = Job::for_child(&child);
    Ok(SpawnedChild { child, _job: job })
}

fn display_name(program: &str) -> &str {
    program.rsplit(['\\', '/']).next().unwrap_or(program)
}

/// Stops the process (and, on Windows, everything it started) and reaps it.
pub fn terminate(spawned: &mut SpawnedChild) {
    let _ = spawned.child.kill();
    let _ = spawned.child.wait();
    spawned._job = None; // closing the job kills any stragglers
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for Job {}

#[cfg(windows)]
impl Job {
    fn for_child(child: &Child) -> Option<Job> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                windows_sys::Win32::Foundation::CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct Job;

#[cfg(not(windows))]
impl Job {
    fn for_child(_child: &Child) -> Option<Job> {
        None
    }
}

/// Reads at most `cap` bytes, then drains (and discards) the rest so the child
/// never blocks on a full pipe. Returns the bytes and whether output was cut.
pub fn read_capped(mut r: impl Read + Send + 'static, cap: usize) -> JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut truncated = false;
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(buf.len());
                    if room > 0 {
                        buf.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    if n > room {
                        truncated = true;
                    }
                }
            }
        }
        (buf, truncated)
    })
}

pub enum LineEvent {
    Line(String),
    /// A line exceeded the limit; the reader stops (the stream is no longer trustworthy).
    TooLong,
}

/// Reads newline-delimited messages with a hard per-line limit and a bounded
/// queue, so a hostile or buggy server can neither exhaust memory with an
/// endless line nor with an endless stream of lines.
pub fn spawn_line_reader(stdout: impl Read + Send + 'static, max_line: usize, queue: usize) -> (Receiver<LineEvent>, JoinHandle<()>) {
    let (tx, rx) = mpsc::sync_channel(queue);
    let handle = std::thread::spawn(move || read_lines(std::io::BufReader::new(stdout), max_line, tx));
    (rx, handle)
}

fn read_lines(mut r: impl BufRead, max_line: usize, tx: SyncSender<LineEvent>) {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let (consumed, found_newline) = {
            let buf = match r.fill_buf() {
                Ok(b) if !b.is_empty() => b,
                _ => break,
            };
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    if line.len() + i > max_line {
                        let _ = tx.send(LineEvent::TooLong);
                        return;
                    }
                    line.extend_from_slice(&buf[..i]);
                    (i + 1, true)
                }
                None => {
                    if line.len() + buf.len() > max_line {
                        let _ = tx.send(LineEvent::TooLong);
                        return;
                    }
                    line.extend_from_slice(buf);
                    (buf.len(), false)
                }
            }
        };
        r.consume(consumed);
        if found_newline {
            let text = String::from_utf8_lossy(&line).trim_end_matches('\r').to_string();
            line.clear();
            if !text.trim().is_empty() && tx.send(LineEvent::Line(text)).is_err() {
                return;
            }
        }
    }
}

pub fn join_briefly(handle: JoinHandle<()>, wait: Duration) {
    let start = std::time::Instant::now();
    while !handle.is_finished() && start.elapsed() < wait {
        std::thread::sleep(Duration::from_millis(10));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn collect(input: &[u8], max: usize) -> Vec<String> {
        let (tx, rx) = mpsc::sync_channel(1000);
        read_lines(std::io::BufReader::with_capacity(7, Cursor::new(input.to_vec())), max, tx);
        rx.try_iter().map(|e| match e { LineEvent::Line(l) => l, LineEvent::TooLong => "<TOO LONG>".into() }).collect()
    }

    #[test]
    fn lines_are_split_across_small_reads_and_blank_lines_are_skipped() {
        assert_eq!(collect(b"{\"a\":1}\r\n\n  \n{\"b\":2}\nlast-without-newline", 100), vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn an_endless_or_oversized_line_stops_the_reader_instead_of_growing_memory() {
        assert_eq!(collect(&vec![b'x'; 10_000], 100), vec!["<TOO LONG>"]);
        let mut input = b"ok\n".to_vec();
        input.extend(vec![b'y'; 500]);
        input.push(b'\n');
        assert_eq!(collect(&input, 100), vec!["ok", "<TOO LONG>"]);
        assert_eq!(collect(&vec![b'x'; 100], 100), Vec::<String>::new(), "no newline: nothing complete");
    }

    #[test]
    fn capped_reads_report_truncation_and_still_drain() {
        let (data, cut) = read_capped(Cursor::new(vec![b'a'; 50_000]), 1000).join().unwrap();
        assert_eq!((data.len(), cut), (1000, true));
        let (data, cut) = read_capped(Cursor::new(vec![b'a'; 10]), 1000).join().unwrap();
        assert_eq!((data.len(), cut), (10, false));
    }
}
