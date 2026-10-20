//! Windows transport for the administrator helper.
//!
//! Topology: the unprivileged app is the pipe *server*; the elevated helper
//! (the same signed executable started with `--winclean-helper`) is the
//! *client*. That direction means:
//!
//! * the app creates the pipe first (`FILE_FLAG_FIRST_PIPE_INSTANCE`), with a
//!   protected DACL (current user + Administrators), remote clients rejected,
//!   a single instance, and a random name — nobody can squat on it;
//! * only the process the app just launched (checked by PID via
//!   `GetNamedPipeClientProcessId`) is accepted as the helper;
//! * the helper only connects to a server that is its own parent *and* runs
//!   the very same executable image, so a copy of the helper started by
//!   another program cannot be steered by an impostor server;
//! * the helper connects with `SECURITY_ANONYMOUS`, so the (unprivileged)
//!   server can never impersonate the elevated token;
//! * the helper exits when its parent exits or after ten idle minutes.
//!
//! Everything platform-neutral (argument parsing, trusted-location rule) is
//! tested on every OS; the Win32 calls are type-checked for Windows in CI.

use super::helper_client::HelperLauncher;
#[cfg(windows)]
use super::helper_client::Transport;

pub const HELPER_FLAG: &str = "--winclean-helper";
const PIPE_PREFIX: &str = "\\\\.\\pipe\\winclean-helper-";

/// Idle time after which the helper exits on its own.
pub const IDLE_TIMEOUT_SECS: u64 = 10 * 60;

#[derive(Debug, PartialEq, Eq)]
pub struct HelperArgs {
    pub pipe: String,
    pub parent_pid: u32,
}

/// True when the process was started as the helper.
pub fn is_helper_invocation(args: &[String]) -> bool {
    args.get(1).map(|a| a == HELPER_FLAG).unwrap_or(false)
}

/// Strictly parses `--winclean-helper --pipe <name> --parent <pid>`. The pipe
/// name must be exactly `\\.\pipe\winclean-helper-<32 hex digits>` so the
/// helper can never be pointed at an arbitrary pipe.
pub fn parse_helper_args(args: &[String]) -> Result<HelperArgs, String> {
    if args.len() != 6 || args[1] != HELPER_FLAG || args[2] != "--pipe" || args[4] != "--parent" {
        return Err("usage: --winclean-helper --pipe <name> --parent <pid>".into());
    }
    let pipe = args[3].clone();
    let valid = pipe.strip_prefix(PIPE_PREFIX).map(|rest| rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())).unwrap_or(false);
    if !valid {
        return Err("invalid pipe name".into());
    }
    let parent_pid: u32 = args[5].parse().map_err(|_| "invalid parent pid".to_string())?;
    if parent_pid == 0 {
        return Err("invalid parent pid".into());
    }
    Ok(HelperArgs { pipe, parent_pid })
}

pub fn pipe_name_from_nonce(nonce_hex: &str) -> String {
    format!("{PIPE_PREFIX}{nonce_hex}")
}

/// An executable may be elevated only if ordinary users cannot overwrite it:
/// it must live under one of the trusted roots (Program Files, Windows).
pub fn is_trusted_install_path(exe: &str, trusted_roots: &[String]) -> bool {
    let normalize = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    let exe = normalize(exe);
    trusted_roots.iter().filter(|r| !r.is_empty()).any(|root| {
        let root = normalize(root);
        exe.len() > root.len() && exe.starts_with(&root) && exe.as_bytes()[root.len()] == b'\\'
    })
}

#[cfg(windows)]
pub use win::{helper_main, process_is_elevated, system_launcher};

#[cfg(not(windows))]
pub fn process_is_elevated() -> bool {
    false
}

#[cfg(not(windows))]
pub fn system_launcher() -> Box<dyn HelperLauncher> {
    Box::new(super::helper_client::UnsupportedLauncher)
}

#[cfg(not(windows))]
pub fn helper_main(_args: &[String]) -> i32 {
    eprintln!("The WinClean administrator helper only runs on Windows.");
    2
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT};
    use windows_sys::Win32::System::IO::CancelSynchronousIo;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, WaitNamedPipeW,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, WaitForSingleObject, INFINITE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// Whether this process runs with an elevated token (cached: it cannot change).
    pub fn process_is_elevated() -> bool {
        static ELEVATED: OnceLock<bool> = OnceLock::new();
        *ELEVATED.get_or_init(|| unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut returned = 0u32;
            let ok = GetTokenInformation(token, TokenElevation, &mut elevation as *mut _ as *mut c_void, std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut returned);
            CloseHandle(token);
            ok != 0 && elevation.TokenIsElevated != 0
        })
    }

    fn current_user_sid_string() -> Result<String, String> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(format!("OpenProcessToken failed: {}", std::io::Error::last_os_error()));
            }
            let mut needed = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, needed, &mut needed);
            CloseHandle(token);
            if ok == 0 {
                return Err(format!("GetTokenInformation(TokenUser) failed: {}", std::io::Error::last_os_error()));
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut sid_w: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid_w) == 0 || sid_w.is_null() {
                return Err(format!("ConvertSidToStringSid failed: {}", std::io::Error::last_os_error()));
            }
            let mut len = 0usize;
            while *sid_w.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_w, len));
            LocalFree(sid_w as *mut c_void);
            Ok(sid)
        }
    }

    fn image_path_of(process: HANDLE) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) };
        (ok != 0 && len > 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }

    fn image_path_of_pid(pid: u32) -> Option<String> {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let path = image_path_of(h);
        unsafe { CloseHandle(h) };
        path
    }

    fn trusted_roots() -> Vec<String> {
        ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "SystemRoot"].iter().filter_map(|v| std::env::var(v).ok()).collect()
    }

    fn exe_location_trusted() -> bool {
        std::env::current_exe().ok().and_then(|p| p.canonicalize().ok().or(Some(p))).map(|p| is_trusted_install_path(&p.to_string_lossy().trim_start_matches("\\\\?\\").to_string(), &trusted_roots())).unwrap_or(false)
    }

    pub fn system_launcher() -> Box<dyn HelperLauncher> {
        Box::new(WindowsLauncher)
    }

    struct WindowsLauncher;

    impl HelperLauncher for WindowsLauncher {
        fn supported(&self) -> bool {
            true
        }
        fn location_trusted(&self) -> bool {
            exe_location_trusted()
        }
        fn launch(&self) -> Result<Box<dyn Transport>, String> {
            launch_helper()
        }
    }

    /// Owns the server end of the pipe until it is handed to the transport.
    struct PipeServer(HANDLE);
    impl Drop for PipeServer {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }
    unsafe impl Send for PipeServer {}

    fn create_server_pipe(name: &str, sid: &str) -> Result<PipeServer, String> {
        // Protected DACL: this user and Administrators (so an over-the-shoulder
        // admin account can still complete the UAC prompt). Nobody else.
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;BA)"));
        unsafe {
            let mut descriptor: *mut c_void = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
                return Err(format!("could not build the pipe security descriptor: {}", std::io::Error::last_os_error()));
            }
            let attrs = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
            let name_w = wide(name);
            let h = CreateNamedPipeW(
                name_w.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                8192,
                8192,
                0,
                &attrs,
            );
            let err = std::io::Error::last_os_error();
            LocalFree(descriptor);
            if h == INVALID_HANDLE_VALUE {
                return Err(format!("CreateNamedPipe failed: {err}"));
            }
            Ok(PipeServer(h))
        }
    }

    fn random_nonce() -> Result<String, String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| format!("random source failed: {e}"))?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Starts the helper with `runas` and returns its process id.
    fn shell_execute_runas(exe: &str, params: &str) -> Result<u32, String> {
        unsafe {
            let verb = wide("runas");
            let file = wide(exe);
            let parameters = wide(params);
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = parameters.as_ptr();
            info.nShow = 0; // SW_HIDE
            if ShellExecuteExW(&mut info) == 0 {
                let code = GetLastError();
                return Err(if code == ERROR_CANCELLED {
                    "Administrator approval was cancelled. No changes were made.".to_string()
                } else {
                    format!("Could not start the administrator helper: {}", std::io::Error::from_raw_os_error(code as i32))
                });
            }
            if info.hProcess.is_null() {
                return Err("Windows did not return a process handle for the administrator helper.".into());
            }
            let pid = GetProcessId(info.hProcess);
            CloseHandle(info.hProcess);
            if pid == 0 {
                return Err("could not identify the administrator helper process".into());
            }
            Ok(pid)
        }
    }

    struct PipeTransport(File);

    impl Read for PipeTransport {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for PipeTransport {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }

    /// App side: create the pipe, start the helper (UAC), accept exactly it.
    fn launch_helper() -> Result<Box<dyn Transport>, String> {
        if process_is_elevated() {
            return Err("WinClean is already running elevated; no helper is needed.".into());
        }
        let exe = std::env::current_exe().map_err(|e| format!("cannot locate WinClean: {e}"))?;
        let exe_str = exe.to_string_lossy().to_string();
        let name = pipe_name_from_nonce(&random_nonce()?);
        let sid = current_user_sid_string()?;
        let pipe = create_server_pipe(&name, &sid)?;

        // Block in ConnectNamedPipe on a worker so the UAC prompt can be answered meanwhile.
        let (tx, rx) = mpsc::channel::<bool>();
        let handle_bits = pipe.0 as usize;
        let worker = std::thread::spawn(move || {
            let h = handle_bits as HANDLE;
            let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) };
            let connected = ok != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            let _ = tx.send(connected);
        });
        let worker_handle = worker.as_raw_handle() as HANDLE;

        let params = format!("{HELPER_FLAG} --pipe {name} --parent {}", unsafe { GetCurrentProcessId() });
        let helper_pid = match shell_execute_runas(&exe_str, &params) {
            Ok(pid) => pid,
            Err(e) => {
                unsafe { CancelSynchronousIo(worker_handle) };
                let _ = worker.join();
                return Err(e);
            }
        };

        match rx.recv_timeout(Duration::from_secs(90)) {
            Ok(true) => {}
            Ok(false) => {
                let _ = worker.join();
                return Err("The administrator helper could not connect.".into());
            }
            Err(_) => {
                unsafe { CancelSynchronousIo(worker_handle) };
                let _ = worker.join();
                return Err("The administrator helper did not connect in time.".into());
            }
        }
        let _ = worker.join();

        // Authenticate the peer: it must be the process we just started.
        let mut client_pid = 0u32;
        let ok = unsafe { GetNamedPipeClientProcessId(pipe.0, &mut client_pid) };
        if ok == 0 || client_pid != helper_pid {
            unsafe { DisconnectNamedPipe(pipe.0) };
            return Err("An unexpected process connected to the helper pipe; connection refused.".into());
        }
        let raw = pipe.0;
        std::mem::forget(pipe);
        // SAFETY: `raw` is a valid, exclusively owned synchronous pipe handle.
        let file = unsafe { File::from_raw_handle(raw as *mut c_void) };
        Ok(Box::new(PipeTransport(file)))
    }

    /// Wraps the stream and records the time of the last byte moved.
    struct ActivityStream<S> {
        inner: S,
        last_activity: Arc<AtomicU64>,
        epoch: Instant,
    }
    impl<S> ActivityStream<S> {
        fn touch(&self) {
            self.last_activity.store(self.epoch.elapsed().as_secs(), Ordering::Relaxed);
        }
    }
    impl<S: Read> Read for ActivityStream<S> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.touch();
            Ok(n)
        }
    }
    impl<S: Write> Write for ActivityStream<S> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(buf)?;
            self.touch();
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    fn connect_to_server(pipe_name: &str) -> Result<File, String> {
        let name_w = wide(pipe_name);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let h = unsafe {
                CreateFileW(
                    name_w.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    // The server must never be able to impersonate this elevated token.
                    SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
                    std::ptr::null_mut(),
                )
            };
            if h != INVALID_HANDLE_VALUE {
                // SAFETY: freshly opened, exclusively owned synchronous handle.
                return Ok(unsafe { File::from_raw_handle(h as *mut c_void) });
            }
            let code = unsafe { GetLastError() };
            if Instant::now() > deadline {
                return Err(format!("could not connect to the WinClean app pipe: {}", std::io::Error::from_raw_os_error(code as i32)));
            }
            if code == ERROR_PIPE_BUSY {
                unsafe { WaitNamedPipeW(name_w.as_ptr(), 1000) };
            } else if code == ERROR_FILE_NOT_FOUND {
                std::thread::sleep(Duration::from_millis(100));
            } else {
                return Err(format!("could not connect to the WinClean app pipe: {}", std::io::Error::from_raw_os_error(code as i32)));
            }
        }
    }

    /// Entry point of the elevated helper process. Returns the exit code.
    pub fn helper_main(args: &[String]) -> i32 {
        let parsed = match parse_helper_args(args) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
        if !process_is_elevated() {
            eprintln!("the WinClean helper must run elevated");
            return 3;
        }
        let file = match connect_to_server(&parsed.pipe) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{e}");
                return 4;
            }
        };

        // The server must be our parent and the very same executable image.
        let mut server_pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as HANDLE, &mut server_pid) } == 0 || server_pid != parsed.parent_pid {
            eprintln!("pipe server is not the launching WinClean process");
            return 5;
        }
        let own = image_path_of(unsafe { GetCurrentProcess() });
        let server = image_path_of_pid(server_pid);
        match (own, server) {
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(&b) => {}
            _ => {
                eprintln!("pipe server is not the WinClean executable");
                return 6;
            }
        }

        // Exit as soon as the parent exits.
        let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parsed.parent_pid) };
        if parent.is_null() {
            eprintln!("cannot watch the parent process");
            return 7;
        }
        let parent_bits = parent as usize;
        std::thread::spawn(move || {
            unsafe { WaitForSingleObject(parent_bits as HANDLE, INFINITE) };
            std::process::exit(0);
        });

        // Exit after a period of inactivity.
        let last_activity = Arc::new(AtomicU64::new(0));
        let epoch = Instant::now();
        {
            let last = last_activity.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                if epoch.elapsed().as_secs().saturating_sub(last.load(Ordering::Relaxed)) > IDLE_TIMEOUT_SECS {
                    std::process::exit(0);
                }
            });
        }

        let mut stream = ActivityStream { inner: file, last_activity, epoch };
        let scm = super::super::privileged::SystemScm;
        super::super::privileged::serve(&mut stream, &scm, &mut |_line| {});
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    const GOOD_PIPE: &str = "\\\\.\\pipe\\winclean-helper-0123456789abcdef0123456789abcdef";

    #[test]
    fn helper_arguments_are_parsed_strictly() {
        let ok = parse_helper_args(&args(&["winclean.exe", HELPER_FLAG, "--pipe", GOOD_PIPE, "--parent", "4242"])).unwrap();
        assert_eq!(ok, HelperArgs { pipe: GOOD_PIPE.into(), parent_pid: 4242 });
        for bad in [
            vec!["winclean.exe", HELPER_FLAG],
            vec!["winclean.exe", HELPER_FLAG, "--pipe", "\\\\.\\pipe\\other", "--parent", "1"],
            vec!["winclean.exe", HELPER_FLAG, "--pipe", "\\\\.\\pipe\\winclean-helper-xyz", "--parent", "1"],
            vec!["winclean.exe", HELPER_FLAG, "--pipe", "\\\\.\\pipe\\winclean-helper-0123456789ABCDEF0123456789ABCDEF", "--parent", "1"],
            vec!["winclean.exe", HELPER_FLAG, "--pipe", GOOD_PIPE, "--parent", "0"],
            vec!["winclean.exe", HELPER_FLAG, "--pipe", GOOD_PIPE, "--parent", "abc"],
            vec!["winclean.exe", HELPER_FLAG, "--pipe", GOOD_PIPE, "--parent", "1", "--extra"],
            vec!["winclean.exe", HELPER_FLAG, "--parent", "1", "--pipe", GOOD_PIPE],
        ] {
            assert!(parse_helper_args(&args(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn helper_mode_is_only_entered_by_the_exact_first_flag() {
        assert!(is_helper_invocation(&args(&["winclean.exe", HELPER_FLAG])));
        assert!(!is_helper_invocation(&args(&["winclean.exe"])));
        assert!(!is_helper_invocation(&args(&["winclean.exe", "--other", HELPER_FLAG])));
    }

    #[test]
    fn only_non_user_writable_roots_are_trusted_for_elevation() {
        let roots = vec!["C:\\Program Files".to_string(), "C:\\Windows".to_string(), String::new()];
        assert!(is_trusted_install_path("C:\\Program Files\\WinClean\\winclean.exe", &roots));
        assert!(is_trusted_install_path("c:/program files/winclean/winclean.exe", &roots));
        assert!(!is_trusted_install_path("C:\\Users\\me\\AppData\\Local\\Programs\\WinClean\\winclean.exe", &roots));
        assert!(!is_trusted_install_path("C:\\Program Files Evil\\winclean.exe", &roots), "prefix match must respect the path separator");
        assert!(!is_trusted_install_path("D:\\src\\winclean\\target\\debug\\winclean.exe", &roots));
        assert!(!is_trusted_install_path("C:\\Program Files", &roots));
    }

    #[test]
    fn pipe_names_are_built_from_the_nonce() {
        assert_eq!(pipe_name_from_nonce("0123456789abcdef0123456789abcdef"), GOOD_PIPE);
    }
}
