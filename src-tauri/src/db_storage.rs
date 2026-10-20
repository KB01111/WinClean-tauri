//! Storage-assistant queries on the scan index: path lookups for correlating
//! search results, possible-duplicate groups, and remembered content hashes.

use std::collections::HashMap;

use anyhow::Context;
use rusqlite::{params, OptionalExtension};

use crate::db::{Database, FileSearchResult};

pub const STORAGE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS duplicate_hashes (
    scan_run_id INTEGER NOT NULL,
    entry_id INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_at_ms INTEGER,
    verified_at_ms INTEGER NOT NULL,
    PRIMARY KEY (scan_run_id, entry_id),
    FOREIGN KEY (scan_run_id) REFERENCES scan_runs(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS duplicate_heads (
    scan_run_id INTEGER NOT NULL,
    entry_id INTEGER NOT NULL,
    head_sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified_at_ms INTEGER,
    PRIMARY KEY (scan_run_id, entry_id),
    FOREIGN KEY (scan_run_id) REFERENCES scan_runs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_duplicate_hashes_hash ON duplicate_hashes(scan_run_id, sha256, size);
"#;

/// Files that share a size, i.e. *possible* duplicates until their content is compared.
#[derive(Clone, Debug)]
pub struct SizeGroup {
    pub size: u64,
    pub files: Vec<FileSearchResult>,
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileSearchResult> {
    Ok(FileSearchResult {
        id: row.get(0)?,
        path: row.get(1)?,
        parent_path: row.get(2)?,
        name: row.get(3)?,
        extension: row.get(4)?,
        size: row.get::<_, i64>(5)?.max(0) as u64,
        modified_at_ms: row.get(6)?,
        is_reparse_point: row.get::<_, bool>(7)?,
        is_placeholder: row.get::<_, bool>(8)?,
        is_system: row.get::<_, bool>(9)?,
    })
}

const ENTRY_COLUMNS: &str = "id, path, parent_path, name, extension, size, modified_at_ms, is_reparse_point, is_placeholder, is_system";

impl Database {
    /// The indexed file at exactly this path (case-insensitive), if the scan saw it.
    pub fn file_entry_by_path(&self, run_id: i64, path: &str) -> anyhow::Result<Option<FileSearchResult>> {
        let conn = self.connect()?;
        let sql = format!("SELECT {ENTRY_COLUMNS} FROM file_entries WHERE scan_run_id = ?1 AND is_directory = 0 AND path = ?2 LIMIT 1");
        if let Some(found) = conn.query_row(&sql, params![run_id, path], row_to_entry).optional()? {
            return Ok(Some(found));
        }
        // Windows paths are case-insensitive; the exact lookup above uses the index, this one is the fallback.
        let sql = format!("SELECT {ENTRY_COLUMNS} FROM file_entries WHERE scan_run_id = ?1 AND is_directory = 0 AND path = ?2 COLLATE NOCASE LIMIT 1");
        conn.query_row(&sql, params![run_id, path.replace('/', "\\")], row_to_entry).optional().context("look up a file by path")
    }

    /// Groups of ordinary files with identical size (≥ `min_size`), biggest potential saving first.
    pub fn duplicate_size_groups(&self, run_id: i64, min_size: u64, max_groups: u32) -> anyhow::Result<Vec<SizeGroup>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT size FROM file_entries
            WHERE scan_run_id = ?1 AND is_directory = 0 AND size >= ?2
              AND is_reparse_point = 0 AND is_placeholder = 0 AND is_system = 0
            GROUP BY size HAVING COUNT(*) > 1
            ORDER BY size * (COUNT(*) - 1) DESC
            LIMIT ?3
            "#,
        )?;
        let sizes: Vec<i64> = stmt.query_map(params![run_id, crate::db_storage::clamp(min_size), max_groups.clamp(1, 500)], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let sql = format!(
            "SELECT {ENTRY_COLUMNS} FROM file_entries WHERE scan_run_id = ?1 AND is_directory = 0 AND size = ?2 AND is_reparse_point = 0 AND is_placeholder = 0 AND is_system = 0 ORDER BY path LIMIT 50"
        );
        let mut files_stmt = conn.prepare(&sql)?;
        let mut groups = Vec::new();
        for size in sizes {
            let files: Vec<FileSearchResult> = files_stmt.query_map(params![run_id, size], row_to_entry)?.collect::<rusqlite::Result<_>>()?;
            groups.push(SizeGroup { size: size.max(0) as u64, files });
        }
        Ok(groups)
    }

    pub fn save_duplicate_hash(&self, run_id: i64, entry_id: i64, sha256: &str, size: u64, modified_at_ms: Option<i64>, now_ms: i64) -> anyhow::Result<()> {
        self.connect()?
            .execute(
                "INSERT OR REPLACE INTO duplicate_hashes (scan_run_id, entry_id, sha256, size, modified_at_ms, verified_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![run_id, entry_id, sha256, clamp(size), modified_at_ms, now_ms],
            )
            .context("save a duplicate hash")?;
        Ok(())
    }

    pub fn save_duplicate_head(&self, run_id: i64, entry_id: i64, head_sha256: &str, size: u64, modified_at_ms: Option<i64>) -> anyhow::Result<()> {
        self.connect()?
            .execute(
                "INSERT OR REPLACE INTO duplicate_heads (scan_run_id, entry_id, head_sha256, size, modified_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, entry_id, head_sha256, clamp(size), modified_at_ms],
            )
            .context("save a duplicate prefix hash")?;
        Ok(())
    }

    /// Remembered prefix hashes that still describe the indexed file: entry id → SHA-256 of its first bytes.
    pub fn valid_heads(&self, run_id: i64) -> anyhow::Result<HashMap<i64, String>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT h.entry_id, h.head_sha256
            FROM duplicate_heads h
            JOIN file_entries e ON e.id = h.entry_id AND e.scan_run_id = h.scan_run_id
             AND e.size = h.size AND COALESCE(e.modified_at_ms, -1) = COALESCE(h.modified_at_ms, -1)
            WHERE h.scan_run_id = ?1
            "#,
        )?;
        let rows = stmt.query_map(params![run_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().collect())
    }

    /// Remembered hashes that still describe the indexed file (same size and
    /// modification time): entry id → SHA-256.
    pub fn valid_hashes(&self, run_id: i64) -> anyhow::Result<HashMap<i64, String>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT h.entry_id, h.sha256
            FROM duplicate_hashes h
            JOIN file_entries e ON e.id = h.entry_id AND e.scan_run_id = h.scan_run_id
             AND e.size = h.size AND COALESCE(e.modified_at_ms, -1) = COALESCE(h.modified_at_ms, -1)
            WHERE h.scan_run_id = ?1
            "#,
        )?;
        let rows = stmt.query_map(params![run_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().collect())
    }

    /// Entries whose content hash (verified earlier, and still matching the
    /// indexed size and modification time) equals another entry's: entry id →
    /// (hash, number of identical files).
    pub fn confirmed_duplicates(&self, run_id: i64) -> anyhow::Result<HashMap<i64, (String, u32)>> {
        let conn = self.connect()?;
        // Only hashes whose file still has the indexed size and modification time count, on both sides of the comparison.
        let mut stmt = conn.prepare(
            r#"
            WITH valid AS (
                SELECT h.entry_id, h.sha256, h.size
                FROM duplicate_hashes h
                JOIN file_entries e ON e.id = h.entry_id AND e.scan_run_id = h.scan_run_id
                 AND e.size = h.size AND COALESCE(e.modified_at_ms, -1) = COALESCE(h.modified_at_ms, -1)
                WHERE h.scan_run_id = ?1
            )
            SELECT v.entry_id, v.sha256, c.n
            FROM valid v
            JOIN (SELECT sha256, size, COUNT(*) AS n FROM valid GROUP BY sha256, size HAVING COUNT(*) > 1) c
              ON c.sha256 = v.sha256 AND c.size = v.size
            "#,
        )?;
        let rows = stmt.query_map(params![run_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().map(|(id, h, n)| (id, (h, n))).collect())
    }
}

pub(crate) fn clamp(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Shared by the storage tests: a database holding one scan of a made-up volume.
#[cfg(test)]
pub(crate) mod testing {
    use crate::db::Database;
    use crate::scan::{IndexedEntry, ScanMode};

    pub const DAY_MS: i64 = 86_400_000;

    pub fn now_ms() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
    }

    pub fn file(path: &str, size: u64, age_days: i64) -> IndexedEntry {
        let name = path.rsplit('\\').next().unwrap().to_string();
        let extension = name.rsplit_once('.').map(|(_, e)| e.to_string());
        IndexedEntry {
            path: path.to_string(),
            parent_path: path.rsplit_once('\\').map(|(p, _)| p.to_string()),
            name,
            extension,
            is_directory: false,
            size,
            modified_at_ms: Some(now_ms() - age_days * DAY_MS),
            created_at_ms: None,
            accessed_at_ms: None,
            attributes: 0,
            is_reparse_point: false,
            is_placeholder: false,
            is_system: false,
        }
    }

    pub fn dir(path: &str) -> IndexedEntry {
        let mut e = file(path, 0, 400);
        e.is_directory = true;
        e.extension = None;
        e
    }

    /// Returns the temp dir (keep it alive), the database, and the scan run id.
    pub fn seeded(entries: Vec<IndexedEntry>) -> (tempfile::TempDir, Database, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        let run = db.create_scan_run("C:\\", Some("NTFS"), ScanMode::Recursive, None, None, now_ms()).unwrap();
        db.insert_file_entries(run, &entries).unwrap();
        (dir, db, run)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn sample() -> Vec<crate::scan::IndexedEntry> {
        vec![
            dir("C:\\Users\\me\\Downloads"),
            file("C:\\Users\\me\\Downloads\\A.iso", 5_000_000, 90),
            file("C:\\Users\\me\\Downloads\\copy of A.iso", 5_000_000, 90),
            file("C:\\Users\\me\\Desktop\\a.iso", 5_000_000, 10),
            file("C:\\Users\\me\\Downloads\\small.txt", 10, 1),
            file("C:\\Users\\me\\Downloads\\small2.txt", 10, 1),
            file("C:\\x\\unique.bin", 123_456, 1),
        ]
    }

    #[test]
    fn files_are_found_by_path_exactly_or_ignoring_case_and_slashes() {
        let (_d, db, run) = seeded(sample());
        assert_eq!(db.file_entry_by_path(run, "C:\\Users\\me\\Downloads\\A.iso").unwrap().unwrap().size, 5_000_000);
        assert!(db.file_entry_by_path(run, "c:/users/ME/downloads/a.ISO").unwrap().is_some());
        assert!(db.file_entry_by_path(run, "C:\\Users\\me\\Downloads").unwrap().is_none(), "directories are not files");
        assert!(db.file_entry_by_path(run, "C:\\nope").unwrap().is_none());
        assert!(db.file_entry_by_path(run + 1, "C:\\Users\\me\\Downloads\\A.iso").unwrap().is_none(), "another run");
    }

    #[test]
    fn size_groups_hold_every_same_size_file_biggest_saving_first() {
        let (_d, db, run) = seeded(sample());
        let groups = db.duplicate_size_groups(run, 1, 10).unwrap();
        assert_eq!(groups.iter().map(|g| (g.size, g.files.len())).collect::<Vec<_>>(), vec![(5_000_000, 3), (10, 2)]);
        assert_eq!(db.duplicate_size_groups(run, 1_000, 10).unwrap().len(), 1, "min size filters small groups");
        assert_eq!(db.duplicate_size_groups(run, 1, 1).unwrap().len(), 1);
    }

    #[test]
    fn confirmed_duplicates_need_equal_hashes_and_unchanged_files() {
        let (_d, db, run) = seeded(sample());
        let get = |p: &str| db.file_entry_by_path(run, p).unwrap().unwrap();
        let (a, copy, desk) = (get("C:\\Users\\me\\Downloads\\A.iso"), get("C:\\Users\\me\\Downloads\\copy of A.iso"), get("C:\\Users\\me\\Desktop\\a.iso"));
        db.save_duplicate_hash(run, a.id, "aa", a.size, a.modified_at_ms, 1).unwrap();
        assert!(db.confirmed_duplicates(run).unwrap().is_empty(), "one hash proves nothing");
        db.save_duplicate_hash(run, copy.id, "aa", copy.size, copy.modified_at_ms, 1).unwrap();
        db.save_duplicate_hash(run, desk.id, "bb", desk.size, desk.modified_at_ms, 1).unwrap();
        let confirmed = db.confirmed_duplicates(run).unwrap();
        assert_eq!(confirmed.len(), 2);
        assert_eq!(confirmed[&copy.id], ("aa".to_string(), 2));
        assert!(!confirmed.contains_key(&desk.id), "a different hash is not a duplicate of the others");
        // A hash recorded for a different modification time is stale and must not count.
        db.save_duplicate_hash(run, copy.id, "aa", copy.size, Some(1), 2).unwrap();
        assert!(db.confirmed_duplicates(run).unwrap().is_empty());
    }

    #[test]
    fn a_read_only_handle_can_read_but_never_write() {
        let (d, db, run) = seeded(sample());
        let ro = Database::open_read_only(d.path().join("t.sqlite3")).unwrap();
        assert_eq!(ro.file_search(run, Some("small"), None, None, 10).unwrap().len(), 2);
        assert!(ro.save_duplicate_hash(run, 1, "x", 1, None, 1).is_err());
        assert!(ro.delete_scan_run(run).is_err());
        assert!(ro.set_optimizer_setting("k", "\"v\"").is_err());
        assert!(ro.insert_optimizer_audit("t", "t", "t", "t", None, None).is_err());
        assert!(db.file_entry_by_path(run, "C:\\x\\unique.bin").unwrap().is_some(), "the data is untouched");
        assert!(Database::open_read_only(d.path().join("missing.sqlite3")).is_err(), "never creates a database");
    }
}
