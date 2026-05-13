use std::fs;

use winclean_lib::db::Database;
use winclean_lib::scan::{
    choose_scan_plan, classify_file_attributes, run_recursive_scan, FileAttributeFlags, ScanMode,
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
