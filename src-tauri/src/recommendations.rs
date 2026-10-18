use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    cleanup::{cleanup_candidate_id, cleanup_reason, is_protected_path, CleanupRisk},
    db::{CleanupFileRecord, Database},
};

const LARGE_FILE_BYTES: u64 = 100 * 1024 * 1024;
const LARGE_INSTALLER_OR_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;
const DUPLICATE_REVIEW_BYTES: u64 = 10 * 1024 * 1024;
const OLD_DOWNLOAD_DAYS: u64 = 30;
const STALE_FILE_DAYS: u64 = 180;
const STALE_LARGE_FILE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationFilters {
    pub mode: Option<String>,
    pub min_size: Option<u64>,
    pub min_age_days: Option<u64>,
    pub only_offload_eligible: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationPlan {
    pub scan_run_id: i64,
    pub candidates: Vec<RecommendationCandidate>,
    pub clean_bytes: u64,
    pub offload_bytes: u64,
    pub review_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecommendationKind {
    Clean,
    Offload,
    Review,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecommendedAction {
    RecycleBin,
    GoogleDriveOffload,
    Review,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecommendationRisk {
    Low,
    Review,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationCandidate {
    pub id: String,
    pub entry_id: i64,
    pub scan_run_id: i64,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub kind: RecommendationKind,
    pub risk_level: RecommendationRisk,
    pub recommended_action: RecommendedAction,
    pub reason: String,
    pub confidence: f32,
    pub age_days: Option<u64>,
    pub eligible_for_offload: bool,
    pub modified_at_ms: Option<i64>,
}

pub fn build_recommendations(
    db: &Database,
    scan_run_id: i64,
    filters: RecommendationFilters,
) -> anyhow::Result<RecommendationPlan> {
    let min_size = filters.min_size.unwrap_or(0);
    let min_age_days = filters.min_age_days.unwrap_or(0);
    let records = db.recommendation_file_records(scan_run_id, min_size, min_age_days, 20_000)?;
    let duplicate_keys = db.duplicate_name_size_keys(scan_run_id, DUPLICATE_REVIEW_BYTES)?;
    let mode = filters.mode.as_deref().map(str::to_ascii_lowercase);
    let only_offload = filters.only_offload_eligible.unwrap_or(false);

    let mut candidates = records
        .into_iter()
        .filter_map(|record| recommendation_candidate(scan_run_id, record, &duplicate_keys))
        .filter(|candidate| {
            if candidate.size < min_size {
                return false;
            }
            if min_age_days > 0 && candidate.age_days.unwrap_or(0) < min_age_days {
                return false;
            }
            if only_offload && !candidate.eligible_for_offload {
                return false;
            }
            match mode.as_deref() {
                Some("clean") => candidate.kind == RecommendationKind::Clean,
                Some("offload") => candidate.kind == RecommendationKind::Offload,
                Some("review") => candidate.kind == RecommendationKind::Review,
                _ => true,
            }
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        kind_order(&left.kind)
            .cmp(&kind_order(&right.kind))
            .then_with(|| right.size.cmp(&left.size))
            .then_with(|| left.path.cmp(&right.path))
    });

    let clean_bytes = sum_kind(&candidates, RecommendationKind::Clean);
    let offload_bytes = sum_kind(&candidates, RecommendationKind::Offload);
    let review_bytes = sum_kind(&candidates, RecommendationKind::Review);

    Ok(RecommendationPlan {
        scan_run_id,
        candidates,
        clean_bytes,
        offload_bytes,
        review_bytes,
        warnings: vec![
            "Clean candidates are conservative and use the Windows Recycle Bin.".to_string(),
            "Offload candidates require Google Drive upload verification before local recycling."
                .to_string(),
            "Review candidates are never auto-selected.".to_string(),
        ],
    })
}

pub fn resolve_recommendation_candidates(
    candidates: &[RecommendationCandidate],
    candidate_ids: &[String],
) -> anyhow::Result<Vec<RecommendationCandidate>> {
    let candidates_by_id = candidates
        .iter()
        .map(|candidate| (candidate.id.as_str(), candidate))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();

    for candidate_id in candidate_ids {
        if !seen.insert(candidate_id.as_str()) {
            continue;
        }

        let Some(candidate) = candidates_by_id.get(candidate_id.as_str()) else {
            anyhow::bail!("recommendation candidate {candidate_id} is no longer available");
        };
        resolved.push((*candidate).clone());
    }

    Ok(resolved)
}

pub fn resolve_offload_recommendation_candidates(
    candidates: &[RecommendationCandidate],
    candidate_ids: &[String],
) -> anyhow::Result<Vec<RecommendationCandidate>> {
    let resolved = resolve_recommendation_candidates(candidates, candidate_ids)?;
    if resolved.iter().any(|candidate| {
        candidate.kind != RecommendationKind::Offload
            || candidate.recommended_action != RecommendedAction::GoogleDriveOffload
            || !candidate.eligible_for_offload
    }) {
        anyhow::bail!("Only Google Drive offload candidates can be sent to Google Drive");
    }
    Ok(resolved)
}

fn recommendation_candidate(
    scan_run_id: i64,
    record: CleanupFileRecord,
    duplicate_keys: &HashSet<(String, u64)>,
) -> Option<RecommendationCandidate> {
    if record.is_reparse_point || record.is_placeholder || record.is_system {
        return None;
    }
    if is_protected_path(&record.path) {
        return None;
    }

    let age_days = age_days(record.modified_at_ms);
    if let Some((reason, risk)) = cleanup_reason(&record) {
        return Some(candidate(
            scan_run_id,
            record,
            RecommendationKind::Clean,
            cleanup_risk(risk),
            RecommendedAction::RecycleBin,
            reason,
            0.95,
            age_days,
            false,
        ));
    }

    if let Some(reason) = offload_reason(&record, age_days) {
        return Some(candidate(
            scan_run_id,
            record,
            RecommendationKind::Offload,
            RecommendationRisk::Review,
            RecommendedAction::GoogleDriveOffload,
            reason,
            0.72,
            age_days,
            true,
        ));
    }

    let duplicate_key = (record.name.to_ascii_lowercase(), record.size);
    if record.size >= DUPLICATE_REVIEW_BYTES && duplicate_keys.contains(&duplicate_key) {
        return Some(candidate(
            scan_run_id,
            record,
            RecommendationKind::Review,
            RecommendationRisk::Review,
            RecommendedAction::Review,
            "Possible duplicate file with matching name and size".to_string(),
            0.5,
            age_days,
            true,
        ));
    }

    None
}

fn candidate(
    scan_run_id: i64,
    record: CleanupFileRecord,
    kind: RecommendationKind,
    risk_level: RecommendationRisk,
    recommended_action: RecommendedAction,
    reason: String,
    confidence: f32,
    age_days: Option<u64>,
    eligible_for_offload: bool,
) -> RecommendationCandidate {
    RecommendationCandidate {
        id: cleanup_candidate_id(scan_run_id, record.id),
        entry_id: record.id,
        scan_run_id,
        path: record.path,
        name: record.name,
        size: record.size,
        kind,
        risk_level,
        recommended_action,
        reason,
        confidence,
        age_days,
        eligible_for_offload,
        modified_at_ms: record.modified_at_ms,
    }
}

fn offload_reason(record: &CleanupFileRecord, age_days: Option<u64>) -> Option<String> {
    let extension = record
        .extension
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let path = record.path.replace('/', "\\").to_ascii_lowercase();
    let in_downloads = path.contains("\\downloads\\");

    if in_downloads && age_days.unwrap_or(0) >= OLD_DOWNLOAD_DAYS {
        return Some("Old file in Downloads".to_string());
    }
    if matches!(
        extension.as_str(),
        "exe" | "msi" | "zip" | "7z" | "rar" | "tar" | "gz" | "iso"
    ) && record.size >= LARGE_INSTALLER_OR_ARCHIVE_BYTES
    {
        return Some("Large installer or archive that can be offloaded".to_string());
    }
    if matches!(
        extension.as_str(),
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "webm" | "jpg" | "jpeg" | "png" | "heic"
    ) && record.size >= LARGE_FILE_BYTES
    {
        return Some("Large media file that can be offloaded".to_string());
    }
    if record.size >= STALE_LARGE_FILE_BYTES && age_days.unwrap_or(0) >= STALE_FILE_DAYS {
        return Some("Storage hog: large file untouched for 6+ months".to_string());
    }
    if record.size >= LARGE_FILE_BYTES {
        return Some("Large file that can be offloaded".to_string());
    }
    None
}

fn cleanup_risk(risk: CleanupRisk) -> RecommendationRisk {
    match risk {
        CleanupRisk::Low => RecommendationRisk::Low,
        CleanupRisk::Review => RecommendationRisk::Review,
    }
}

fn age_days(modified_at_ms: Option<i64>) -> Option<u64> {
    let modified_at_ms = modified_at_ms?;
    let now_ms = unix_millis();
    if now_ms <= modified_at_ms {
        return Some(0);
    }
    Some(((now_ms - modified_at_ms) as u64) / 86_400_000)
}

fn sum_kind(candidates: &[RecommendationCandidate], kind: RecommendationKind) -> u64 {
    candidates
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .map(|candidate| candidate.size)
        .sum()
}

fn kind_order(kind: &RecommendationKind) -> u8 {
    match kind {
        RecommendationKind::Clean => 0,
        RecommendationKind::Offload => 1,
        RecommendationKind::Review => 2,
    }
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
