use std::collections::HashSet;

use winclean_lib::db::Database;
use winclean_lib::scan::{IndexedEntry, ScanMode};

fn file_entry(path: &str, name: &str, size: u64) -> IndexedEntry {
    IndexedEntry {
        path: path.to_string(),
        parent_path: None,
        name: name.to_string(),
        extension: None,
        is_directory: false,
        size,
        modified_at_ms: None,
        created_at_ms: None,
        accessed_at_ms: None,
        attributes: 0,
        is_reparse_point: false,
        is_placeholder: false,
        is_system: false,
    }
}

fn open_db() -> (tempfile::TempDir, Database) {
    let temp = tempfile::tempdir().expect("temp dir");
    let db = Database::open(&temp.path().join("index.sqlite3")).expect("open db");
    db.migrate().expect("migrate db");
    (temp, db)
}

#[test]
fn duplicate_keys_exclude_protected_windows_directories_with_native_separators() {
    let (_temp, db) = open_db();
    let run_id = db
        .create_scan_run("C:\\", Some("NTFS"), ScanMode::Recursive, None, None, 100)
        .expect("create scan run");
    let size = 500 * 1024 * 1024;
    db.insert_file_entries(
        run_id,
        &[
            // Eligible: same name+size in two user folders (native and mixed separators).
            file_entry("C:\\Users\\me\\Videos\\clip.mp4", "clip.mp4", size),
            file_entry("C:/Users/me/Backup/clip.mp4", "clip.mp4", size),
            // Protected directories must never form a duplicate group.
            file_entry("C:\\Windows\\Temp\\same.bin", "same.bin", size),
            file_entry("C:\\Users\\me\\Downloads\\same.bin", "same.bin", size),
            file_entry("C:\\Program Files\\App\\lib.dll", "lib.dll", size),
            file_entry("C:\\Program Files (x86)\\App\\lib.dll", "lib.dll", size),
            file_entry("C:\\ProgramData\\Pkg\\data.pak", "data.pak", size),
            file_entry("C:\\Users\\me\\AppData\\Local\\Pkg\\data.pak", "data.pak", size),
        ],
    )
    .expect("insert entries");

    let keys = db.duplicate_name_size_keys(run_id, 1).expect("duplicate keys");

    let expected: HashSet<(String, u64)> = [("clip.mp4".to_string(), size)].into_iter().collect();
    assert_eq!(keys, expected);
}

#[test]
fn ecoqos_ownership_follows_the_latest_completed_action_per_process() {
    let (_temp, db) = open_db();
    let audit = |target: &str, action: &str, status: &str| {
        db.insert_optimizer_audit("process", target, action, status, None, None)
            .expect("insert audit");
    };

    audit("Updater.exe (10)", "ecoqos:on", "completed");
    audit("Indexer.exe (11)", "ecoqos:on", "completed");
    audit("Indexer.exe (11)", "ecoqos:off", "completed"); // restored: no longer owned
    audit("Telemetry.exe (12)", "ecoqos:on", "failed"); // never applied
    audit("Crashpad.exe (13)", "ecoqos:on", "pending"); // not finished
    audit("Onedrive.exe (14)", "ecoqos:on", "completed");
    audit("OneDrive.exe (14)", "ecoqos:off", "completed"); // same process, different case
    audit("Sync.exe (15)", "ecoqos:off", "completed");
    audit("Sync.exe (15)", "ecoqos:on", "completed"); // re-throttled later: owned again
    // Other audit kinds never count.
    db.insert_optimizer_audit("service", "Updater.exe (99)", "ecoqos:on", "completed", None, None)
        .expect("insert audit");

    let owned: Vec<String> = db
        .ecoqos_owned_processes()
        .expect("owned processes")
        .into_iter()
        .map(|(target, _)| target)
        .collect();

    assert_eq!(owned, vec!["Updater.exe (10)".to_string(), "Sync.exe (15)".to_string()]);
}

#[test]
fn snapshot_payload_can_be_updated_after_apply() {
    let (_temp, db) = open_db();
    let id = db
        .insert_optimizer_snapshot("powerScheme", "Balanced", "{\"schemeGuid\":\"a\"}")
        .expect("insert snapshot");

    db.update_optimizer_snapshot_payload(id, "{\"schemeGuid\":\"a\",\"ecoQosEnabled\":[]}")
        .expect("update payload");

    let snapshot = db.get_optimizer_snapshot(id).expect("read").expect("exists");
    assert!(snapshot.payload_json.contains("ecoQosEnabled"));
}
