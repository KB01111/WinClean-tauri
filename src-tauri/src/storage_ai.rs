//! The deterministic half of AI-assisted storage cleanup.
//!
//! The assistant only ever *talks to this module's output*:
//!
//! ```text
//! request ─▶ native index ─▶ (optional Windows search) ─▶ candidate correlation
//!   ─▶ policy validation ─▶ [AI explains / ranks] ─▶ structured proposal
//!   ─▶ validate_proposals() ─▶ user review ─▶ native execution ─▶ verification
//! ```
//!
//! Nothing a model says is trusted: a proposal carries only candidate *ids*; each
//! id is resolved against the candidates WinClean itself computed from the scan,
//! the action must match the candidate's own kind, sizes are recomputed here, and
//! each file is checked against the disk again. A model never supplies a path,
//! and its confidence is informational only. Execution happens elsewhere
//! (`cleanup`, `offload`), which re-validates every file once more.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    cleanup::cleanup_candidate_id,
    db::{Database, FileSearchResult},
    pathutil::is_within,
    recommendations::{build_recommendations, RecommendationCandidate, RecommendationFilters, RecommendationKind, RecommendationRisk, RecommendedAction},
    storage_policy::{BlockReason, StoragePolicy},
};

pub const MAX_RESULTS: usize = 250;
pub const MAX_PROPOSALS: usize = 12;
pub const MAX_IDS_PER_PROPOSAL: usize = 500;
pub const STALE_SCAN_DAYS: u64 = 3;
const BLOCKED_SAMPLE: usize = 40;
const BLOCKED_SCAN_ROWS: u32 = 1000;
const DEFAULT_BLOCKED_MIN_SIZE: u64 = 10 * 1024 * 1024;
const DAY_MS: i64 = 86_400_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

fn mb(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    }
}

/// Model- or MCP-supplied text: one line, no control or direction-override characters, bounded.
pub fn clean_text(s: &str, max: usize) -> String {
    let flattened: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !matches!(*c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFEFF))
        .collect();
    flattened.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageQuery {
    /// `clean`, `offload` or `review`; anything else means all.
    pub mode: Option<String>,
    pub min_size: Option<u64>,
    pub min_age_days: Option<u64>,
    /// Case-insensitive text that must appear in the file name or path.
    pub name_contains: Option<String>,
    /// Only files inside this folder.
    pub folder: Option<String>,
    pub extension: Option<String>,
    /// "Free up about this much": a deterministic selection is computed.
    pub goal_bytes: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedItem {
    /// Candidate id; the only handle a proposal may use.
    pub id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub age_days: Option<u64>,
    pub kind: RecommendationKind,
    pub action: RecommendedAction,
    pub risk: RecommendationRisk,
    pub reason: String,
    /// What the deterministic rules saw, in plain words.
    pub evidence: Vec<String>,
    pub duplicate_group: Option<String>,
    pub duplicate_copies: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedItem {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub reason: BlockReason,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalSelection {
    pub goal_bytes: u64,
    pub selected_ids: Vec<String>,
    pub selected_bytes: u64,
    pub shortfall_bytes: u64,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageEvaluation {
    pub scan_run_id: i64,
    pub scan_root: String,
    pub scan_age_days: Option<u64>,
    pub scan_stale: bool,
    /// Clean and offload candidates: WinClean's own rules allow an action.
    pub verified: Vec<EvaluatedItem>,
    /// Files that need a person's judgment; no automatic action is offered.
    pub manual_review: Vec<EvaluatedItem>,
    /// Matching files WinClean refuses to touch, with the reason.
    pub blocked: Vec<BlockedItem>,
    pub blocked_total: u32,
    pub clean_bytes: u64,
    pub offload_bytes: u64,
    pub review_bytes: u64,
    pub goal: Option<GoalSelection>,
    pub warnings: Vec<String>,
}

fn age_days_of(ms: Option<i64>) -> Option<u64> {
    ms.map(|m| (now_ms().saturating_sub(m).max(0) / DAY_MS) as u64)
}

fn evidence(c: &RecommendationCandidate) -> Vec<String> {
    let mut out = vec![c.reason.clone(), format!("Size {}", mb(c.size))];
    if let Some(days) = c.age_days {
        out.push(format!("Not modified for {days} day(s)"));
    }
    match c.recommended_action {
        RecommendedAction::RecycleBin => out.push("Low risk: moved to the Recycle Bin, restorable until it is emptied".to_string()),
        RecommendedAction::GoogleDriveOffload => out.push("Offload: uploaded to Google Drive and verified before the local copy is recycled".to_string()),
        RecommendedAction::Review => out.push("Needs your judgment: WinClean offers no automatic action".to_string()),
    }
    if let (Some(group), Some(copies)) = (&c.duplicate_group, c.duplicate_copies) {
        out.push(format!("Contents verified identical across {copies} files (fingerprint {group})"));
    }
    out
}

fn to_item(c: &RecommendationCandidate) -> EvaluatedItem {
    EvaluatedItem {
        id: c.id.clone(),
        path: c.path.clone(),
        name: c.name.clone(),
        size: c.size,
        age_days: c.age_days,
        kind: c.kind.clone(),
        action: c.recommended_action.clone(),
        risk: c.risk_level.clone(),
        reason: c.reason.clone(),
        evidence: evidence(c),
        duplicate_group: c.duplicate_group.clone(),
        duplicate_copies: c.duplicate_copies,
    }
}

fn matches_query(path: &str, name: &str, extension: Option<&str>, q: &StorageQuery) -> bool {
    if let Some(needle) = q.name_contains.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        let n = needle.to_lowercase();
        if !name.to_lowercase().contains(&n) && !path.to_lowercase().contains(&n) {
            return false;
        }
    }
    if let Some(folder) = q.folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        if !is_within(folder, path) {
            return false;
        }
    }
    if let Some(ext) = q.extension.as_deref().map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()) {
        if extension.map(str::to_lowercase).as_deref() != Some(ext.as_str()) {
            return false;
        }
    }
    true
}

/// Lowest-risk, biggest-win first: clean candidates, then offload candidates,
/// until the goal is met. Review items are never part of a goal.
pub fn select_for_goal(items: &[EvaluatedItem], goal_bytes: u64) -> GoalSelection {
    let mut ordered: Vec<&EvaluatedItem> = items.iter().filter(|i| matches!(i.kind, RecommendationKind::Clean | RecommendationKind::Offload)).collect();
    ordered.sort_by(|a, b| {
        let rank = |i: &EvaluatedItem| if i.kind == RecommendationKind::Clean { 0 } else { 1 };
        rank(a).cmp(&rank(b)).then_with(|| b.size.cmp(&a.size)).then_with(|| a.id.cmp(&b.id))
    });
    let mut selected = Vec::new();
    let mut total = 0u64;
    for item in ordered {
        if total >= goal_bytes {
            break;
        }
        total += item.size;
        selected.push(item.id.clone());
    }
    let shortfall = goal_bytes.saturating_sub(total);
    let note = if shortfall == 0 {
        format!("{} item(s), {} — enough for the goal of {}", selected.len(), mb(total), mb(goal_bytes))
    } else {
        format!("All safe candidates together give {}, {} short of {}", mb(total), mb(shortfall), mb(goal_bytes))
    };
    GoalSelection { goal_bytes, selected_ids: selected, selected_bytes: total, shortfall_bytes: shortfall, note }
}

pub fn evaluate(db: &Database, run_id: i64, query: &StorageQuery) -> anyhow::Result<StorageEvaluation> {
    let run = db.scan_run(run_id)?.ok_or_else(|| anyhow::anyhow!("scan {run_id} does not exist"))?;
    let plan = build_recommendations(
        db,
        run_id,
        RecommendationFilters { mode: query.mode.clone(), min_size: query.min_size, min_age_days: query.min_age_days, only_offload_eligible: None },
    )?;
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_RESULTS);

    let mut verified = Vec::new();
    let mut manual = Vec::new();
    let (mut clean_bytes, mut offload_bytes, mut review_bytes) = (0u64, 0u64, 0u64);
    for c in plan.candidates.iter().filter(|c| matches_query(&c.path, &c.name, ext_of(&c.name), query)) {
        let item = to_item(c);
        match c.kind {
            RecommendationKind::Clean => {
                clean_bytes += c.size;
                verified.push(item);
            }
            RecommendationKind::Offload => {
                offload_bytes += c.size;
                verified.push(item);
            }
            RecommendationKind::Review => {
                review_bytes += c.size;
                manual.push(item);
            }
        }
    }
    let goal = query.goal_bytes.filter(|g| *g > 0).map(|g| select_for_goal(&verified, g));
    verified.truncate(limit);
    manual.truncate(limit);

    let (blocked, blocked_total) = blocked_matches(db, run_id, query, &plan.candidates)?;

    let scan_age_days = age_days_of(run.finished_at_ms);
    let scan_stale = scan_age_days.map(|d| d >= STALE_SCAN_DAYS).unwrap_or(true);
    let mut warnings = plan.warnings.clone();
    if scan_stale {
        warnings.push(match scan_age_days {
            Some(d) => format!("This scan is {d} day(s) old. Files are re-checked before anything happens, but a fresh scan finds more."),
            None => "This scan has no completion time; run a fresh scan.".to_string(),
        });
    }
    Ok(StorageEvaluation {
        scan_run_id: run_id,
        scan_root: run.root_path,
        scan_age_days,
        scan_stale,
        verified,
        manual_review: manual,
        blocked,
        blocked_total,
        clean_bytes,
        offload_bytes,
        review_bytes,
        goal,
        warnings,
    })
}

fn ext_of(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, e)| e)
}

/// Large files that match the query but that policy keeps out, with the reason.
fn blocked_matches(db: &Database, run_id: i64, query: &StorageQuery, candidates: &[RecommendationCandidate]) -> anyhow::Result<(Vec<BlockedItem>, u32)> {
    let policy = StoragePolicy::load(db, run_id)?;
    let in_plan: HashSet<i64> = candidates.iter().map(|c| c.entry_id).collect();
    let min = query.min_size.unwrap_or(DEFAULT_BLOCKED_MIN_SIZE);
    let rows = db.file_search(run_id, query.name_contains.as_deref().map(str::trim).filter(|s| !s.is_empty()), query.extension.as_deref(), Some(min), BLOCKED_SCAN_ROWS)?;
    let mut out = Vec::new();
    let mut total = 0u32;
    for row in rows {
        if in_plan.contains(&row.id) || !matches_query(&row.path, &row.name, row.extension.as_deref(), query) {
            continue;
        }
        if let Some(b) = policy.check_entry(&row, false) {
            total += 1;
            if out.len() < BLOCKED_SAMPLE {
                out.push(BlockedItem { path: row.path, name: row.name, size: row.size, reason: b.reason, detail: b.detail });
            }
        }
    }
    Ok((out, total))
}

// ---------------------------------------------------------------------------
// Correlation (search results ↔ the scan index)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrelationStatus {
    /// WinClean's rules allow an action; `candidate_id` is set.
    Candidate,
    /// Eligible only for a person's judgment.
    ManualReview,
    /// Policy refuses to touch it.
    Blocked,
    /// In the scan, but no rule recommends doing anything.
    NotRecommended,
    /// Not in this scan, so it can be neither validated nor acted on.
    NotInIndex,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Correlated {
    pub path: String,
    pub name: String,
    pub size: Option<u64>,
    pub age_days: Option<u64>,
    pub status: CorrelationStatus,
    pub candidate_id: Option<String>,
    pub action: Option<RecommendedAction>,
    pub detail: String,
}

struct Correlator {
    policy: StoragePolicy,
    by_entry: HashMap<i64, RecommendationCandidate>,
}

impl Correlator {
    fn load(db: &Database, run_id: i64) -> anyhow::Result<Self> {
        let plan = build_recommendations(db, run_id, RecommendationFilters::default())?;
        Ok(Self { policy: StoragePolicy::load(db, run_id)?, by_entry: plan.candidates.into_iter().map(|c| (c.entry_id, c)).collect() })
    }

    fn classify(&self, entry: &FileSearchResult) -> Correlated {
        let base = |status, id: Option<String>, action, detail: String| Correlated {
            path: entry.path.clone(),
            name: entry.name.clone(),
            size: Some(entry.size),
            age_days: age_days_of(entry.modified_at_ms),
            status,
            candidate_id: id,
            action,
            detail,
        };
        if let Some(c) = self.by_entry.get(&entry.id) {
            let status = if c.kind == RecommendationKind::Review { CorrelationStatus::ManualReview } else { CorrelationStatus::Candidate };
            return base(status, Some(c.id.clone()), Some(c.recommended_action.clone()), c.reason.clone());
        }
        match self.policy.check_entry(entry, false) {
            Some(b) => base(CorrelationStatus::Blocked, None, None, b.detail),
            None => base(CorrelationStatus::NotRecommended, None, None, "No cleanup rule recommends this file".to_string()),
        }
    }
}

/// Native filename/metadata search of the scan index, each hit classified.
pub fn native_search(db: &Database, run_id: i64, query: &StorageQuery) -> anyhow::Result<Vec<Correlated>> {
    let correlator = Correlator::load(db, run_id)?;
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_RESULTS);
    let rows = db.file_search(run_id, query.name_contains.as_deref().map(str::trim).filter(|s| !s.is_empty()), query.extension.as_deref(), query.min_size, 1000)?;
    Ok(rows
        .iter()
        .filter(|r| matches_query(&r.path, &r.name, r.extension.as_deref(), query))
        .filter(|r| query.min_age_days.map(|d| age_days_of(r.modified_at_ms).unwrap_or(0) >= d).unwrap_or(true))
        .take(limit)
        .map(|r| correlator.classify(r))
        .collect())
}

/// Classify paths reported by an external search (Windows MCP). The paths are
/// untrusted: they are only *looked up* in the scan index, and an action is
/// possible only for a path the scan indexed and WinClean's own rules accept.
pub fn correlate_paths(db: &Database, run_id: i64, paths: &[String]) -> anyhow::Result<Vec<Correlated>> {
    let correlator = Correlator::load(db, run_id)?;
    let mut out = Vec::with_capacity(paths.len().min(MAX_RESULTS));
    for path in paths.iter().take(MAX_RESULTS) {
        out.push(match db.file_entry_by_path(run_id, path)? {
            Some(entry) => correlator.classify(&entry),
            None => Correlated {
                path: path.clone(),
                name: path.rsplit(['\\', '/']).next().unwrap_or(path).to_string(),
                size: None,
                age_days: None,
                status: CorrelationStatus::NotInIndex,
                candidate_id: None,
                action: None,
                detail: "Not part of the selected scan, so it cannot be cleaned up from here. Scan this location first.".to_string(),
            },
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalInput {
    pub title: String,
    pub action: String,
    pub candidate_ids: Vec<String>,
    pub reason: String,
    /// What the model believes it adds up to. Compared, never used.
    pub estimated_bytes: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProposalAction {
    Clean,
    Offload,
    Review,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejected {
    pub candidate_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionPreview {
    pub summary: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub destination: String,
    /// WinClean has no permanent-delete action; always false.
    pub permanent_delete: bool,
    pub reversible: bool,
    pub requires_google_drive: bool,
    pub steps: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedProposal {
    pub title: String,
    pub reason: String,
    pub action: Option<ProposalAction>,
    pub items: Vec<EvaluatedItem>,
    pub rejected: Vec<Rejected>,
    pub verified_bytes: u64,
    /// The model's own estimate, when it gave one.
    pub claimed_bytes: Option<u64>,
    /// False when the estimate was wrong; the verified figure is the one to trust.
    pub claim_matches: bool,
    pub preview: ActionPreview,
}

fn parse_candidate_id(id: &str, run_id: i64) -> Result<i64, String> {
    let rest = id.strip_prefix("scan-").ok_or("not a WinClean candidate id")?;
    let (run, entry) = rest.split_once(":entry-").ok_or("not a WinClean candidate id")?;
    let run: i64 = run.parse().map_err(|_| "not a WinClean candidate id".to_string())?;
    let entry: i64 = entry.parse().map_err(|_| "not a WinClean candidate id".to_string())?;
    if run != run_id {
        return Err(format!("belongs to scan {run}, not scan {run_id}"));
    }
    if cleanup_candidate_id(run, entry) != id {
        return Err("not a WinClean candidate id".to_string());
    }
    Ok(entry)
}

fn parse_action(s: &str) -> Option<ProposalAction> {
    match s.trim().to_ascii_lowercase().as_str() {
        "clean" => Some(ProposalAction::Clean),
        "offload" => Some(ProposalAction::Offload),
        "review" => Some(ProposalAction::Review),
        _ => None,
    }
}

fn kind_label(k: &RecommendationKind) -> &'static str {
    match k {
        RecommendationKind::Clean => "a clean candidate",
        RecommendationKind::Offload => "an offload candidate",
        RecommendationKind::Review => "a review-only candidate",
    }
}

fn preview(action: Option<ProposalAction>, items: &[EvaluatedItem]) -> ActionPreview {
    let total: u64 = items.iter().map(|i| i.size).sum();
    let n = items.len();
    let (summary, destination, steps, notes, reversible, drive) = match action {
        Some(ProposalAction::Clean) => (
            format!("Move {n} file(s) ({}) to the Windows Recycle Bin", mb(total)),
            "Windows Recycle Bin".to_string(),
            vec![
                "Each file is checked again right before it is moved: same size and date as scanned, still a regular file, not a link, cloud placeholder or protected location".to_string(),
                "Files that no longer match are skipped, never forced".to_string(),
            ],
            vec!["Disk space is only freed once the Recycle Bin is emptied".to_string(), "Nothing is permanently deleted".to_string()],
            true,
            false,
        ),
        Some(ProposalAction::Offload) => (
            format!("Upload {n} file(s) ({}) to Google Drive, verify them, then move the local copies to the Recycle Bin", mb(total)),
            "Google Drive, then the Recycle Bin".to_string(),
            vec![
                "Each file is uploaded and its checksum compared with Google Drive's".to_string(),
                "The local file is recycled only after the upload is verified".to_string(),
                "Offloaded files can be restored from the history view".to_string(),
            ],
            vec!["Requires a connected Google Drive account".to_string(), "Nothing is permanently deleted".to_string()],
            true,
            true,
        ),
        Some(ProposalAction::Review) => (
            format!("{n} file(s) ({}) for you to look at yourself", mb(total)),
            "None".to_string(),
            vec!["WinClean offers no automatic action for these; open the location and decide".to_string()],
            vec!["Confirmed duplicates: keep at least one copy".to_string()],
            true,
            false,
        ),
        None => ("Nothing to do".to_string(), "None".to_string(), Vec::new(), vec!["The proposal's action was not recognised".to_string()], true, false),
    };
    ActionPreview { summary, file_count: n, total_bytes: total, destination, permanent_delete: false, reversible, requires_google_drive: drive, steps, notes }
}

/// Resolve and check model proposals against the candidates WinClean computed.
///
/// `still_valid` re-checks one item against the disk (the command passes the
/// real file check; tests pass a stub). An item that fails it is rejected with
/// its reason rather than listed.
pub fn validate_proposals(
    db: &Database,
    run_id: i64,
    inputs: &[ProposalInput],
    still_valid: &dyn Fn(&EvaluatedItem) -> Option<String>,
) -> anyhow::Result<Vec<ValidatedProposal>> {
    let plan = build_recommendations(db, run_id, RecommendationFilters::default())?;
    let by_entry: HashMap<i64, &RecommendationCandidate> = plan.candidates.iter().map(|c| (c.entry_id, c)).collect();
    let mut claimed_elsewhere: HashSet<i64> = HashSet::new();
    let mut out = Vec::new();

    for input in inputs.iter().take(MAX_PROPOSALS) {
        let action = parse_action(&input.action);
        let mut items: Vec<EvaluatedItem> = Vec::new();
        let mut rejected: Vec<Rejected> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::new();

        for raw in input.candidate_ids.iter().take(MAX_IDS_PER_PROPOSAL) {
            let shown = clean_text(raw, 80);
            let reject = |reason: String, rejected: &mut Vec<Rejected>| rejected.push(Rejected { candidate_id: shown.clone(), reason });
            let Some(action) = action else {
                reject("the proposal's action is not clean, offload or review".to_string(), &mut rejected);
                continue;
            };
            let entry_id = match parse_candidate_id(raw, run_id) {
                Ok(e) => e,
                Err(why) => {
                    reject(why, &mut rejected);
                    continue;
                }
            };
            if !seen.insert(entry_id) {
                continue; // repeated id: counted once
            }
            let Some(candidate) = by_entry.get(&entry_id) else {
                reject("not a current candidate (it may have been excluded, protected or changed)".to_string(), &mut rejected);
                continue;
            };
            let fits = match action {
                ProposalAction::Clean => candidate.kind == RecommendationKind::Clean && candidate.recommended_action == RecommendedAction::RecycleBin,
                ProposalAction::Offload => candidate.kind == RecommendationKind::Offload && candidate.eligible_for_offload && candidate.recommended_action == RecommendedAction::GoogleDriveOffload,
                ProposalAction::Review => candidate.kind == RecommendationKind::Review,
            };
            if !fits {
                reject(format!("proposed as {:?} but WinClean sees {}", action, kind_label(&candidate.kind)).to_lowercase(), &mut rejected);
                continue;
            }
            if claimed_elsewhere.contains(&entry_id) {
                reject("already used by an earlier proposal".to_string(), &mut rejected);
                continue;
            }
            let item = to_item(candidate);
            if let Some(problem) = still_valid(&item) {
                reject(problem, &mut rejected);
                continue;
            }
            claimed_elsewhere.insert(entry_id);
            items.push(item);
        }
        if input.candidate_ids.len() > MAX_IDS_PER_PROPOSAL {
            rejected.push(Rejected { candidate_id: String::new(), reason: format!("only the first {MAX_IDS_PER_PROPOSAL} ids were considered") });
        }

        let verified_bytes: u64 = items.iter().map(|i| i.size).sum();
        let claimed_bytes = input.estimated_bytes.filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(u64::MAX as f64) as u64);
        let claim_matches = claimed_bytes.map(|c| c == verified_bytes).unwrap_or(false);
        let p = preview(action, &items);
        out.push(ValidatedProposal {
            title: clean_text(&input.title, 120),
            reason: clean_text(&input.reason, 500),
            action,
            items,
            rejected,
            verified_bytes,
            claimed_bytes,
            claim_matches,
            preview: p,
        });
    }
    Ok(out)
}

/// The real "is this file still what the scan saw" check used before a proposal is shown.
pub fn disk_check(item: &EvaluatedItem, modified_at_ms: Option<i64>) -> Option<String> {
    match crate::cleanup::validate_file_for_action(&item.path, item.size, modified_at_ms) {
        Ok(_) => None,
        Err(e) => Some(format!("changed or unavailable since the scan: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_storage::testing::*;

    const MIB: u64 = 1024 * 1024;

    fn sample() -> (tempfile::TempDir, Database, i64) {
        seeded(vec![
            dir("C:\\Users\\me\\AppData\\Local\\Temp"),
            file("C:\\Users\\me\\AppData\\Local\\Temp\\old.tmp", 30 * MIB, 40),
            file("C:\\Users\\me\\Downloads\\setup.msi", 120 * MIB, 90),
            file("C:\\Users\\me\\Downloads\\notes.docx", 2 * MIB, 90),
            file("C:\\Users\\me\\Videos\\holiday.mp4", 400 * MIB, 200),
            file("C:\\Users\\me\\code\\app\\package.json", 1000, 1),
            file("C:\\Users\\me\\code\\app\\data\\dump.bin", 500 * MIB, 300),
            file("C:\\Users\\me\\OneDrive\\big.iso", 900 * MIB, 300),
            file("C:\\Users\\me\\Pictures\\vm.vhdx", 800 * MIB, 300),
        ])
    }

    fn all_valid(_: &EvaluatedItem) -> Option<String> {
        None
    }

    fn input(action: &str, ids: Vec<String>, est: Option<f64>) -> ProposalInput {
        ProposalInput { title: "t".into(), action: action.into(), candidate_ids: ids, reason: "r".into(), estimated_bytes: est }
    }

    #[test]
    fn evaluation_separates_verified_manual_and_blocked_files_with_reasons() {
        let (_d, db, run) = sample();
        let ev = evaluate(&db, run, &StorageQuery::default()).unwrap();
        let verified: Vec<_> = ev.verified.iter().map(|i| i.name.as_str()).collect();
        assert!(verified.contains(&"old.tmp") && verified.contains(&"holiday.mp4") && verified.contains(&"setup.msi"), "{verified:?}");
        for n in ["dump.bin", "big.iso", "vm.vhdx"] {
            assert!(!verified.contains(&n) && !ev.manual_review.iter().any(|i| i.name == n), "{n} must never be a candidate");
        }
        let blocked: HashMap<_, _> = ev.blocked.iter().map(|b| (b.name.as_str(), b.reason)).collect();
        assert_eq!(blocked["dump.bin"], BlockReason::DevelopmentProject);
        assert_eq!(blocked["big.iso"], BlockReason::SyncedFolder);
        assert_eq!(blocked["vm.vhdx"], BlockReason::ProtectedFileType);
        assert!(ev.blocked.iter().all(|b| !b.detail.is_empty()));
        assert!(ev.verified.iter().all(|i| !i.evidence.is_empty()));
    }

    #[test]
    fn user_exclusions_remove_candidates_and_appear_as_blocked() {
        let (_d, db, run) = sample();
        crate::storage_policy::add_exclusion(&db, "C:\\Users\\me\\Videos").unwrap();
        let ev = evaluate(&db, run, &StorageQuery::default()).unwrap();
        assert!(!ev.verified.iter().any(|i| i.name == "holiday.mp4"));
        let b = ev.blocked.iter().find(|b| b.name == "holiday.mp4").unwrap();
        assert_eq!(b.reason, BlockReason::UserExcluded);
    }

    #[test]
    fn query_filters_by_mode_size_age_folder_and_name() {
        let (_d, db, run) = sample();
        let q = |f: &dyn Fn(&mut StorageQuery)| {
            let mut q = StorageQuery::default();
            f(&mut q);
            evaluate(&db, run, &q).unwrap().verified.iter().map(|i| i.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(q(&|q| q.mode = Some("clean".into())), vec!["old.tmp"]);
        assert!(q(&|q| q.min_size = Some(300 * MIB)).iter().all(|n| n == "holiday.mp4"));
        assert_eq!(q(&|q| q.folder = Some("C:\\Users\\me\\Downloads".into())), vec!["setup.msi"]);
        assert_eq!(q(&|q| q.name_contains = Some("HOLIDAY".into())), vec!["holiday.mp4"]);
        assert!(q(&|q| q.min_age_days = Some(150)).iter().all(|n| n == "holiday.mp4"));
    }

    #[test]
    fn a_goal_is_met_with_the_lowest_risk_items_first_and_a_shortfall_is_honest() {
        let (_d, db, run) = sample();
        let ev = evaluate(&db, run, &StorageQuery { goal_bytes: Some(20 * MIB), ..Default::default() }).unwrap();
        let g = ev.goal.unwrap();
        assert_eq!(g.selected_ids.len(), 1, "the temp file alone is enough");
        assert!(ev.verified.iter().any(|i| i.id == g.selected_ids[0] && i.name == "old.tmp"));
        assert_eq!(g.shortfall_bytes, 0);

        let big = evaluate(&db, run, &StorageQuery { goal_bytes: Some(100_000 * MIB), ..Default::default() }).unwrap().goal.unwrap();
        assert!(big.shortfall_bytes > 0 && big.selected_bytes < 100_000 * MIB);
        assert!(big.note.contains("short"));
    }

    #[test]
    fn a_proposal_is_resolved_from_ids_and_its_size_claim_is_recomputed() {
        let (_d, db, run) = sample();
        let ev = evaluate(&db, run, &StorageQuery::default()).unwrap();
        let tmp = ev.verified.iter().find(|i| i.name == "old.tmp").unwrap();
        let v = validate_proposals(&db, run, &[input("clean", vec![tmp.id.clone()], Some(999_999_999_999.0))], &all_valid).unwrap();
        assert_eq!(v[0].items.len(), 1);
        assert_eq!(v[0].verified_bytes, 30 * MIB);
        assert_eq!(v[0].claimed_bytes, Some(999_999_999_999));
        assert!(!v[0].claim_matches, "an inflated estimate is flagged, not believed");
        assert!(!v[0].preview.permanent_delete && v[0].preview.reversible);
        assert!(v[0].preview.summary.contains("Recycle Bin"));
        let ok = validate_proposals(&db, run, &[input("clean", vec![tmp.id.clone()], Some((30 * MIB) as f64))], &all_valid).unwrap();
        assert!(ok[0].claim_matches);
    }

    #[test]
    fn model_supplied_paths_unknown_and_foreign_ids_are_rejected_with_reasons() {
        let (_d, db, run) = sample();
        let blocked = db.file_entry_by_path(run, "C:\\Users\\me\\Pictures\\vm.vhdx").unwrap().unwrap();
        let ids = vec![
            "C:\\Windows\\System32\\config\\SAM".to_string(),
            "scan-999:entry-1".to_string(),
            "scan-1:entry-99999".to_string(),
            cleanup_candidate_id(run, blocked.id), // a real file, but protected
            "scan-1:entry-1; DROP".to_string(),
            "".to_string(),
        ];
        let v = validate_proposals(&db, run, &[input("clean", ids, None)], &all_valid).unwrap();
        assert!(v[0].items.is_empty());
        assert_eq!(v[0].rejected.len(), 6);
        assert!(v[0].rejected.iter().all(|r| !r.reason.is_empty()));
    }

    #[test]
    fn the_action_must_match_what_winclean_decided_for_the_file() {
        let (_d, db, run) = sample();
        let ev = evaluate(&db, run, &StorageQuery::default()).unwrap();
        let find = |n: &str| ev.verified.iter().find(|i| i.name == n).unwrap().id.clone();
        let (tmp, video) = (find("old.tmp"), find("holiday.mp4"));
        // Offload candidates cannot be recycled directly, and clean candidates cannot be offloaded.
        let a = validate_proposals(&db, run, &[input("clean", vec![video.clone()], None)], &all_valid).unwrap();
        assert!(a[0].items.is_empty() && a[0].rejected[0].reason.contains("offload"), "{:?}", a[0].rejected);
        let b = validate_proposals(&db, run, &[input("offload", vec![tmp], None)], &all_valid).unwrap();
        assert!(b[0].items.is_empty());
        let c = validate_proposals(&db, run, &[input("offload", vec![video], None)], &all_valid).unwrap();
        assert_eq!(c[0].items.len(), 1);
        assert!(c[0].preview.requires_google_drive);
        let d = validate_proposals(&db, run, &[input("delete everything", vec!["scan-1:entry-1".into()], None)], &all_valid).unwrap();
        assert!(d[0].action.is_none() && d[0].items.is_empty());
    }

    #[test]
    fn stale_files_are_rejected_at_review_time_and_ids_are_used_once() {
        let (_d, db, run) = sample();
        let ev = evaluate(&db, run, &StorageQuery::default()).unwrap();
        let tmp = ev.verified.iter().find(|i| i.name == "old.tmp").unwrap().id.clone();
        let changed = |i: &EvaluatedItem| (i.name == "old.tmp").then(|| "changed or unavailable since the scan".to_string());
        let v = validate_proposals(&db, run, &[input("clean", vec![tmp.clone()], None)], &changed).unwrap();
        assert!(v[0].items.is_empty() && v[0].rejected[0].reason.contains("changed"));
        let twice = validate_proposals(&db, run, &[input("clean", vec![tmp.clone(), tmp.clone()], None), input("clean", vec![tmp], None)], &all_valid).unwrap();
        assert_eq!(twice[0].items.len(), 1, "a repeated id counts once");
        assert!(twice[1].items.is_empty() && twice[1].rejected[0].reason.contains("earlier"), "{:?}", twice[1].rejected);
    }

    #[test]
    fn proposal_text_cannot_smuggle_control_characters_or_unbounded_length() {
        let (_d, db, run) = sample();
        let mut p = input("clean", vec![], None);
        p.title = format!("Free\u{202E} space\n\n{}", "x".repeat(1000));
        p.reason = "line1\u{0007}\nline2".into();
        let v = validate_proposals(&db, run, &[p], &all_valid).unwrap();
        assert!(v[0].title.chars().count() <= 120 && !v[0].title.contains('\u{202E}') && !v[0].title.contains('\n'));
        assert_eq!(v[0].reason, "line1 line2");
    }

    #[test]
    fn proposal_and_id_counts_are_bounded() {
        let (_d, db, run) = sample();
        let many: Vec<ProposalInput> = (0..40).map(|_| input("review", vec![], None)).collect();
        assert_eq!(validate_proposals(&db, run, &many, &all_valid).unwrap().len(), MAX_PROPOSALS);
        let ids: Vec<String> = (0..2000).map(|n| format!("scan-{run}:entry-{n}")).collect();
        let v = validate_proposals(&db, run, &[input("clean", ids, None)], &all_valid).unwrap();
        assert!(v[0].rejected.len() <= MAX_IDS_PER_PROPOSAL + 1);
    }

    #[test]
    fn external_search_hits_are_classified_against_the_index_only() {
        let (_d, db, run) = sample();
        let hits = vec![
            "C:\\Users\\me\\AppData\\Local\\Temp\\old.tmp".to_string(),
            "c:/users/me/videos/HOLIDAY.mp4".to_string(),
            "C:\\Users\\me\\Pictures\\vm.vhdx".to_string(),
            "C:\\Users\\me\\Downloads\\notes.docx".to_string(),
            "D:\\never\\scanned.bin".to_string(),
        ];
        let out = correlate_paths(&db, run, &hits).unwrap();
        let status = |n: &str| out.iter().find(|c| c.name.eq_ignore_ascii_case(n)).unwrap();
        assert_eq!(status("old.tmp").status, CorrelationStatus::Candidate);
        assert!(status("old.tmp").candidate_id.is_some());
        assert_eq!(status("holiday.mp4").status, CorrelationStatus::Candidate);
        assert_eq!(status("vm.vhdx").status, CorrelationStatus::Blocked);
        assert!(status("vm.vhdx").candidate_id.is_none());
        assert_eq!(status("notes.docx").status, CorrelationStatus::Blocked, "documents are protected unless an existing rule offers them");
        assert_eq!(status("scanned.bin").status, CorrelationStatus::NotInIndex);
        assert!(status("scanned.bin").candidate_id.is_none(), "an unindexed path can never carry an action");
    }

    #[test]
    fn native_search_is_the_fallback_and_classifies_every_hit() {
        let (_d, db, run) = sample();
        let hits = native_search(&db, run, &StorageQuery { name_contains: Some("vhdx".into()), ..Default::default() }).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].status, CorrelationStatus::Blocked);
        let big = native_search(&db, run, &StorageQuery { min_size: Some(300 * MIB), limit: Some(2), ..Default::default() }).unwrap();
        assert_eq!(big.len(), 2);
        assert!(big[0].size >= big[1].size);
    }

    #[test]
    fn clean_text_flattens_and_bounds() {
        assert_eq!(clean_text("a\u{0000}b \u{200B}c\r\nd", 20), "ab c d");
        assert_eq!(clean_text("abcdef", 3), "abc");
    }
}
