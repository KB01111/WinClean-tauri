use std::{
    fs::File,
    io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{macros::format_description, OffsetDateTime};

use crate::{
    cleanup::{recycle_file, validate_file_for_action},
    db::{
        CleanupActionInsert, Database, OffloadItemInsert, OffloadItemRecord, OffloadJobInsert,
        OffloadJobRecord,
    },
    recommendations::RecommendationCandidate,
};

const GOOGLE_DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file openid email";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const DRIVE_UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
const ROOT_FOLDER_NAME: &str = "WinClean Offloads";
const CREDENTIAL_SERVICE: &str = "WinClean Google Drive";
const REFRESH_TOKEN_USER: &str = "refresh_token";
const ACCOUNT_EMAIL_USER: &str = "account_email";
const DRIVE_REQUEST_TIMEOUT_SECS: u64 = 10 * 60;
const AUTH_CALLBACK_TIMEOUT_SECS: u64 = 120;
const AUTH_REQUEST_LINE_LIMIT: usize = 8 * 1024;
const TOKEN_EXPIRY_SKEW_SECS: u64 = 60;
const UPLOAD_CHUNK_BYTES: u64 = 16 * 1024 * 1024;
const UPLOAD_MAX_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY_MS: u64 = 500;
const BYTE_PROGRESS_EMIT_INTERVAL_MS: u64 = 500;
pub const OFFLOAD_CANCELED_MESSAGE: &str = "Offload was canceled";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveStatus {
    pub configured: bool,
    pub connected: bool,
    pub account_email: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffloadJobStatus {
    pub id: i64,
    pub scan_run_id: i64,
    pub status: String,
    pub destination_label: String,
    pub drive_folder_id: Option<String>,
    pub total_items: u64,
    pub completed_items: u64,
    pub failed_items: u64,
    pub total_bytes: u64,
    pub uploaded_bytes: u64,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub items: Vec<OffloadItemRecord>,
}

pub type OffloadProgressSink = Arc<dyn Fn(&OffloadJobStatus) + Send + Sync>;

struct OffloadProgressEmitter<'a> {
    db: &'a Database,
    job_id: i64,
    sink: Option<OffloadProgressSink>,
    last_byte_emit: Mutex<Instant>,
}

impl<'a> OffloadProgressEmitter<'a> {
    fn new(db: &'a Database, job_id: i64, sink: Option<OffloadProgressSink>) -> Self {
        Self {
            db,
            job_id,
            sink,
            last_byte_emit: Mutex::new(
                Instant::now()
                    .checked_sub(Duration::from_millis(BYTE_PROGRESS_EMIT_INTERVAL_MS))
                    .unwrap_or_else(Instant::now),
            ),
        }
    }

    fn emit_now(&self) {
        let Some(sink) = &self.sink else {
            return;
        };
        if let Ok(status) = offload_status(self.db, self.job_id) {
            sink(&status);
        }
    }

    fn emit_byte_progress(&self) {
        if self.sink.is_none() {
            return;
        }
        let should_emit = self
            .last_byte_emit
            .lock()
            .map(|mut last| {
                if last.elapsed() >= Duration::from_millis(BYTE_PROGRESS_EMIT_INTERVAL_MS) {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if should_emit {
            self.emit_now();
        }
    }
}

#[derive(Clone, Debug)]
pub struct DriveFolder {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct DriveUploadResult {
    pub file_id: String,
    pub web_url: Option<String>,
    pub size: u64,
    pub md5_checksum: Option<String>,
}

/// Progress reporting and cancellation hooks shared with upload implementations.
pub struct UploadProgress<'a> {
    pub report: &'a (dyn Fn(u64) + Send + Sync),
    pub cancel: &'a AtomicBool,
}

impl<'a> UploadProgress<'a> {
    pub fn noop(cancel: &'a AtomicBool) -> Self {
        Self {
            report: &|_| {},
            cancel,
        }
    }
}

pub trait DriveUploader: Send + Sync {
    fn ensure_batch_folder(&self, batch_name: &str) -> anyhow::Result<DriveFolder>;

    fn upload_file(
        &self,
        folder_id: &str,
        candidate: &RecommendationCandidate,
        progress: &UploadProgress<'_>,
    ) -> anyhow::Result<DriveUploadResult>;

    fn delete_file(&self, file_id: &str) -> anyhow::Result<()> {
        let _ = file_id;
        Ok(())
    }

    fn download_file(&self, file_id: &str, destination: &Path) -> anyhow::Result<u64> {
        let _ = (file_id, destination);
        anyhow::bail!("Drive download is not supported by this client")
    }
}

pub trait FileRecycler: Send + Sync {
    fn recycle_file(&self, path: &Path) -> anyhow::Result<()>;
}

pub struct WindowsRecycleBin;

impl FileRecycler for WindowsRecycleBin {
    fn recycle_file(&self, path: &Path) -> anyhow::Result<()> {
        recycle_file(path)
    }
}

pub fn google_drive_status() -> GoogleDriveStatus {
    let configured = google_client_id().is_some();
    let connected = refresh_token().is_some();
    GoogleDriveStatus {
        configured,
        connected,
        account_email: connected.then(stored_account_email).flatten(),
        error: (!configured)
            .then_some("Set WINCLEAN_GOOGLE_CLIENT_ID to enable Google Drive offload.".to_string()),
    }
}

pub fn begin_google_drive_auth() -> anyhow::Result<GoogleDriveStatus> {
    let client_id = google_client_id()
        .ok_or_else(|| anyhow::anyhow!("Set WINCLEAN_GOOGLE_CLIENT_ID before connecting Drive"))?;
    let verifier = random_url_token(32)?;
    let challenge = pkce_challenge(&verifier);
    let state = random_url_token(24)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(false)?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth2redirect/winclean");
    let auth_url = auth_url(&client_id, &redirect_uri, &challenge, &state);

    open::that(&auth_url).map_err(|error| anyhow::anyhow!("open browser: {error}"))?;
    let code = wait_for_auth_code(listener, &state)?;
    let token = exchange_auth_code(&client_id, &redirect_uri, &verifier, &code)?;
    let Some(refresh_token) = token.refresh_token else {
        anyhow::bail!(
            "Google did not return a refresh token. Remove WinClean from your Google account's \
             third-party access list and connect again."
        );
    };
    store_refresh_token(&refresh_token)?;
    if let Ok(Some(email)) = fetch_account_email(&token.access_token) {
        let _ = store_account_email(&email);
    }
    Ok(google_drive_status())
}

pub fn disconnect_google_drive() -> anyhow::Result<GoogleDriveStatus> {
    if let Ok(entry) = credential_entry(REFRESH_TOKEN_USER) {
        let _ = entry.delete_credential();
    }
    if let Ok(entry) = credential_entry(ACCOUNT_EMAIL_USER) {
        let _ = entry.delete_credential();
    }
    Ok(google_drive_status())
}

pub fn create_offload_job(
    db: &Database,
    scan_run_id: i64,
    candidates: &[RecommendationCandidate],
) -> anyhow::Result<OffloadJobStatus> {
    let created_at_ms = unix_millis();
    let total_bytes = candidates.iter().map(|candidate| candidate.size).sum();
    let destination_label = batch_folder_name();
    let job_id = db.create_offload_job(OffloadJobInsert {
        scan_run_id,
        status: "queued".to_string(),
        destination_label,
        total_items: candidates.len() as u64,
        total_bytes,
        created_at_ms,
    })?;
    for candidate in candidates {
        db.insert_offload_item(OffloadItemInsert {
            job_id,
            entry_id: candidate.entry_id,
            candidate_id: candidate.id.clone(),
            source_path: candidate.path.clone(),
            size: candidate.size,
            created_at_ms,
        })?;
    }
    offload_status(db, job_id)
}

pub fn run_offload_job_with_client(
    db: &Database,
    scan_run_id: i64,
    candidates: Vec<RecommendationCandidate>,
    drive: &dyn DriveUploader,
    recycler: &dyn FileRecycler,
    cancel: &AtomicBool,
) -> anyhow::Result<OffloadJobStatus> {
    let status = create_offload_job(db, scan_run_id, &candidates)?;
    process_offload_job_with_client(db, status.id, candidates, drive, recycler, cancel)?;
    offload_status(db, status.id)
}

pub fn process_offload_job_with_client(
    db: &Database,
    job_id: i64,
    candidates: Vec<RecommendationCandidate>,
    drive: &dyn DriveUploader,
    recycler: &dyn FileRecycler,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    process_offload_job_with_sink(db, job_id, candidates, drive, recycler, cancel, None)
}

pub fn process_offload_job_with_sink(
    db: &Database,
    job_id: i64,
    candidates: Vec<RecommendationCandidate>,
    drive: &dyn DriveUploader,
    recycler: &dyn FileRecycler,
    cancel: &AtomicBool,
    progress_sink: Option<OffloadProgressSink>,
) -> anyhow::Result<()> {
    let emitter = OffloadProgressEmitter::new(db, job_id, progress_sink);
    let started_at_ms = unix_millis();
    db.update_offload_job(job_id, "running", None, Some(started_at_ms), None, None)?;
    emitter.emit_now();

    if cancel.load(Ordering::Relaxed) {
        db.update_offload_job(job_id, "canceled", None, None, Some(unix_millis()), None)?;
        emitter.emit_now();
        return Ok(());
    }

    let job = db
        .offload_job(job_id)?
        .ok_or_else(|| anyhow::anyhow!("offload job {job_id} was not found"))?;
    let folder = match drive.ensure_batch_folder(&job.destination_label) {
        Ok(folder) => folder,
        Err(error) => {
            db.update_offload_job(
                job_id,
                "failed",
                None,
                None,
                Some(unix_millis()),
                Some(&error.to_string()),
            )?;
            emitter.emit_now();
            return Ok(());
        }
    };
    db.update_offload_job(job_id, "running", Some(&folder.id), None, None, None)?;
    emitter.emit_now();

    let items = db.offload_items(job_id)?;
    if items.len() != candidates.len() {
        let message = format!(
            "offload job items mismatch: expected {}, found {}",
            candidates.len(),
            items.len()
        );
        db.update_offload_job(
            job_id,
            "failed",
            None,
            None,
            Some(unix_millis()),
            Some(&message),
        )?;
        emitter.emit_now();
        anyhow::bail!(message);
    }
    for (item, candidate) in items.into_iter().zip(candidates.into_iter()) {
        if cancel.load(Ordering::Relaxed) {
            db.update_offload_job(job_id, "canceled", None, None, Some(unix_millis()), None)?;
            emitter.emit_now();
            return Ok(());
        }
        process_item(db, item, candidate, &folder, drive, recycler, cancel, &emitter)?;
        if cancel.load(Ordering::Relaxed) {
            db.update_offload_job(job_id, "canceled", None, None, Some(unix_millis()), None)?;
            emitter.emit_now();
            return Ok(());
        }
    }

    let final_status = offload_status(db, job_id)?;
    let status = if final_status.failed_items == 0 {
        "completed"
    } else {
        "completed_with_errors"
    };
    db.update_offload_job(job_id, status, None, None, Some(unix_millis()), None)?;
    emitter.emit_now();
    Ok(())
}

pub fn offload_status(db: &Database, job_id: i64) -> anyhow::Result<OffloadJobStatus> {
    let job = db
        .offload_job(job_id)?
        .ok_or_else(|| anyhow::anyhow!("offload job {job_id} was not found"))?;
    let items = db.offload_items(job_id)?;
    Ok(status_from_records(job, items))
}

pub fn list_offload_jobs(db: &Database, limit: u32) -> anyhow::Result<Vec<OffloadJobStatus>> {
    let jobs = db.list_offload_jobs(limit)?;
    let mut statuses = Vec::with_capacity(jobs.len());
    for job in jobs {
        let items = db.offload_items(job.id)?;
        statuses.push(status_from_records(job, items));
    }
    Ok(statuses)
}

pub fn active_offload_job(db: &Database) -> anyhow::Result<Option<OffloadJobStatus>> {
    let Some(job_id) = db.latest_active_offload_job_id()? else {
        return Ok(None);
    };
    offload_status(db, job_id).map(Some)
}

/// Downloads a previously offloaded file back from Google Drive, verifies it,
/// and restores it to its original location (or a caller-provided directory).
pub fn restore_offload_item(
    db: &Database,
    item_id: i64,
    drive: &dyn DriveUploader,
    target_dir: Option<String>,
) -> anyhow::Result<OffloadItemRecord> {
    let item = db
        .offload_item(item_id)?
        .ok_or_else(|| anyhow::anyhow!("offload item {item_id} was not found"))?;
    let drive_file_id = item
        .drive_file_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("This item was never uploaded to Google Drive"))?;

    let destination = match target_dir {
        Some(dir) => {
            let file_name = Path::new(&item.source_path)
                .file_name()
                .map(|name| name.to_owned())
                .ok_or_else(|| anyhow::anyhow!("offload item has no file name"))?;
            PathBuf::from(dir).join(file_name)
        }
        None => PathBuf::from(&item.source_path),
    };
    if destination.exists() {
        anyhow::bail!(
            "A file already exists at {}. Move it aside before restoring.",
            destination.display()
        );
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let temp_file_name = format!(
        "{}.winclean-restore-tmp",
        destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("offload-item-{item_id}"))
    );
    let temp_path = destination.with_file_name(temp_file_name);
    if temp_path.exists() {
        anyhow::bail!(
            "A file already exists at the temporary restore path {}. Move it aside before restoring.",
            temp_path.display()
        );
    }
    let downloaded_bytes = match drive.download_file(&drive_file_id, &temp_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(error);
        }
    };
    if downloaded_bytes != item.size {
        let _ = std::fs::remove_file(&temp_path);
        anyhow::bail!(
            "Restored file size mismatch: expected {} bytes, got {} bytes",
            item.size,
            downloaded_bytes
        );
    }
    if let Some(expected_md5) = item.md5_checksum.as_deref() {
        let actual_md5 = md5_for_file(&temp_path)?;
        if !actual_md5.eq_ignore_ascii_case(expected_md5) {
            let _ = std::fs::remove_file(&temp_path);
            anyhow::bail!("Restored file checksum mismatch; the local copy was discarded");
        }
    }
    if let Err(error) = std::fs::rename(&temp_path, &destination) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error.into());
    }

    db.record_cleanup_action(CleanupActionInsert {
        action_type: "drive_restore".to_string(),
        status: "completed".to_string(),
        target_path: destination.display().to_string(),
        original_path: Some(item.source_path.clone()),
        destination_path: Some(destination.display().to_string()),
        scan_run_id: None,
        explanation: Some("Restored from Google Drive offload".to_string()),
        created_at_ms: unix_millis(),
        completed_at_ms: Some(unix_millis()),
        undo_token: item.drive_web_url.clone(),
    })?;
    db.set_offload_item_recycle_status(item.id, "restored")?;
    db.offload_item(item.id)?
        .ok_or_else(|| anyhow::anyhow!("offload item {item_id} was not found after restore"))
}

pub struct GoogleDriveClient {
    client: Client,
    client_id: String,
    cached_token: Mutex<Option<CachedAccessToken>>,
}

#[derive(Clone, Debug)]
struct CachedAccessToken {
    value: String,
    expires_at: Instant,
}

impl CachedAccessToken {
    fn new(value: String, expires_in: Option<u64>, now: Instant) -> Self {
        let usable_seconds = expires_in
            .unwrap_or(0)
            .saturating_sub(TOKEN_EXPIRY_SKEW_SECS);
        Self {
            value,
            expires_at: now + Duration::from_secs(usable_seconds),
        }
    }

    fn is_usable_at(&self, now: Instant) -> bool {
        now < self.expires_at
    }
}

fn usable_cached_token(cache: &Option<CachedAccessToken>, now: Instant) -> Option<String> {
    cache
        .as_ref()
        .filter(|token| token.is_usable_at(now))
        .map(|token| token.value.clone())
}

enum ChunkOutcome {
    Incomplete(u64),
    Done(DriveFile),
}

enum SessionProbe {
    Incomplete(u64),
    Completed(DriveFile),
    Unknown,
}

impl GoogleDriveClient {
    pub fn configured() -> anyhow::Result<Self> {
        let client_id = google_client_id()
            .ok_or_else(|| anyhow::anyhow!("Set WINCLEAN_GOOGLE_CLIENT_ID before uploading"))?;
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(DRIVE_REQUEST_TIMEOUT_SECS))
                .build()?,
            client_id,
            cached_token: Mutex::new(None),
        })
    }

    fn access_token(&self) -> anyhow::Result<String> {
        {
            let cached_token = self
                .cached_token
                .lock()
                .map_err(|_| anyhow::anyhow!("Google Drive token cache lock is poisoned"))?;
            if let Some(token) = usable_cached_token(&cached_token, Instant::now()) {
                return Ok(token);
            }
        }
        let refresh_token =
            refresh_token().ok_or_else(|| anyhow::anyhow!("Connect Google Drive first"))?;
        let token = refresh_access_token(&self.client, &self.client_id, &refresh_token)?;
        let token_value = token.value.clone();
        let mut cached_token = self
            .cached_token
            .lock()
            .map_err(|_| anyhow::anyhow!("Google Drive token cache lock is poisoned"))?;
        *cached_token = Some(token);
        Ok(token_value)
    }

    fn invalidate_token(&self) {
        if let Ok(mut cached_token) = self.cached_token.lock() {
            *cached_token = None;
        }
    }

    fn find_folder(
        &self,
        token: &str,
        name: &str,
        parent_id: Option<&str>,
    ) -> anyhow::Result<Option<DriveFolder>> {
        let mut q = format!(
            "name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
            name.replace('\\', "\\\\").replace('\'', "\\'")
        );
        if let Some(parent_id) = parent_id {
            q.push_str(&format!(
                " and '{}' in parents",
                parent_id.replace('\'', "\\'")
            ));
        }
        let response = self
            .client
            .get(format!("{DRIVE_API}/files"))
            .bearer_auth(token)
            .query(&[
                ("q", q.as_str()),
                ("spaces", "drive"),
                ("fields", "files(id,name)"),
            ])
            .send()?
            .error_for_status()?
            .json::<DriveListResponse>()?;
        Ok(response.files.into_iter().next().map(|file| DriveFolder {
            id: file.id,
            name: file.name.unwrap_or_else(|| name.to_string()),
        }))
    }

    fn create_folder(
        &self,
        token: &str,
        name: &str,
        parent_id: Option<&str>,
    ) -> anyhow::Result<DriveFolder> {
        let mut metadata = serde_json::json!({
            "name": name,
            "mimeType": "application/vnd.google-apps.folder"
        });
        if let Some(parent_id) = parent_id {
            metadata["parents"] = serde_json::json!([parent_id]);
        }
        let file = self
            .client
            .post(format!("{DRIVE_API}/files"))
            .bearer_auth(token)
            .query(&[("fields", "id,name")])
            .json(&metadata)
            .send()?
            .error_for_status()?
            .json::<DriveFile>()?;
        Ok(DriveFolder {
            id: file.id,
            name: file.name.unwrap_or_else(|| name.to_string()),
        })
    }

    fn ensure_folder(
        &self,
        token: &str,
        name: &str,
        parent_id: Option<&str>,
    ) -> anyhow::Result<DriveFolder> {
        if let Some(folder) = self.find_folder(token, name, parent_id)? {
            return Ok(folder);
        }
        self.create_folder(token, name, parent_id)
    }

    fn initiate_resumable_session(
        &self,
        token: &str,
        folder_id: &str,
        candidate: &RecommendationCandidate,
    ) -> anyhow::Result<String> {
        let metadata = serde_json::json!({
            "name": candidate.name,
            "parents": [folder_id],
            "description": format!("Offloaded by WinClean from {}", candidate.path)
        });
        let initiate = self
            .client
            .post(format!("{DRIVE_UPLOAD_API}/files"))
            .bearer_auth(token)
            .query(&[
                ("uploadType", "resumable"),
                ("fields", "id,name,size,md5Checksum,webViewLink"),
            ])
            .header("Content-Type", "application/json; charset=UTF-8")
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", candidate.size)
            .json(&metadata)
            .send()?
            .error_for_status()?;
        initiate
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Google Drive did not return a resumable upload URL"))
    }

    /// Asks the resumable session how many bytes it already has, so a retry
    /// can continue from the right offset instead of starting over. If Google
    /// accepted the final chunk before the connection dropped, the probe
    /// returns the completed file metadata instead of a 308.
    fn query_session_offset(&self, token: &str, session_url: &str, total: u64) -> SessionProbe {
        let Ok(response) = self
            .client
            .put(session_url)
            .bearer_auth(token)
            .header("Content-Length", 0_u64)
            .header("Content-Range", format!("bytes */{total}"))
            .send()
        else {
            return SessionProbe::Unknown;
        };
        let status = response.status();
        if status.as_u16() == 308 {
            SessionProbe::Incomplete(next_offset_from_range_header(
                response
                    .headers()
                    .get(reqwest::header::RANGE)
                    .and_then(|value| value.to_str().ok()),
            ))
        } else if status.is_success() {
            match response.json::<DriveFile>() {
                Ok(drive_file) => SessionProbe::Completed(drive_file),
                Err(_) => SessionProbe::Unknown,
            }
        } else {
            SessionProbe::Unknown
        }
    }

    fn send_chunk(
        &self,
        token: &str,
        session_url: &str,
        chunk: &[u8],
        offset: u64,
        total: u64,
    ) -> anyhow::Result<ChunkOutcome> {
        let end = offset + (chunk.len() as u64).saturating_sub(1);
        let response = self
            .client
            .put(session_url)
            .bearer_auth(token)
            .header("Content-Length", chunk.len() as u64)
            .header("Content-Range", format!("bytes {offset}-{end}/{total}"))
            .body(chunk.to_vec())
            .send()?;
        let status = response.status();
        if status.as_u16() == 308 {
            let next = next_offset_from_range_header(
                response
                    .headers()
                    .get(reqwest::header::RANGE)
                    .and_then(|value| value.to_str().ok()),
            );
            return Ok(ChunkOutcome::Incomplete(next));
        }
        if status.is_success() {
            return Ok(ChunkOutcome::Done(response.json::<DriveFile>()?));
        }
        let retryable = status.as_u16() == 429 || status.is_server_error();
        let message = format!("Google Drive chunk upload failed with status {status}");
        if retryable {
            Err(RetryableUploadError(message).into())
        } else {
            anyhow::bail!(message)
        }
    }

    fn upload_chunked(
        &self,
        folder_id: &str,
        candidate: &RecommendationCandidate,
        progress: &UploadProgress<'_>,
    ) -> anyhow::Result<DriveFile> {
        let token = self.access_token()?;
        let session_url = self.initiate_resumable_session(&token, folder_id, candidate)?;
        let total = candidate.size;
        let mut file = File::open(&candidate.path)?;
        let mut offset = 0_u64;
        let mut attempts = 0_u32;

        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                anyhow::bail!(OFFLOAD_CANCELED_MESSAGE);
            }
            let chunk_len = (total - offset).min(UPLOAD_CHUNK_BYTES) as usize;
            file.seek(SeekFrom::Start(offset))?;
            let mut chunk = vec![0_u8; chunk_len];
            file.read_exact(&mut chunk)?;

            let token = self.access_token()?;
            match self.send_chunk(&token, &session_url, &chunk, offset, total) {
                Ok(ChunkOutcome::Done(drive_file)) => {
                    (progress.report)(total);
                    return Ok(drive_file);
                }
                Ok(ChunkOutcome::Incomplete(next_offset)) => {
                    attempts = 0;
                    offset = next_offset.min(total);
                    (progress.report)(offset);
                }
                Err(error) => {
                    let transient = error.downcast_ref::<RetryableUploadError>().is_some()
                        || error.downcast_ref::<reqwest::Error>().is_some_and(|err| {
                            err.is_timeout() || err.is_connect() || err.is_request()
                        });
                    if !transient {
                        return Err(error);
                    }
                    attempts += 1;
                    if attempts >= UPLOAD_MAX_ATTEMPTS {
                        return Err(error.context(format!(
                            "upload failed after {UPLOAD_MAX_ATTEMPTS} attempts"
                        )));
                    }
                    self.invalidate_token();
                    backoff_sleep(attempts, progress.cancel)?;
                    let token = self.access_token()?;
                    match self.query_session_offset(&token, &session_url, total) {
                        SessionProbe::Completed(drive_file) => {
                            (progress.report)(total);
                            return Ok(drive_file);
                        }
                        SessionProbe::Incomplete(confirmed) => {
                            offset = confirmed.min(total);
                            (progress.report)(offset);
                        }
                        SessionProbe::Unknown => {}
                    }
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct RetryableUploadError(String);

fn backoff_sleep(attempt: u32, cancel: &AtomicBool) -> anyhow::Result<()> {
    let exponent = attempt.saturating_sub(1).min(6);
    let base = RETRY_BASE_DELAY_MS.saturating_mul(1_u64 << exponent);
    let jitter = unix_millis().unsigned_abs() % (base / 2 + 1);
    let total = Duration::from_millis(base + jitter);
    let slept_until = Instant::now() + total;
    while Instant::now() < slept_until {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!(OFFLOAD_CANCELED_MESSAGE);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn next_offset_from_range_header(range: Option<&str>) -> u64 {
    // Header form: "bytes=0-12345" — the next byte to send is 12346.
    range
        .and_then(|value| value.rsplit('-').next())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|last_byte| last_byte + 1)
        .unwrap_or(0)
}

impl DriveUploader for GoogleDriveClient {
    fn ensure_batch_folder(&self, batch_name: &str) -> anyhow::Result<DriveFolder> {
        let token = self.access_token()?;
        let root = self.ensure_folder(&token, ROOT_FOLDER_NAME, None)?;
        self.ensure_folder(&token, batch_name, Some(&root.id))
    }

    fn upload_file(
        &self,
        folder_id: &str,
        candidate: &RecommendationCandidate,
        progress: &UploadProgress<'_>,
    ) -> anyhow::Result<DriveUploadResult> {
        let local_md5 = md5_for_file(Path::new(&candidate.path))?;
        let uploaded = self.upload_chunked(folder_id, candidate, progress)?;
        let uploaded_size = uploaded
            .size
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("Google Drive did not return file size for verification")
            })?;
        if uploaded_size != candidate.size {
            let _ = self.delete_file(&uploaded.id);
            anyhow::bail!(
                "Google Drive upload size mismatch: expected {} bytes, got {} bytes. \
                 The incomplete Drive copy was removed.",
                candidate.size,
                uploaded_size
            );
        }
        if let Some(remote_md5) = uploaded.md5_checksum.as_deref() {
            if !remote_md5.eq_ignore_ascii_case(&local_md5) {
                let _ = self.delete_file(&uploaded.id);
                anyhow::bail!(
                    "Google Drive upload checksum mismatch. The corrupt Drive copy was removed."
                );
            }
        }
        Ok(DriveUploadResult {
            file_id: uploaded.id,
            web_url: uploaded.web_view_link,
            size: uploaded_size,
            md5_checksum: Some(local_md5),
        })
    }

    fn delete_file(&self, file_id: &str) -> anyhow::Result<()> {
        let token = self.access_token()?;
        self.client
            .delete(format!("{DRIVE_API}/files/{file_id}"))
            .bearer_auth(token)
            .send()?
            .error_for_status()?;
        Ok(())
    }

    fn download_file(&self, file_id: &str, destination: &Path) -> anyhow::Result<u64> {
        let token = self.access_token()?;
        // Restores stream the whole file in one request, so the per-request
        // timeout used for API calls would abort large downloads. Use a
        // dedicated client with only a connect timeout.
        let download_client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()?;
        let mut response = download_client
            .get(format!("{DRIVE_API}/files/{file_id}"))
            .bearer_auth(token)
            .query(&[("alt", "media")])
            .send()?
            .error_for_status()?;
        let mut file = File::create(destination)?;
        let bytes = std::io::copy(&mut response, &mut file)?;
        file.sync_all()?;
        Ok(bytes)
    }
}

fn process_item(
    db: &Database,
    item: OffloadItemRecord,
    candidate: RecommendationCandidate,
    folder: &DriveFolder,
    drive: &dyn DriveUploader,
    recycler: &dyn FileRecycler,
    cancel: &AtomicBool,
    emitter: &OffloadProgressEmitter<'_>,
) -> anyhow::Result<()> {
    db.update_offload_item(item.id, "validating", None, None, 0, None, None, None, None)?;
    emitter.emit_now();

    if let Err(error) =
        validate_file_for_action(&candidate.path, candidate.size, candidate.modified_at_ms)
    {
        let result = mark_item_failed(db, item.id, "skipped", 0, &error.to_string());
        emitter.emit_now();
        return result;
    }

    db.update_offload_item(item.id, "uploading", None, None, 0, None, None, None, None)?;
    emitter.emit_now();
    let item_id = item.id;
    let report = move |uploaded: u64| {
        let _ = db.update_offload_item_progress(item_id, uploaded);
        emitter.emit_byte_progress();
    };
    let progress = UploadProgress {
        report: &report,
        cancel,
    };
    let upload = match drive.upload_file(&folder.id, &candidate, &progress) {
        Ok(upload) => upload,
        Err(error) => {
            let status = if cancel.load(Ordering::Relaxed) {
                "canceled"
            } else {
                "failed"
            };
            let result = mark_item_failed(db, item.id, status, 0, &error.to_string());
            emitter.emit_now();
            return result;
        }
    };

    if upload.size != candidate.size {
        let _ = drive.delete_file(&upload.file_id);
        let result = mark_item_failed(
            db,
            item.id,
            "failed",
            upload.size,
            "Google Drive upload verification failed",
        );
        emitter.emit_now();
        return result;
    }

    // The cancel flag may have been set while the final chunk was in flight;
    // never recycle the local original for a canceled batch.
    if cancel.load(Ordering::Relaxed) {
        let _ = drive.delete_file(&upload.file_id);
        let result = mark_item_failed(db, item.id, "canceled", 0, OFFLOAD_CANCELED_MESSAGE);
        emitter.emit_now();
        return result;
    }

    db.record_cleanup_action(CleanupActionInsert {
        action_type: "google_drive_offload".to_string(),
        status: "completed".to_string(),
        target_path: candidate.path.clone(),
        original_path: Some(candidate.path.clone()),
        destination_path: upload.web_url.clone().or_else(|| Some(folder.name.clone())),
        scan_run_id: Some(candidate.scan_run_id),
        explanation: Some(candidate.reason.clone()),
        created_at_ms: unix_millis(),
        completed_at_ms: Some(unix_millis()),
        undo_token: upload.web_url.clone(),
    })?;

    let result = match recycler.recycle_file(Path::new(&candidate.path)) {
        Ok(()) => {
            db.record_cleanup_action(CleanupActionInsert {
                action_type: "recycle_bin".to_string(),
                status: "recycled".to_string(),
                target_path: candidate.path.clone(),
                original_path: Some(candidate.path.clone()),
                destination_path: Some("Windows Recycle Bin".to_string()),
                scan_run_id: Some(candidate.scan_run_id),
                explanation: Some("Recycled after verified Google Drive upload".to_string()),
                created_at_ms: unix_millis(),
                completed_at_ms: Some(unix_millis()),
                undo_token: Some("windows-recycle-bin".to_string()),
            })?;
            db.update_offload_item(
                item.id,
                "completed",
                Some(&upload.file_id),
                upload.web_url.as_deref(),
                upload.size,
                upload.md5_checksum.as_deref(),
                None,
                Some("recycled"),
                Some(unix_millis()),
            )
        }
        Err(error) => db.update_offload_item(
            item.id,
            "uploaded_recycle_failed",
            Some(&upload.file_id),
            upload.web_url.as_deref(),
            upload.size,
            upload.md5_checksum.as_deref(),
            Some(&format!(
                "Uploaded to Drive, but moving the local file to the Recycle Bin failed: {error}"
            )),
            Some("failed"),
            Some(unix_millis()),
        ),
    };
    emitter.emit_now();
    result
}

fn mark_item_failed(
    db: &Database,
    item_id: i64,
    status: &str,
    uploaded_bytes: u64,
    error: &str,
) -> anyhow::Result<()> {
    db.update_offload_item(
        item_id,
        status,
        None,
        None,
        uploaded_bytes,
        None,
        Some(error),
        None,
        Some(unix_millis()),
    )
}

fn status_from_records(job: OffloadJobRecord, items: Vec<OffloadItemRecord>) -> OffloadJobStatus {
    OffloadJobStatus {
        id: job.id,
        scan_run_id: job.scan_run_id,
        status: job.status,
        destination_label: job.destination_label,
        drive_folder_id: job.drive_folder_id,
        total_items: job.total_items,
        completed_items: job.completed_items,
        failed_items: job.failed_items,
        total_bytes: job.total_bytes,
        uploaded_bytes: job.uploaded_bytes,
        created_at_ms: job.created_at_ms,
        started_at_ms: job.started_at_ms,
        completed_at_ms: job.completed_at_ms,
        last_error: job.last_error,
        items,
    }
}

fn auth_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let params = [
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", GOOGLE_DRIVE_SCOPE),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish();
    format!("{AUTH_URL}?{query}")
}

fn wait_for_auth_code(listener: TcpListener, expected_state: &str) -> anyhow::Result<String> {
    listener.set_nonblocking(true)?;
    let (mut stream, _) = accept_auth_callback(&listener)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let first_line = read_http_request_line(&mut stream)?;
    let path = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("OAuth callback request path was missing"))?;
    let query = path
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    let params = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect::<std::collections::HashMap<_, _>>();
    let state = params
        .get("state")
        .ok_or_else(|| anyhow::anyhow!("OAuth callback did not include state"))?;
    if state != expected_state {
        respond_to_browser(&mut stream, "WinClean rejected the sign-in response.")?;
        anyhow::bail!("OAuth state did not match");
    }
    if let Some(error) = params.get("error") {
        respond_to_browser(&mut stream, "Google Drive sign-in was canceled.")?;
        anyhow::bail!("Google OAuth returned {error}");
    }
    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("OAuth callback did not include a code"))?;
    respond_to_browser(
        &mut stream,
        "Google Drive is connected. You can return to WinClean.",
    )?;
    Ok(code)
}

fn accept_auth_callback(
    listener: &TcpListener,
) -> anyhow::Result<(TcpStream, std::net::SocketAddr)> {
    let deadline = Instant::now() + Duration::from_secs(AUTH_CALLBACK_TIMEOUT_SECS);
    loop {
        match listener.accept() {
            Ok(connection) => return Ok(connection),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    anyhow::bail!("Timed out waiting for Google Drive sign-in callback");
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_http_request_line(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            anyhow::bail!("OAuth callback request was empty");
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline
            .map(|position| position + 1)
            .unwrap_or(available.len());
        if line.len() + take > AUTH_REQUEST_LINE_LIMIT {
            anyhow::bail!("OAuth callback request line was too large");
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    let first_line = String::from_utf8(line)
        .map_err(|_| anyhow::anyhow!("OAuth callback request line was not valid UTF-8"))?;
    let first_line = first_line.trim_end_matches(['\r', '\n']).to_string();
    if first_line.is_empty() {
        anyhow::bail!("OAuth callback request was empty");
    }
    Ok(first_line)
}

fn respond_to_browser(stream: &mut std::net::TcpStream, message: &str) -> anyhow::Result<()> {
    let body = format!("<html><body><p>{message}</p></body></html>");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn exchange_auth_code(
    client_id: &str,
    redirect_uri: &str,
    verifier: &str,
    code: &str,
) -> anyhow::Result<TokenResponse> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;
    Ok(client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()?
        .error_for_status()?
        .json::<TokenResponse>()?)
}

fn fetch_account_email(access_token: &str) -> anyhow::Result<Option<String>> {
    let client = Client::builder().timeout(Duration::from_secs(15)).build()?;
    let info = client
        .get(USERINFO_URL)
        .bearer_auth(access_token)
        .send()?
        .error_for_status()?
        .json::<UserInfoResponse>()?;
    Ok(info.email)
}

fn refresh_access_token(
    client: &Client,
    client_id: &str,
    refresh_token: &str,
) -> anyhow::Result<CachedAccessToken> {
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()?;
    if response.status().is_client_error() {
        anyhow::bail!(
            "Google Drive session expired or access was revoked. Disconnect and connect Google \
             Drive again."
        );
    }
    let token = response.error_for_status()?.json::<TokenResponse>()?;
    Ok(CachedAccessToken::new(
        token.access_token,
        token.expires_in,
        Instant::now(),
    ))
}

fn random_url_token(byte_count: usize) -> anyhow::Result<String> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("generate random token: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn google_client_id() -> Option<String> {
    std::env::var("WINCLEAN_GOOGLE_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            option_env!("WINCLEAN_GOOGLE_CLIENT_ID")
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
        })
}

fn credential_entry(user: &str) -> anyhow::Result<keyring::Entry> {
    keyring::Entry::new(CREDENTIAL_SERVICE, user)
        .map_err(|error| anyhow::anyhow!("open Windows Credential Manager entry: {error}"))
}

fn refresh_token() -> Option<String> {
    credential_entry(REFRESH_TOKEN_USER).ok()?.get_password().ok()
}

fn store_refresh_token(refresh_token: &str) -> anyhow::Result<()> {
    credential_entry(REFRESH_TOKEN_USER)?
        .set_password(refresh_token)
        .map_err(|error| anyhow::anyhow!("store Google Drive refresh token: {error}"))
}

fn stored_account_email() -> Option<String> {
    credential_entry(ACCOUNT_EMAIL_USER).ok()?.get_password().ok()
}

fn store_account_email(email: &str) -> anyhow::Result<()> {
    credential_entry(ACCOUNT_EMAIL_USER)?
        .set_password(email)
        .map_err(|error| anyhow::anyhow!("store Google Drive account email: {error}"))
}

fn batch_folder_name() -> String {
    let format = format_description!("[year]-[month]-[day] [hour][minute][second] UTC");
    OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| format!("batch-{}", unix_millis()))
}

fn md5_for_file(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = md5::Md5::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DriveListResponse {
    files: Vec<DriveFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    name: Option<String>,
    size: Option<String>,
    md5_checksum: Option<String>,
    web_view_link: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_access_token_is_reused_only_before_expiry_window() {
        let now = Instant::now();
        let fresh = CachedAccessToken::new("fresh-token".to_string(), Some(3600), now);
        assert_eq!(
            usable_cached_token(&Some(fresh), now + Duration::from_secs(60)).as_deref(),
            Some("fresh-token")
        );

        let near_expiry = CachedAccessToken::new("near-expiry-token".to_string(), Some(30), now);
        assert!(usable_cached_token(&Some(near_expiry), now).is_none());

        let unknown_expiry = CachedAccessToken::new("unknown-expiry-token".to_string(), None, now);
        assert!(usable_cached_token(&Some(unknown_expiry), now).is_none());
    }

    #[test]
    fn next_offset_parses_google_range_header() {
        assert_eq!(next_offset_from_range_header(Some("bytes=0-1048575")), 1048576);
        assert_eq!(next_offset_from_range_header(None), 0);
        assert_eq!(next_offset_from_range_header(Some("garbage")), 0);
    }
}
