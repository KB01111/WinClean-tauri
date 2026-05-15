use std::{
    fs::File,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
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

const GOOGLE_DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const DRIVE_UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
const ROOT_FOLDER_NAME: &str = "WinClean Offloads";
const CREDENTIAL_SERVICE: &str = "WinClean Google Drive";
const REFRESH_TOKEN_USER: &str = "refresh_token";

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

pub trait DriveUploader: Send + Sync {
    fn ensure_batch_folder(&self, batch_name: &str) -> anyhow::Result<DriveFolder>;

    fn upload_file(
        &self,
        folder_id: &str,
        candidate: &RecommendationCandidate,
    ) -> anyhow::Result<DriveUploadResult>;
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
        account_email: None,
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
    if let Some(refresh_token) = token.refresh_token {
        store_refresh_token(&refresh_token)?;
    }
    Ok(google_drive_status())
}

pub fn disconnect_google_drive() -> anyhow::Result<GoogleDriveStatus> {
    let entry = credential_entry()?;
    let _ = entry.delete_credential();
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
    let started_at_ms = unix_millis();
    db.update_offload_job(job_id, "running", None, Some(started_at_ms), None, None)?;

    if cancel.load(Ordering::Relaxed) {
        db.update_offload_job(job_id, "canceled", None, None, Some(unix_millis()), None)?;
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
            return Ok(());
        }
    };
    db.update_offload_job(job_id, "running", Some(&folder.id), None, None, None)?;

    let items = db.offload_items(job_id)?;
    for (item, candidate) in items.into_iter().zip(candidates.into_iter()) {
        if cancel.load(Ordering::Relaxed) {
            db.update_offload_job(job_id, "canceled", None, None, Some(unix_millis()), None)?;
            return Ok(());
        }
        process_item(db, item, candidate, &folder, drive, recycler)?;
    }

    let final_status = offload_status(db, job_id)?;
    let status = if final_status.failed_items == 0 {
        "completed"
    } else {
        "completed_with_errors"
    };
    db.update_offload_job(job_id, status, None, None, Some(unix_millis()), None)
}

pub fn offload_status(db: &Database, job_id: i64) -> anyhow::Result<OffloadJobStatus> {
    let job = db
        .offload_job(job_id)?
        .ok_or_else(|| anyhow::anyhow!("offload job {job_id} was not found"))?;
    let items = db.offload_items(job_id)?;
    Ok(status_from_records(job, items))
}

pub struct GoogleDriveClient {
    client: Client,
    client_id: String,
}

impl GoogleDriveClient {
    pub fn configured() -> anyhow::Result<Self> {
        let client_id = google_client_id()
            .ok_or_else(|| anyhow::anyhow!("Set WINCLEAN_GOOGLE_CLIENT_ID before uploading"))?;
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()?,
            client_id,
        })
    }

    fn access_token(&self) -> anyhow::Result<String> {
        let refresh_token =
            refresh_token().ok_or_else(|| anyhow::anyhow!("Connect Google Drive first"))?;
        refresh_access_token(&self.client, &self.client_id, &refresh_token)
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
    ) -> anyhow::Result<DriveUploadResult> {
        let token = self.access_token()?;
        let local_md5 = md5_for_file(Path::new(&candidate.path))?;
        let metadata = serde_json::json!({
            "name": candidate.name,
            "parents": [folder_id],
            "description": format!("Offloaded by WinClean from {}", candidate.path)
        });
        let initiate = self
            .client
            .post(format!("{DRIVE_UPLOAD_API}/files"))
            .bearer_auth(&token)
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
        let location = initiate
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| anyhow::anyhow!("Google Drive did not return a resumable upload URL"))?
            .to_string();

        let file = File::open(&candidate.path)?;
        let uploaded = self
            .client
            .put(location)
            .bearer_auth(&token)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", candidate.size)
            .body(reqwest::blocking::Body::new(file))
            .send()?
            .error_for_status()?
            .json::<DriveFile>()?;
        let uploaded_size = uploaded
            .size
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("Google Drive did not return file size for verification"))?;
            anyhow::bail!(
                "Google Drive upload size mismatch: expected {} bytes, got {} bytes",
                candidate.size,
                uploaded_size
            );
        }
        if let Some(remote_md5) = uploaded.md5_checksum.as_deref() {
            if !remote_md5.eq_ignore_ascii_case(&local_md5) {
                anyhow::bail!("Google Drive upload checksum mismatch");
            }
        }
        Ok(DriveUploadResult {
            file_id: uploaded.id,
            web_url: uploaded.web_view_link,
            size: uploaded_size,
            md5_checksum: Some(local_md5),
        })
    }
}

fn process_item(
    db: &Database,
    item: OffloadItemRecord,
    candidate: RecommendationCandidate,
    folder: &DriveFolder,
    drive: &dyn DriveUploader,
    recycler: &dyn FileRecycler,
) -> anyhow::Result<()> {
    db.update_offload_item(item.id, "validating", None, None, 0, None, None, None, None)?;

    if let Err(error) =
        validate_file_for_action(&candidate.path, candidate.size, candidate.modified_at_ms)
    {
        return mark_item_failed(db, item.id, "skipped", candidate.size, &error.to_string());
    }

    db.update_offload_item(item.id, "uploading", None, None, 0, None, None, None, None)?;
    let upload = match drive.upload_file(&folder.id, &candidate) {
        Ok(upload) => upload,
        Err(error) => return mark_item_failed(db, item.id, "failed", 0, &error.to_string()),
    };

    if upload.size != candidate.size {
        return mark_item_failed(
            db,
            item.id,
            "failed",
            upload.size,
            "Google Drive upload verification failed",
        );
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

    match recycler.recycle_file(Path::new(&candidate.path)) {
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
            "failed",
            Some(&upload.file_id),
            upload.web_url.as_deref(),
            upload.size,
            upload.md5_checksum.as_deref(),
            Some(&format!("uploaded but recycle failed: {error}")),
            Some("failed"),
            Some(unix_millis()),
        ),
    }
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
    listener.set_nonblocking(false)?;
    let (mut stream, _) = listener.accept()?;
    let mut buffer = [0_u8; 8192];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("OAuth callback request was empty"))?;
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

fn refresh_access_token(
    client: &Client,
    client_id: &str,
    refresh_token: &str,
) -> anyhow::Result<String> {
    let token = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()?
        .error_for_status()?
        .json::<TokenResponse>()?;
    Ok(token.access_token)
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

fn credential_entry() -> anyhow::Result<keyring::Entry> {
    keyring::Entry::new(CREDENTIAL_SERVICE, REFRESH_TOKEN_USER)
        .map_err(|error| anyhow::anyhow!("open Windows Credential Manager entry: {error}"))
}

fn refresh_token() -> Option<String> {
    credential_entry().ok()?.get_password().ok()
}

fn store_refresh_token(refresh_token: &str) -> anyhow::Result<()> {
    credential_entry()?
        .set_password(refresh_token)
        .map_err(|error| anyhow::anyhow!("store Google Drive refresh token: {error}"))
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
