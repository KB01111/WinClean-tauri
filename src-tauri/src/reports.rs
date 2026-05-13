use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    cleanup::{build_cleanup_plan, CleanupCandidate},
    db::{
        CategorySummary, CleanupActionRecord, Database, ExtensionSummary, ScanRunRecord,
        TopDirectory,
    },
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportArtifact {
    pub path: PathBuf,
    pub created_at_ms: i64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub exported_at_ms: i64,
    pub scan_run: ScanRunRecord,
    pub top_directories: Vec<TopDirectory>,
    pub extensions: Vec<ExtensionSummary>,
    pub categories: Vec<CategorySummary>,
    pub cleanup_preview: Vec<CleanupCandidate>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub exported_at_ms: i64,
    pub app_version: String,
    pub database_path: String,
    pub schema_version: Option<String>,
    pub local_diagnostics_only: bool,
    pub scan_runs: Vec<ScanRunRecord>,
    pub cleanup_history: Vec<CleanupActionRecord>,
}

pub fn write_scan_report(
    db: &Database,
    scan_run_id: i64,
    export_dir: &Path,
) -> anyhow::Result<ExportArtifact> {
    let scan_run = db
        .scan_run(scan_run_id)?
        .ok_or_else(|| anyhow::anyhow!("scan run {scan_run_id} was not found"))?;
    let report = ScanReport {
        exported_at_ms: unix_millis(),
        top_directories: db.top_directories(scan_run_id, 50)?,
        extensions: db.extension_summary(scan_run_id, 50)?,
        categories: db.category_summary(scan_run_id)?,
        cleanup_preview: build_cleanup_plan(db, scan_run_id, 25)?.candidates,
        scan_run,
    };
    write_json_artifact(
        export_dir,
        &format!("winclean-scan-report-{scan_run_id}.json"),
        &report,
    )
}

pub fn write_diagnostics_report(
    db: &Database,
    app_version: &str,
    export_dir: &Path,
) -> anyhow::Result<ExportArtifact> {
    let report = DiagnosticsReport {
        exported_at_ms: unix_millis(),
        app_version: app_version.to_string(),
        database_path: db.path().display().to_string(),
        schema_version: db.schema_version()?,
        local_diagnostics_only: true,
        scan_runs: db.list_scan_runs(25)?,
        cleanup_history: db.cleanup_history(50)?,
    };
    write_json_artifact(export_dir, "winclean-diagnostics.json", &report)
}

fn write_json_artifact<T: Serialize>(
    export_dir: &Path,
    filename: &str,
    value: &T,
) -> anyhow::Result<ExportArtifact> {
    std::fs::create_dir_all(export_dir)?;
    let path = export_dir.join(filename);
    let body = serde_json::to_string_pretty(value)?;
    std::fs::write(&path, body.as_bytes())?;
    Ok(ExportArtifact {
        path,
        created_at_ms: unix_millis(),
        bytes: body.len() as u64,
    })
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
