//! The storage workflow on real files: scan a directory, let WinClean's rules pick candidates,
//! validate a model-style proposal against the real disk, and watch it be refused once a file changes.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use winclean_lib::{
    db::Database,
    scan::run_recursive_scan,
    storage_ai::{disk_check, evaluate, validate_proposals, EvaluatedItem, ProposalInput, StorageQuery},
};

/// Windows keeps cleanup candidates out of %TEMP% (it is inside AppData), so the fixture lives in C:\tmp there.
fn fixture_base() -> PathBuf {
    if cfg!(windows) {
        let base = PathBuf::from("C:\\tmp");
        fs::create_dir_all(&base).expect("C:\\tmp");
        base
    } else {
        std::env::temp_dir()
    }
}

fn write_aged(path: &Path, bytes: usize, age_days: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7_u8; bytes]).unwrap();
    let when = SystemTime::now() - Duration::from_secs(age_days * 86_400);
    fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(when).unwrap();
}

fn proposal(ids: &[String]) -> ProposalInput {
    ProposalInput { title: "Old temporary files".into(), action: "clean".into(), candidate_ids: ids.to_vec(), reason: "test".into(), estimated_bytes: None }
}

#[test]
fn scan_to_validated_proposal_on_real_files() {
    let temp = tempfile::Builder::new().prefix("winclean-pipeline-").tempdir_in(fixture_base()).unwrap();
    let root = temp.path().join("disk");
    let old = root.join("Documents").join("old-session.tmp");
    let recent = root.join("Documents").join("recent.tmp");
    let in_project_marker = root.join("code").join("app").join("package.json");
    let in_project_file = root.join("code").join("app").join("cache").join("build.tmp");
    write_aged(&old, 4096, 40);
    write_aged(&recent, 4096, 0);
    write_aged(&in_project_marker, 20, 40);
    write_aged(&in_project_file, 4096, 40);

    let db = Database::open(temp.path().join("index.sqlite3")).unwrap();
    db.migrate().unwrap();
    let summary = run_recursive_scan(&db, &root, None).expect("scan");
    assert_eq!(summary.files_indexed, 4);

    // 1. WinClean's rules: only the old, unprotected temp file is a clean candidate.
    let evaluation = evaluate(&db, summary.run_id, &StorageQuery { min_size: Some(1), ..Default::default() }).unwrap();
    let names: Vec<&str> = evaluation.verified.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["old-session.tmp"], "recent files and files inside a code project are not candidates");
    assert!(
        evaluation.blocked.iter().any(|b| b.name == "build.tmp"),
        "the project file is listed as kept out: {:?}",
        evaluation.blocked.iter().map(|b| &b.name).collect::<Vec<_>>()
    );
    let id = evaluation.verified[0].id.clone();

    // 2. A model proposes the real id plus an invented one; only the real file survives, re-sized by WinClean.
    let check = |item: &EvaluatedItem| {
        let modified = db.file_entry_by_path(summary.run_id, &item.path).ok().flatten().and_then(|e| e.modified_at_ms);
        disk_check(item, modified)
    };
    let invented = format!("scan-{}:entry-99999", summary.run_id);
    let proposals = validate_proposals(&db, summary.run_id, &[proposal(&[id.clone(), invented.clone()])], &check).unwrap();
    assert_eq!(proposals[0].items.len(), 1);
    assert_eq!(proposals[0].verified_bytes, 4096);
    assert_eq!(proposals[0].rejected.len(), 1);
    assert_eq!(proposals[0].rejected[0].candidate_id, invented);
    assert!(!proposals[0].preview.permanent_delete);

    // 3. The file changes after the scan: the same proposal is now refused on the real disk check.
    fs::write(&old, vec![9_u8; 5000]).unwrap();
    let again = validate_proposals(&db, summary.run_id, &[proposal(&[id.clone()])], &check).unwrap();
    assert!(again[0].items.is_empty(), "a file that changed since the scan is not offered");
    assert!(again[0].rejected[0].reason.contains("changed or unavailable"), "{}", again[0].rejected[0].reason);

    // 4. And a file that has disappeared.
    fs::remove_file(&old).unwrap();
    let gone = validate_proposals(&db, summary.run_id, &[proposal(&[id])], &check).unwrap();
    assert!(gone[0].items.is_empty());
}
