use std::fs;

use winclean_lib::db::Database;
use winclean_lib::reports::write_scan_report;
use winclean_lib::scan::{
    choose_scan_plan, classify_file_attributes, finalize_failed_scan_run, run_recursive_scan,
    FileAttributeFlags, ScanMode, ScanSummary,
};
use winclean_lib::{
    cleanup::{build_cleanup_plan, cleanup_candidate_id, resolve_cleanup_candidates},
    db::CleanupActionInsert,
};

#[test]
fn migration_creates_expected_tables() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");

    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let tables = db.table_names().expect("table names");
    assert!(tables.contains(&"scan_runs".to_string()));
    assert!(tables.contains(&"file_entries".to_string()));
    assert!(tables.contains(&"directory_rollups".to_string()));
    assert!(tables.contains(&"action_history".to_string()));
    assert!(tables.contains(&"schema_migrations".to_string()));
    assert_eq!(
        db.schema_version().expect("schema version").as_deref(),
        Some("202605130001_production_foundation")
    );
}

#[test]
fn ntfs_scan_plan_uses_recursive_fallback_until_usn_provider_is_ready() {
    let plan = choose_scan_plan("C:\\", Some("NTFS"));

    assert_eq!(plan.mode, ScanMode::Recursive);
    assert_eq!(plan.attempted_mode, Some(ScanMode::NtfsUsn));
    assert!(plan
        .fallback_reason
        .as_deref()
        .unwrap_or_default()
        .contains("USN"));
}

#[test]
fn file_attribute_classification_marks_reparse_and_offline_files() {
    let flags = classify_file_attributes(
        FileAttributeFlags::REPARSE_POINT.bits | FileAttributeFlags::OFFLINE.bits,
    );

    assert!(flags.is_reparse_point);
    assert!(flags.is_placeholder);
    assert!(!flags.is_system);
}

#[test]
fn recursive_scan_indexes_files_and_rolls_sizes_up_to_parent_directories() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    let nested = root.join("large").join("nested");
    fs::create_dir_all(&nested).expect("fixture dirs");
    fs::write(root.join("small.txt"), vec![1_u8; 10]).expect("small file");
    fs::write(nested.join("big.bin"), vec![1_u8; 90]).expect("big file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    assert_eq!(summary.files_indexed, 2);
    assert_eq!(summary.bytes_indexed, 100);
    assert_eq!(summary.access_denied_count, 0);

    let top = db
        .top_directories(summary.run_id, 5)
        .expect("top directories");
    assert_eq!(
        top.first().map(|entry| entry.path.as_str()),
        Some(root.to_string_lossy().as_ref())
    );
    assert_eq!(top.first().map(|entry| entry.total_size), Some(100));
    assert!(top
        .iter()
        .any(|entry| entry.path.ends_with("large") && entry.total_size == 90));
}

#[test]
fn failed_scan_finalization_persists_error_state_and_counters() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let run_id = db
        .create_scan_run(
            root.to_string_lossy().as_ref(),
            Some("NTFS"),
            ScanMode::Recursive,
            Some(ScanMode::NtfsUsn),
            Some("USN fallback"),
            100,
        )
        .expect("create scan run");
    let mut summary = ScanSummary {
        run_id,
        root_path: root.to_string_lossy().to_string(),
        mode: ScanMode::Recursive,
        attempted_mode: Some(ScanMode::NtfsUsn),
        fallback_reason: Some("USN fallback".to_string()),
        started_at_ms: 100,
        finished_at_ms: 100,
        files_indexed: 7,
        directories_indexed: 3,
        bytes_indexed: 4096,
        access_denied_count: 1,
        skipped_reparse_count: 2,
        error_count: 4,
        canceled: false,
    };

    finalize_failed_scan_run(&db, &mut summary, None, "simulated sqlite write failure")
        .expect("finalize failed scan");

    let run = db
        .scan_run(run_id)
        .expect("read scan run")
        .expect("scan run exists");
    assert_eq!(run.status, "failed");
    assert!(run.finished_at_ms.expect("finished at") >= 100);
    assert_eq!(run.files_indexed, 7);
    assert_eq!(run.directories_indexed, 3);
    assert_eq!(run.bytes_indexed, 4096);
    assert_eq!(run.access_denied_count, 1);
    assert_eq!(run.skipped_reparse_count, 2);
    assert_eq!(run.error_count, 4);
    assert_eq!(
        run.last_error.as_deref(),
        Some("simulated sqlite write failure")
    );
}

#[test]
fn scan_history_and_directory_queries_expose_persisted_scan_data() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    let nested = root.join("cache");
    fs::create_dir_all(&nested).expect("fixture dirs");
    fs::write(root.join("keep.txt"), vec![1_u8; 16]).expect("keep file");
    fs::write(nested.join("old.log"), vec![1_u8; 84]).expect("log file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let runs = db.list_scan_runs(10).expect("scan runs");
    assert_eq!(runs.first().map(|run| run.id), Some(summary.run_id));
    assert_eq!(runs.first().map(|run| run.bytes_indexed), Some(100));

    let root_children = db
        .directory_children(summary.run_id, Some(root.to_string_lossy().as_ref()), 25)
        .expect("directory children");
    assert!(root_children
        .iter()
        .any(|entry| entry.name == "cache" && entry.is_directory));
    assert!(root_children
        .iter()
        .any(|entry| entry.name == "keep.txt" && !entry.is_directory));

    let found_logs = db
        .file_search(summary.run_id, Some("old"), Some("log"), Some(1), 25)
        .expect("file search");
    assert_eq!(found_logs.len(), 1);
    assert!(found_logs[0].path.ends_with("old.log"));

    let extensions = db
        .extension_summary(summary.run_id, 10)
        .expect("extensions");
    assert!(extensions
        .iter()
        .any(|entry| entry.extension.as_deref() == Some("log") && entry.total_size == 84));

    let categories = db.category_summary(summary.run_id).expect("categories");
    assert!(categories
        .iter()
        .any(|entry| entry.category == "Documents" && entry.file_count >= 1));
}

#[test]
fn cleanup_plan_only_includes_conservative_file_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("cache.tmp"), vec![1_u8; 20]).expect("tmp file");
    fs::write(root.join("debug.log"), vec![1_u8; 30]).expect("log file");
    fs::write(root.join("photo.jpg"), vec![1_u8; 40]).expect("photo file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let plan = build_cleanup_plan(&db, summary.run_id, 50).expect("cleanup plan");

    assert_eq!(plan.candidates.len(), 2);
    assert_eq!(plan.total_bytes, 50);
    assert!(plan
        .candidates
        .iter()
        .any(|candidate| candidate.path.ends_with("cache.tmp")));
    assert!(plan
        .candidates
        .iter()
        .any(|candidate| candidate.path.ends_with("debug.log")));
    assert!(!plan
        .candidates
        .iter()
        .any(|candidate| candidate.path.ends_with("photo.jpg")));
    assert_eq!(
        cleanup_candidate_id(summary.run_id, plan.candidates[0].entry_id),
        plan.candidates[0].id
    );
}

#[test]
fn cleanup_candidate_resolution_dedupes_and_rejects_stale_ids_before_cleanup() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("cache.tmp"), vec![1_u8; 20]).expect("tmp file");
    fs::write(root.join("debug.log"), vec![1_u8; 30]).expect("log file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let plan = build_cleanup_plan(&db, summary.run_id, 50).expect("cleanup plan");
    let tmp_id = plan
        .candidates
        .iter()
        .find(|candidate| candidate.path.ends_with("cache.tmp"))
        .expect("tmp candidate")
        .id
        .clone();
    let log_id = plan
        .candidates
        .iter()
        .find(|candidate| candidate.path.ends_with("debug.log"))
        .expect("log candidate")
        .id
        .clone();

    let resolved =
        resolve_cleanup_candidates(&plan.candidates, &[tmp_id.clone(), tmp_id.clone(), log_id])
            .expect("resolve candidates");
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].id, tmp_id);

    let error = resolve_cleanup_candidates(
        &plan.candidates,
        &[tmp_id, "scan-999:entry-999".to_string()],
    )
    .expect_err("stale candidate id should fail validation");
    assert!(error.to_string().contains("scan-999:entry-999"));
}

#[test]
fn database_connections_enforce_foreign_keys_after_migration() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let error = db
        .record_cleanup_action(CleanupActionInsert {
            action_type: "recycle_bin".to_string(),
            status: "running".to_string(),
            target_path: "C:\\Temp\\cache.tmp".to_string(),
            original_path: Some("C:\\Temp\\cache.tmp".to_string()),
            destination_path: Some("Windows Recycle Bin".to_string()),
            scan_run_id: Some(999_999),
            explanation: Some("Temporary file".to_string()),
            created_at_ms: 10,
            completed_at_ms: None,
            undo_token: None,
        })
        .expect_err("foreign key violation should reject unknown scan run");

    let message = error.to_string().to_ascii_lowercase();
    assert!(message.contains("foreign key") || message.contains("constraint"));
}

#[test]
fn cleanup_history_records_recycle_bin_actions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let id = db
        .record_cleanup_action(CleanupActionInsert {
            action_type: "recycle_bin".to_string(),
            status: "recycled".to_string(),
            target_path: "C:\\Temp\\cache.tmp".to_string(),
            original_path: Some("C:\\Temp\\cache.tmp".to_string()),
            destination_path: Some("Windows Recycle Bin".to_string()),
            scan_run_id: None,
            explanation: Some("Temporary file".to_string()),
            created_at_ms: 10,
            completed_at_ms: Some(20),
            undo_token: Some("recycle-bin".to_string()),
        })
        .expect("record cleanup action");

    let history = db.cleanup_history(10).expect("cleanup history");
    assert_eq!(history.first().map(|entry| entry.id), Some(id));
    assert_eq!(
        history
            .first()
            .map(|entry| entry.destination_path.as_deref()),
        Some(Some("Windows Recycle Bin"))
    );
    assert_eq!(db.pending_action_count().expect("pending count"), 0);
}

#[test]
fn scan_report_export_writes_local_json_artifact() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("cache.tmp"), vec![1_u8; 20]).expect("tmp file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let artifact = write_scan_report(&db, summary.run_id, temp.path()).expect("report");
    let body = fs::read_to_string(&artifact.path).expect("read report");

    assert!(artifact.bytes > 0);
    assert!(body.contains("\"scanRun\""));
    assert!(body.contains("\"topDirectories\""));
    assert!(body.contains("\"cleanupPreview\""));
}
