use std::{
    fs::{self, File},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, UNIX_EPOCH},
};

use std::collections::HashMap;

use winclean_lib::db::Database;
use winclean_lib::offload::{
    active_offload_job, create_offload_job, list_offload_jobs, process_offload_job_with_client,
    restore_offload_item, run_offload_job_with_client, DriveFolder, DriveUploadResult,
    DriveUploader, FileRecycler, UploadProgress,
};
use winclean_lib::recommendations::{
    build_recommendations, resolve_offload_recommendation_candidates, RecommendationFilters,
    RecommendationKind, RecommendedAction,
};
use winclean_lib::reports::{write_diagnostics_report, write_scan_report};
use winclean_lib::scan::{
    balanced_worker_count, choose_scan_plan, classify_file_attributes, finalize_failed_scan_run,
    run_recursive_scan, should_skip_directory_for_scan, DirectoryRollup, FileAttributeFlags,
    IndexedEntry, ScanMode, ScanRunOptions, ScanState, ScanSummary,
};
use winclean_lib::{
    cleanup::{
        build_cleanup_plan, cleanup_candidate_id, execute_cleanup_plan, resolve_cleanup_candidates,
        validate_cleanup_target, CleanupCandidate, CleanupRisk,
    },
    commands::begin_scan_progress,
    db::CleanupActionInsert,
    volume::{resolve_local_scan_root, should_probe_volume_details, VolumeInfo},
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
    assert!(tables.contains(&"offload_jobs".to_string()));
    assert!(tables.contains(&"offload_items".to_string()));
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
fn balanced_worker_count_keeps_one_logical_core_free_and_caps_pool() {
    assert_eq!(balanced_worker_count(0), 1);
    assert_eq!(balanced_worker_count(1), 1);
    assert_eq!(balanced_worker_count(4), 3);
    assert_eq!(balanced_worker_count(16), 15);
    assert_eq!(balanced_worker_count(64), 16);
}

#[test]
fn local_scan_root_policy_accepts_ready_fixed_and_removable_roots_only() {
    let volumes = vec![
        volume("C:\\", "fixed", true, Some("NTFS")),
        volume("E:\\", "removable", true, Some("exFAT")),
        volume("Z:\\", "remote", true, Some("NTFS")),
        volume("D:\\", "fixed", false, Some("NTFS")),
    ];

    assert_eq!(
        resolve_local_scan_root("c:/", &volumes)
            .expect("fixed root")
            .root_path,
        "C:\\"
    );
    assert_eq!(
        resolve_local_scan_root("E:\\", &volumes)
            .expect("removable root")
            .root_path,
        "E:\\"
    );
    assert!(resolve_local_scan_root("Z:\\", &volumes)
        .expect_err("remote root should be rejected")
        .contains("local"));
    assert!(resolve_local_scan_root("D:\\", &volumes)
        .expect_err("unready root should be rejected")
        .contains("ready"));
    assert!(resolve_local_scan_root("C:\\Users", &volumes)
        .expect_err("arbitrary path should be rejected")
        .contains("local volume root"));
}

#[test]
fn volume_metadata_probe_policy_skips_unsupported_drive_types() {
    assert!(should_probe_volume_details(2));
    assert!(should_probe_volume_details(3));

    assert!(!should_probe_volume_details(0));
    assert!(!should_probe_volume_details(1));
    assert!(!should_probe_volume_details(4));
    assert!(!should_probe_volume_details(5));
    assert!(!should_probe_volume_details(6));
}

#[test]
fn begin_scan_progress_atomically_rejects_existing_running_scan() {
    let progress = std::sync::Mutex::new(Default::default());
    let plan = choose_scan_plan("C:\\", Some("NTFS"));

    begin_scan_progress(&progress, "C:\\", &plan).expect("first scan starts");
    let error =
        begin_scan_progress(&progress, "C:\\", &plan).expect_err("second scan should be rejected");

    assert!(error.contains("already running"));
}

#[test]
fn scan_directory_policy_skips_reparse_placeholder_and_symlink_directories() {
    assert!(should_skip_directory_for_scan(
        FileAttributeFlags::REPARSE_POINT.bits,
        false
    ));
    assert!(should_skip_directory_for_scan(
        FileAttributeFlags::OFFLINE.bits,
        false
    ));
    assert!(should_skip_directory_for_scan(0, true));
    assert!(!should_skip_directory_for_scan(0, false));
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
fn recursive_scan_reports_multicore_worker_count_in_progress() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("small.txt"), vec![1_u8; 10]).expect("small file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let progress = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));

    let summary = run_recursive_scan(
        &db,
        root.as_path(),
        Some(ScanRunOptions {
            file_system: None,
            attempted_mode: None,
            fallback_reason: None,
            cancel: None,
            progress: Some(std::sync::Arc::clone(&progress)),
            progress_sink: None,
        }),
    )
    .expect("scan");

    assert_eq!(summary.files_indexed, 1);
    let progress = progress.lock().expect("progress lock");
    assert_eq!(progress.state, ScanState::Completed);
    assert_eq!(
        progress.worker_count,
        Some(balanced_worker_count(
            std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(1)
        ))
    );
    assert_eq!(progress.phase.as_deref(), Some("complete"));
}

#[test]
fn missing_scan_root_is_persisted_as_failed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let missing_root = temp.path().join("missing");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let progress = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));

    let error = run_recursive_scan(
        &db,
        missing_root.as_path(),
        Some(ScanRunOptions {
            file_system: None,
            attempted_mode: None,
            fallback_reason: None,
            cancel: None,
            progress: Some(std::sync::Arc::clone(&progress)),
            progress_sink: None,
        }),
    )
    .expect_err("missing root should fail");

    assert!(error.to_string().contains("scan root is not readable"));
    let runs = db.list_scan_runs(10).expect("scan runs");
    assert_eq!(runs.first().map(|run| run.status.as_str()), Some("failed"));
    assert!(runs
        .first()
        .and_then(|run| run.last_error.as_deref())
        .unwrap_or_default()
        .contains("scan root is not readable"));
    let progress = progress.lock().expect("progress lock");
    assert_eq!(progress.state, ScanState::Failed);
    assert!(progress
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("scan root is not readable"));
}

#[test]
fn startup_finalizes_interrupted_running_scan_runs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let run_id = db
        .create_scan_run("C:\\", Some("NTFS"), ScanMode::Recursive, None, None, 100)
        .expect("create running scan");

    assert_eq!(db.finalize_interrupted_scan_runs().expect("finalize"), 1);

    let run = db
        .scan_run(run_id)
        .expect("read scan run")
        .expect("scan run exists");
    assert_eq!(run.status, "failed");
    assert!(run.finished_at_ms.expect("finished timestamp") >= 100);
    assert_eq!(
        run.last_error.as_deref(),
        Some("Scan was interrupted before the app closed.")
    );
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
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run(
            "C:\\Data",
            Some("NTFS"),
            ScanMode::Recursive,
            None,
            None,
            100,
        )
        .expect("create scan run");
    let old_modified = current_test_millis() - hours_ms(25);
    db.insert_file_entries(
        run_id,
        &[
            cleanup_indexed_entry("C:\\Data\\cache.tmp", 20, Some(old_modified)),
            cleanup_indexed_entry("C:\\Data\\debug.log", 30, Some(old_modified)),
            cleanup_indexed_entry("C:\\Data\\photo.jpg", 40, Some(old_modified)),
        ],
    )
    .expect("insert file entries");

    let plan = build_cleanup_plan(&db, run_id, 50).expect("cleanup plan");

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
        cleanup_candidate_id(run_id, plan.candidates[0].entry_id),
        plan.candidates[0].id
    );
}

#[test]
fn recommendations_split_safe_cleanup_from_review_and_offload_candidates() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    let downloads = root.join("Users").join("kevin").join("Downloads");
    let duplicates_a = root.join("A");
    let duplicates_b = root.join("B");
    fs::create_dir_all(&downloads).expect("downloads dir");
    fs::create_dir_all(&duplicates_a).expect("duplicates a");
    fs::create_dir_all(&duplicates_b).expect("duplicates b");
    fs::write(root.join("cache.tmp"), vec![1_u8; 20]).expect("tmp file");
    write_sparse_file(&downloads.join("setup.msi"), 75 * 1024 * 1024);
    write_sparse_file(&root.join("movie.mp4"), 120 * 1024 * 1024);
    write_sparse_file(&duplicates_a.join("same.bin"), 15 * 1024 * 1024);
    write_sparse_file(&duplicates_b.join("same.bin"), 15 * 1024 * 1024);
    // Anything changed in the last 24 hours is never suggested (cleanup would refuse it), so age the
    // files that are expected to be candidates.
    set_modified_time(&root.join("cache.tmp"), current_test_millis() - hours_ms(25));
    set_modified_time(&downloads.join("setup.msi"), current_test_millis() - hours_ms(24 * 40));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");

    let tmp = recommendations
        .candidates
        .iter()
        .find(|candidate| candidate.path.ends_with("cache.tmp"))
        .expect("tmp candidate");
    assert_eq!(tmp.kind, RecommendationKind::Clean);
    assert_eq!(tmp.recommended_action, RecommendedAction::RecycleBin);
    assert!(!tmp.eligible_for_offload);

    let installer = recommendations
        .candidates
        .iter()
        .find(|candidate| candidate.path.ends_with("setup.msi"))
        .expect("installer candidate");
    assert_eq!(installer.kind, RecommendationKind::Offload);
    assert_eq!(
        installer.recommended_action,
        RecommendedAction::GoogleDriveOffload
    );
    assert!(installer.eligible_for_offload);

    let possible_duplicates = recommendations
        .candidates
        .iter()
        .filter(|candidate| candidate.reason.contains("Possible duplicate"))
        .collect::<Vec<_>>();
    assert_eq!(possible_duplicates.len(), 2);
    assert!(possible_duplicates
        .iter()
        .all(|candidate| candidate.kind == RecommendationKind::Review));
}

#[test]
fn cleanup_revalidates_changed_files_before_recycling() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let candidate_path = root.join("cache.tmp");
    fs::write(&candidate_path, vec![1_u8; 20]).expect("tmp file");
    set_modified_time(&candidate_path, current_test_millis() - hours_ms(25));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let plan = build_cleanup_plan(&db, summary.run_id, 50).expect("cleanup plan");
    let candidate = plan
        .candidates
        .iter()
        .find(|candidate| candidate.path.ends_with("cache.tmp"))
        .expect("tmp candidate");

    fs::write(&candidate_path, vec![2_u8; 40]).expect("change tmp file");
    let result = winclean_lib::cleanup::execute_cleanup_plan(
        &db,
        summary.run_id,
        std::slice::from_ref(&candidate.id),
    )
    .expect("cleanup execution");

    assert_eq!(result.recycled_count, 0);
    assert_eq!(result.skipped_count, 1);
    assert!(candidate_path.exists());
}

#[test]
fn offload_job_uploads_verifies_then_recycles_with_fake_drive_client() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let file_path = root.join("movie.mp4");
    write_sparse_file(&file_path, 120 * 1024 * 1024);
    set_modified_time(&file_path, current_test_millis() - hours_ms(25));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let candidate = recommendations
        .candidates
        .into_iter()
        .find(|candidate| candidate.path.ends_with("movie.mp4"))
        .expect("movie offload candidate");
    let drive = FakeDrive::default();
    let recycler = FakeRecycler::default();
    let cancel = AtomicBool::new(false);

    let status = run_offload_job_with_client(
        &db,
        summary.run_id,
        vec![candidate],
        &drive,
        &recycler,
        &cancel,
    )
    .expect("offload job");

    assert_eq!(status.status, "completed");
    assert_eq!(status.total_items, 1);
    assert_eq!(status.completed_items, 1);
    assert_eq!(status.failed_items, 0);
    assert_eq!(status.items[0].status, "completed");
    assert_eq!(
        status.items[0].drive_file_id.as_deref(),
        Some("drive-file-1")
    );
    assert_eq!(status.items[0].recycle_status.as_deref(), Some("recycled"));
    // The offload job recycles the canonical path, which on Windows carries a `\\?\` prefix.
    let recycled = recycler
        .recycled
        .lock()
        .expect("recycler lock")
        .iter()
        .map(|path| path.strip_prefix("\\\\?\\").unwrap_or(path).to_string())
        .collect::<Vec<_>>();
    assert_eq!(recycled, vec![file_path.to_string_lossy().to_string()]);
}

#[test]
fn offload_candidate_resolution_rejects_review_candidates_even_when_eligible() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    let duplicates_a = root.join("A");
    let duplicates_b = root.join("B");
    fs::create_dir_all(&duplicates_a).expect("duplicates a");
    fs::create_dir_all(&duplicates_b).expect("duplicates b");
    write_sparse_file(&duplicates_a.join("same.bin"), 15 * 1024 * 1024);
    write_sparse_file(&duplicates_b.join("same.bin"), 15 * 1024 * 1024);

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let review_candidate = recommendations
        .candidates
        .iter()
        .find(|candidate| candidate.kind == RecommendationKind::Review)
        .expect("review candidate");
    assert!(review_candidate.eligible_for_offload);

    let error = resolve_offload_recommendation_candidates(
        &recommendations.candidates,
        std::slice::from_ref(&review_candidate.id),
    )
    .expect_err("review candidates must not be executable offload candidates");

    assert!(error
        .to_string()
        .contains("Only Google Drive offload candidates"));
}

#[test]
fn offload_job_rejects_item_count_mismatches_before_uploading() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let file_path = root.join("movie.mp4");
    write_sparse_file(&file_path, 120 * 1024 * 1024);
    set_modified_time(&file_path, current_test_millis() - hours_ms(25));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let candidate = recommendations
        .candidates
        .into_iter()
        .find(|candidate| candidate.path.ends_with("movie.mp4"))
        .expect("movie offload candidate");
    let status = create_offload_job(&db, summary.run_id, std::slice::from_ref(&candidate))
        .expect("offload job");
    let drive = FakeDrive::default();
    let recycler = FakeRecycler::default();
    let cancel = AtomicBool::new(false);

    let error =
        process_offload_job_with_client(&db, status.id, Vec::new(), &drive, &recycler, &cancel)
            .expect_err("item mismatch should fail");

    assert!(error.to_string().contains("offload job items mismatch"));
    assert_eq!(drive.upload_count.load(Ordering::Relaxed), 0);
    assert!(recycler.recycled.lock().expect("recycler lock").is_empty());
}

#[test]
fn offload_job_skips_changed_files_without_counting_uploaded_bytes() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let file_path = root.join("movie.mp4");
    write_sparse_file(&file_path, 120 * 1024 * 1024);
    set_modified_time(&file_path, current_test_millis() - hours_ms(25));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let candidate = recommendations
        .candidates
        .into_iter()
        .find(|candidate| candidate.path.ends_with("movie.mp4"))
        .expect("movie offload candidate");
    fs::write(&file_path, b"changed after scan").expect("change offload candidate");

    let drive = FakeDrive::default();
    let recycler = FakeRecycler::default();
    let cancel = AtomicBool::new(false);

    let status = run_offload_job_with_client(
        &db,
        summary.run_id,
        vec![candidate],
        &drive,
        &recycler,
        &cancel,
    )
    .expect("offload job");

    assert_eq!(status.status, "completed_with_errors");
    assert_eq!(status.completed_items, 0);
    assert_eq!(status.failed_items, 1);
    assert_eq!(status.uploaded_bytes, 0);
    assert_eq!(status.items[0].status, "skipped");
    assert_eq!(status.items[0].uploaded_bytes, 0);
    assert_eq!(drive.upload_count.load(Ordering::Relaxed), 0);
    assert!(recycler.recycled.lock().expect("recycler lock").is_empty());
}

#[test]
fn offload_job_can_be_cancelled_before_side_effects() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let file_path = root.join("movie.mp4");
    write_sparse_file(&file_path, 120 * 1024 * 1024);
    set_modified_time(&file_path, current_test_millis() - hours_ms(25));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let candidate = recommendations
        .candidates
        .into_iter()
        .find(|candidate| candidate.path.ends_with("movie.mp4"))
        .expect("movie offload candidate");
    let drive = FakeDrive::default();
    let recycler = FakeRecycler::default();
    let cancel = AtomicBool::new(true);

    let status = run_offload_job_with_client(
        &db,
        summary.run_id,
        vec![candidate],
        &drive,
        &recycler,
        &cancel,
    )
    .expect("offload job");

    assert_eq!(status.status, "canceled");
    assert_eq!(status.completed_items, 0);
    assert_eq!(status.failed_items, 0);
    assert_eq!(drive.upload_count.load(Ordering::Relaxed), 0);
    assert!(recycler.recycled.lock().expect("recycler lock").is_empty());
}

#[test]
fn offload_job_records_failed_uploads_and_history_listing() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let file_path = root.join("movie.mp4");
    write_sparse_file(&file_path, 120 * 1024 * 1024);
    set_modified_time(&file_path, current_test_millis() - hours_ms(25));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let candidate = recommendations
        .candidates
        .into_iter()
        .find(|candidate| candidate.path.ends_with("movie.mp4"))
        .expect("movie offload candidate");
    let drive = FakeDrive::default();
    drive.fail_first_uploads.store(1, Ordering::Relaxed);
    let recycler = FakeRecycler::default();
    let cancel = AtomicBool::new(false);

    let status = run_offload_job_with_client(
        &db,
        summary.run_id,
        vec![candidate],
        &drive,
        &recycler,
        &cancel,
    )
    .expect("offload job");

    assert_eq!(status.status, "completed_with_errors");
    assert_eq!(status.failed_items, 1);
    assert_eq!(status.items[0].status, "failed");
    assert!(status.items[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("simulated transient upload failure"));

    let jobs = list_offload_jobs(&db, 10).expect("list offload jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, status.id);
    assert_eq!(jobs[0].items.len(), 1);
    assert!(active_offload_job(&db)
        .expect("active offload job")
        .is_none());
}

#[test]
fn offload_item_can_be_restored_from_drive_with_verification() {
    let temp = safe_tempdir();
    let root = temp.path().join("fixture");
    let downloads = root.join("Downloads");
    fs::create_dir_all(&downloads).expect("fixture dir");
    let file_path = downloads.join("movie.mp4");
    let payload = vec![7_u8; 64 * 1024];
    fs::write(&file_path, &payload).expect("write payload");
    set_modified_time(&file_path, current_test_millis() - hours_ms(31 * 24));

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");

    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    let recommendations =
        build_recommendations(&db, summary.run_id, RecommendationFilters::default())
            .expect("recommendations");
    let candidate = recommendations
        .candidates
        .into_iter()
        .find(|candidate| candidate.path.ends_with("movie.mp4"))
        .expect("movie offload candidate");
    let drive = FakeDrive::default();
    *drive.download_payload.lock().expect("payload lock") = Some(payload.clone());
    let recycler = DeletingRecycler;
    let cancel = AtomicBool::new(false);

    let status = run_offload_job_with_client(
        &db,
        summary.run_id,
        vec![candidate],
        &drive,
        &recycler,
        &cancel,
    )
    .expect("offload job");
    assert_eq!(status.status, "completed");
    assert!(!file_path.exists());

    let restored = restore_offload_item(&db, status.items[0].id, &drive, None).expect("restore");
    assert_eq!(restored.recycle_status.as_deref(), Some("restored"));
    assert!(file_path.exists());
    assert_eq!(
        fs::metadata(&file_path).expect("restored metadata").len(),
        payload.len() as u64
    );

    let history = db.cleanup_history(20).expect("history");
    assert!(history
        .iter()
        .any(|action| action.action_type == "drive_restore" && action.status == "completed"));
}

#[test]
fn interrupted_offload_jobs_are_finalized_on_startup() {
    let temp = safe_tempdir();
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run("C:\\Data", Some("NTFS"), ScanMode::Recursive, None, None, 1)
        .expect("scan run");
    let job_id = db
        .create_offload_job(winclean_lib::db::OffloadJobInsert {
            scan_run_id: run_id,
            status: "running".to_string(),
            destination_label: "test".to_string(),
            total_items: 1,
            total_bytes: 64,
            created_at_ms: 1,
        })
        .expect("offload job");
    db.insert_offload_item(winclean_lib::db::OffloadItemInsert {
        job_id,
        entry_id: 1,
        candidate_id: "scan-1:entry-1".to_string(),
        source_path: "C:\\Data\\stuck.bin".to_string(),
        size: 64,
        created_at_ms: 1,
    })
    .expect("offload item");

    assert!(db
        .latest_active_offload_job_id()
        .expect("active id")
        .is_some());
    let finalized = db
        .finalize_interrupted_offload_jobs()
        .expect("finalize interrupted offload jobs");
    assert_eq!(finalized, 1);
    assert!(db
        .latest_active_offload_job_id()
        .expect("active id")
        .is_none());
    let jobs = db.list_offload_jobs(10).expect("list jobs");
    assert_eq!(jobs[0].status, "interrupted");
    assert_eq!(jobs[0].failed_items, 1, "aggregates are recalculated");
    let items = db.offload_items(job_id).expect("job items");
    assert_eq!(items[0].status, "interrupted");
}

#[test]
fn cleanup_candidate_resolution_dedupes_and_rejects_stale_ids_before_cleanup() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run(
            "C:\\Data",
            Some("NTFS"),
            ScanMode::Recursive,
            None,
            None,
            100,
        )
        .expect("create scan run");
    let old_modified = current_test_millis() - hours_ms(25);
    db.insert_file_entries(
        run_id,
        &[
            cleanup_indexed_entry("C:\\Data\\cache.tmp", 20, Some(old_modified)),
            cleanup_indexed_entry("C:\\Data\\debug.log", 30, Some(old_modified)),
        ],
    )
    .expect("insert file entries");

    let plan = build_cleanup_plan(&db, run_id, 50).expect("cleanup plan");
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
fn cleanup_plan_filters_candidates_in_sql_before_size_limit() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run("C:\\", Some("NTFS"), ScanMode::Recursive, None, None, 100)
        .expect("create scan run");

    let old_modified = current_test_millis() - hours_ms(25);
    let mut entries = (0..5000)
        .map(|index| IndexedEntry {
            path: format!("C:\\Data\\photo-{index}.jpg"),
            parent_path: Some("C:\\Data".to_string()),
            name: format!("photo-{index}.jpg"),
            extension: Some("jpg".to_string()),
            is_directory: false,
            size: 10_000,
            modified_at_ms: Some(old_modified),
            created_at_ms: None,
            accessed_at_ms: None,
            attributes: 0,
            is_reparse_point: false,
            is_placeholder: false,
            is_system: false,
        })
        .collect::<Vec<_>>();
    entries.push(IndexedEntry {
        path: "C:\\Data\\cache.tmp".to_string(),
        parent_path: Some("C:\\Data".to_string()),
        name: "cache.tmp".to_string(),
        extension: Some("tmp".to_string()),
        is_directory: false,
        size: 1,
        modified_at_ms: Some(old_modified),
        created_at_ms: None,
        accessed_at_ms: None,
        attributes: 0,
        is_reparse_point: false,
        is_placeholder: false,
        is_system: false,
    });
    db.insert_file_entries(run_id, &entries)
        .expect("insert file entries");

    let plan = build_cleanup_plan(&db, run_id, 10).expect("cleanup plan");

    assert!(plan
        .candidates
        .iter()
        .any(|candidate| candidate.path.ends_with("cache.tmp")));
}

#[test]
fn cleanup_plan_skips_recent_and_unknown_age_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run(
            "C:\\Data",
            Some("NTFS"),
            ScanMode::Recursive,
            None,
            None,
            100,
        )
        .expect("create scan run");
    let now = current_test_millis();
    db.insert_file_entries(
        run_id,
        &[
            cleanup_indexed_entry("C:\\Data\\old.tmp", 10, Some(now - hours_ms(25))),
            cleanup_indexed_entry("C:\\Data\\recent.tmp", 20, Some(now - hours_ms(1))),
            cleanup_indexed_entry("C:\\Data\\unknown.tmp", 30, None),
        ],
    )
    .expect("insert file entries");

    let plan = build_cleanup_plan(&db, run_id, 50).expect("cleanup plan");

    assert_eq!(plan.candidates.len(), 1);
    assert!(plan.candidates[0].path.ends_with("old.tmp"));
}

#[test]
fn cleanup_execution_reports_missing_and_changed_candidates_as_skipped() {
    // On Windows the fixture must live outside %TEMP% (inside AppData, which cleanup protects),
    // so it uses C:\tmp; elsewhere the ordinary temp directory has no such protection.
    let base = if cfg!(windows) {
        let base = std::path::PathBuf::from("C:\\tmp");
        fs::create_dir_all(&base).expect("C:\\tmp");
        base
    } else {
        std::env::temp_dir()
    };
    let temp = tempfile::Builder::new()
        .prefix("winclean-cleanup-")
        .tempdir_in(base)
        .expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    let changed_path = root.join("changed.tmp");
    fs::write(&changed_path, vec![1_u8; 40]).expect("changed file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run(
            root.to_string_lossy().as_ref(),
            Some("NTFS"),
            ScanMode::Recursive,
            None,
            None,
            100,
        )
        .expect("create scan run");
    let old_modified = current_test_millis() - hours_ms(25);
    let missing_path = root.join("missing.tmp");
    db.insert_file_entries(
        run_id,
        &[
            cleanup_indexed_entry(&changed_path.to_string_lossy(), 10, Some(old_modified)),
            cleanup_indexed_entry(&missing_path.to_string_lossy(), 10, Some(old_modified)),
        ],
    )
    .expect("insert file entries");
    let plan = build_cleanup_plan(&db, run_id, 50).expect("cleanup plan");
    let candidate_ids = plan
        .candidates
        .iter()
        .map(|candidate| candidate.id.clone())
        .collect::<Vec<_>>();

    let result = execute_cleanup_plan(&db, run_id, &candidate_ids).expect("cleanup execution");

    assert_eq!(result.recycled_count, 0);
    assert_eq!(result.failed_count, 0);
    assert_eq!(result.skipped_count, 2);
    assert!(changed_path.exists());
    assert!(db
        .cleanup_history(10)
        .expect("cleanup history")
        .iter()
        .all(|action| action.status == "skipped"));
}

#[test]
fn top_directories_clamps_oversized_limits() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let run_id = db
        .create_scan_run("C:\\", Some("NTFS"), ScanMode::Recursive, None, None, 100)
        .expect("create scan run");
    let rollups = (0..150)
        .map(|index| {
            (
                std::path::PathBuf::from(format!("C:\\Data\\dir-{index}")),
                DirectoryRollup {
                    total_size: index,
                    file_count: 1,
                    directory_count: 0,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    db.replace_directory_rollups(run_id, &rollups)
        .expect("insert rollups");

    let rows = db
        .top_directories(run_id, u32::MAX)
        .expect("top directories");

    assert_eq!(rows.len(), 100);
}

#[test]
fn cleanup_target_revalidation_rejects_protected_original_paths() {
    let candidate = cleanup_candidate_for_path("C:\\Windows\\Temp\\cache.tmp");

    let error = validate_cleanup_target(&candidate).expect_err("protected path rejected");

    assert!(error.to_string().contains("protected"));
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

#[test]
fn repeated_exports_use_distinct_timestamped_paths() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("cache.tmp"), vec![1_u8; 20]).expect("tmp file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");

    let first_scan = write_scan_report(&db, summary.run_id, temp.path()).expect("first report");
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second_scan = write_scan_report(&db, summary.run_id, temp.path()).expect("second report");
    let first_diagnostics =
        write_diagnostics_report(&db, "0.1.0", temp.path()).expect("first diagnostics");
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second_diagnostics =
        write_diagnostics_report(&db, "0.1.0", temp.path()).expect("second diagnostics");

    assert_ne!(first_scan.path, second_scan.path);
    assert_ne!(first_diagnostics.path, second_diagnostics.path);
    assert!(first_scan
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .starts_with(&format!("winclean-scan-report-{}-", summary.run_id)));
    assert!(first_diagnostics
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .starts_with("winclean-diagnostics-"));
}

#[test]
fn deleting_scan_history_removes_scan_data_but_preserves_cleanup_history() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("old.tmp"), vec![1_u8; 20]).expect("tmp file");

    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let summary = run_recursive_scan(&db, root.as_path(), None).expect("scan");
    db.record_cleanup_action(CleanupActionInsert {
        action_type: "recycle_bin".to_string(),
        status: "skipped".to_string(),
        target_path: root.join("old.tmp").to_string_lossy().to_string(),
        original_path: Some(root.join("old.tmp").to_string_lossy().to_string()),
        destination_path: Some("Windows Recycle Bin".to_string()),
        scan_run_id: Some(summary.run_id),
        explanation: Some("test action".to_string()),
        created_at_ms: 10,
        completed_at_ms: Some(20),
        undo_token: None,
    })
    .expect("record cleanup action");

    assert_eq!(
        db.delete_scan_run(summary.run_id).expect("delete scan"),
        true
    );

    assert!(db.scan_run(summary.run_id).expect("read scan").is_none());
    assert!(db
        .top_directories(summary.run_id, 10)
        .expect("top directories")
        .is_empty());
    let history = db.cleanup_history(10).expect("cleanup history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].scan_run_id, None);
}

#[test]
fn clear_scan_history_keeps_running_scans() {
    let temp = tempfile::tempdir().expect("temp dir");
    let db_path = temp.path().join("index.sqlite3");
    let db = Database::open(&db_path).expect("open db");
    db.migrate().expect("migrate db");
    let completed_run = db
        .create_scan_run(
            "C:\\Done",
            Some("NTFS"),
            ScanMode::Recursive,
            None,
            None,
            100,
        )
        .expect("create completed scan");
    let running_run = db
        .create_scan_run(
            "C:\\Running",
            Some("NTFS"),
            ScanMode::Recursive,
            None,
            None,
            200,
        )
        .expect("create running scan");
    let summary = ScanSummary {
        run_id: completed_run,
        root_path: "C:\\Done".to_string(),
        mode: ScanMode::Recursive,
        attempted_mode: None,
        fallback_reason: None,
        started_at_ms: 100,
        finished_at_ms: 150,
        files_indexed: 0,
        directories_indexed: 0,
        bytes_indexed: 0,
        access_denied_count: 0,
        skipped_reparse_count: 0,
        error_count: 0,
        canceled: false,
    };
    db.finish_scan_run(&summary, "completed", None)
        .expect("finish completed scan");

    assert_eq!(db.clear_scan_history().expect("clear history"), 1);

    assert!(db
        .scan_run(completed_run)
        .expect("completed scan")
        .is_none());
    assert_eq!(
        db.scan_run(running_run)
            .expect("running scan")
            .map(|run| run.status),
        Some("running".to_string())
    );
}

fn write_sparse_file(path: &std::path::Path, size: u64) {
    let file = File::create(path).expect("create sparse file");
    file.set_len(size).expect("set sparse file length");
}

fn set_modified_time(path: &std::path::Path, modified_at_ms: i64) {
    let modified = UNIX_EPOCH + Duration::from_millis(modified_at_ms as u64);
    File::options()
        .write(true)
        .open(path)
        .expect("open file for timestamp")
        .set_modified(modified)
        .expect("set modified time");
}

fn safe_tempdir() -> tempfile::TempDir {
    let fixture_root = std::env::current_dir()
        .expect("current dir")
        .join("target")
        .join("test-fixtures");
    fs::create_dir_all(&fixture_root).expect("fixture root");
    tempfile::Builder::new()
        .prefix("winclean-")
        .tempdir_in(fixture_root)
        .expect("safe temp dir")
}

#[derive(Default)]
struct FakeDrive {
    upload_count: std::sync::atomic::AtomicUsize,
    delete_count: std::sync::atomic::AtomicUsize,
    fail_first_uploads: std::sync::atomic::AtomicUsize,
    download_payload: Mutex<Option<Vec<u8>>>,
}

impl DriveUploader for FakeDrive {
    fn ensure_batch_folder(&self, _batch_name: &str) -> anyhow::Result<DriveFolder> {
        Ok(DriveFolder {
            id: "drive-folder-1".to_string(),
            name: "WinClean Offloads/test".to_string(),
        })
    }

    fn upload_file(
        &self,
        _folder_id: &str,
        candidate: &winclean_lib::recommendations::RecommendationCandidate,
        progress: &UploadProgress<'_>,
    ) -> anyhow::Result<DriveUploadResult> {
        if self
            .fail_first_uploads
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                (remaining > 0).then(|| remaining - 1)
            })
            .is_ok()
        {
            anyhow::bail!("simulated transient upload failure");
        }
        self.upload_count.fetch_add(1, Ordering::Relaxed);
        (progress.report)(candidate.size / 2);
        (progress.report)(candidate.size);
        Ok(DriveUploadResult {
            file_id: "drive-file-1".to_string(),
            web_url: Some("https://drive.google.com/file/d/drive-file-1/view".to_string()),
            size: candidate.size,
            md5_checksum: None,
        })
    }

    fn delete_file(&self, _file_id: &str) -> anyhow::Result<()> {
        self.delete_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn download_file(
        &self,
        _file_id: &str,
        destination: &std::path::Path,
    ) -> anyhow::Result<u64> {
        let payload = self
            .download_payload
            .lock()
            .expect("download payload lock")
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no download payload configured"))?;
        fs::write(destination, &payload)?;
        Ok(payload.len() as u64)
    }
}

/// Recycler stand-in that actually removes the file so restore flows can be
/// exercised end to end in tests.
struct DeletingRecycler;

impl FileRecycler for DeletingRecycler {
    fn recycle_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        fs::remove_file(path)?;
        Ok(())
    }
}

#[derive(Default)]
struct FakeRecycler {
    recycled: Mutex<Vec<String>>,
}

impl FileRecycler for FakeRecycler {
    fn recycle_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        self.recycled
            .lock()
            .expect("recycler lock")
            .push(path.to_string_lossy().to_string());
        Ok(())
    }
}

fn volume(
    root_path: &str,
    drive_type: &str,
    is_ready: bool,
    file_system: Option<&str>,
) -> VolumeInfo {
    VolumeInfo {
        name: root_path.to_string(),
        root_path: root_path.to_string(),
        file_system: file_system.map(str::to_string),
        drive_type: drive_type.to_string(),
        is_ready,
        available_space: Some(1),
        total_space: Some(2),
    }
}

fn cleanup_candidate_for_path(path: &str) -> CleanupCandidate {
    CleanupCandidate {
        id: "scan-1:entry-1".to_string(),
        entry_id: 1,
        scan_run_id: 1,
        path: path.to_string(),
        name: "cache.tmp".to_string(),
        size: 1,
        reason: "Temporary file extension".to_string(),
        risk_level: CleanupRisk::Low,
        action_kind: "recycle_bin".to_string(),
        modified_at_ms: None,
    }
}

fn cleanup_indexed_entry(path: &str, size: u64, modified_at_ms: Option<i64>) -> IndexedEntry {
    let path_buf = std::path::PathBuf::from(path);
    IndexedEntry {
        path: path.to_string(),
        parent_path: path_buf
            .parent()
            .map(|parent| parent.to_string_lossy().to_string()),
        name: path_buf
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string()),
        extension: path_buf
            .extension()
            .map(|ext| ext.to_string_lossy().to_string()),
        is_directory: false,
        size,
        modified_at_ms,
        created_at_ms: None,
        accessed_at_ms: None,
        attributes: 0,
        is_reparse_point: false,
        is_placeholder: false,
        is_system: false,
    }
}

fn current_test_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn hours_ms(hours: i64) -> i64 {
    hours * 60 * 60 * 1000
}

#[test]
fn callback_only_progress_reports_terminal_state_without_mutex() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("fixture");
    fs::create_dir_all(&root).expect("fixture dir");
    fs::write(root.join("small.txt"), vec![1_u8; 10]).expect("small file");
    let db = Database::open(temp.path().join("index.sqlite3")).expect("open db");
    db.migrate().expect("migrate db");

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_seen = std::sync::Arc::clone(&seen);
    let summary = run_recursive_scan(
        &db,
        root.as_path(),
        Some(ScanRunOptions {
            file_system: None,
            attempted_mode: None,
            fallback_reason: None,
            cancel: None,
            progress: None,
            progress_sink: Some(std::sync::Arc::new(move |p: &winclean_lib::scan::ScanProgress| {
                sink_seen.lock().expect("sink lock").push((p.state.clone(), p.run_id));
            })),
        }),
    )
    .expect("scan");
    assert_eq!(summary.files_indexed, 1);
    let seen = seen.lock().expect("seen lock");
    let (last_state, last_run) = seen.last().expect("terminal callback");
    assert_eq!(*last_state, ScanState::Completed);
    assert!(last_run.is_some());

    // Failed scans also deliver a terminal callback without a mutex.
    let failed_seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let failed_sink = std::sync::Arc::clone(&failed_seen);
    let missing = temp.path().join("missing");
    let _ = run_recursive_scan(
        &db,
        missing.as_path(),
        Some(ScanRunOptions {
            file_system: None,
            attempted_mode: None,
            fallback_reason: None,
            cancel: None,
            progress: None,
            progress_sink: Some(std::sync::Arc::new(move |p: &winclean_lib::scan::ScanProgress| {
                failed_sink.lock().expect("sink lock").push(p.state.clone());
            })),
        }),
    )
    .expect_err("missing root should fail");
    let failed_seen = failed_seen.lock().expect("failed lock");
    assert_eq!(failed_seen.last(), Some(&ScanState::Failed));
}
