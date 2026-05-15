use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::scan::{DirectoryRollup, IndexedEntry, ScanMode, ScanSummary};

pub const CURRENT_SCHEMA_VERSION: &str = "202605130001_production_foundation";

#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRunRecord {
    pub id: i64,
    pub root_path: String,
    pub file_system: Option<String>,
    pub scanner_mode: String,
    pub attempted_mode: Option<String>,
    pub fallback_reason: Option<String>,
    pub status: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub files_indexed: u64,
    pub directories_indexed: u64,
    pub bytes_indexed: u64,
    pub access_denied_count: u64,
    pub skipped_reparse_count: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryChild {
    pub id: i64,
    pub path: String,
    pub parent_path: Option<String>,
    pub name: String,
    pub extension: Option<String>,
    pub is_directory: bool,
    pub size: u64,
    pub total_size: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub modified_at_ms: Option<i64>,
    pub is_reparse_point: bool,
    pub is_placeholder: bool,
    pub is_system: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResult {
    pub id: i64,
    pub path: String,
    pub parent_path: Option<String>,
    pub name: String,
    pub extension: Option<String>,
    pub size: u64,
    pub modified_at_ms: Option<i64>,
    pub is_reparse_point: bool,
    pub is_placeholder: bool,
    pub is_system: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopDirectory {
    pub path: String,
    pub total_size: u64,
    pub file_count: u64,
    pub directory_count: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionSummary {
    pub extension: Option<String>,
    pub file_count: u64,
    pub total_size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySummary {
    pub category: String,
    pub file_count: u64,
    pub total_size: u64,
}

#[derive(Clone, Debug)]
pub struct CleanupFileRecord {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub size: u64,
    pub modified_at_ms: Option<i64>,
    pub is_reparse_point: bool,
    pub is_placeholder: bool,
    pub is_system: bool,
}

#[derive(Clone, Debug)]
pub struct CleanupActionInsert {
    pub action_type: String,
    pub status: String,
    pub target_path: String,
    pub original_path: Option<String>,
    pub destination_path: Option<String>,
    pub scan_run_id: Option<i64>,
    pub explanation: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub undo_token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupActionRecord {
    pub id: i64,
    pub action_type: String,
    pub status: String,
    pub target_path: String,
    pub original_path: Option<String>,
    pub destination_path: Option<String>,
    pub scan_run_id: Option<i64>,
    pub explanation: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub undo_token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OffloadJobInsert {
    pub scan_run_id: i64,
    pub status: String,
    pub destination_label: String,
    pub total_items: u64,
    pub total_bytes: u64,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug)]
pub struct OffloadItemInsert {
    pub job_id: i64,
    pub entry_id: i64,
    pub candidate_id: String,
    pub source_path: String,
    pub size: u64,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffloadJobRecord {
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
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffloadItemRecord {
    pub id: i64,
    pub job_id: i64,
    pub entry_id: i64,
    pub candidate_id: String,
    pub source_path: String,
    pub size: u64,
    pub status: String,
    pub drive_file_id: Option<String>,
    pub drive_web_url: Option<String>,
    pub uploaded_bytes: u64,
    pub md5_checksum: Option<String>,
    pub error: Option<String>,
    pub recycle_status: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS scan_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                root_path TEXT NOT NULL,
                file_system TEXT,
                scanner_mode TEXT NOT NULL,
                attempted_mode TEXT,
                fallback_reason TEXT,
                status TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER,
                files_indexed INTEGER NOT NULL DEFAULT 0,
                directories_indexed INTEGER NOT NULL DEFAULT 0,
                bytes_indexed INTEGER NOT NULL DEFAULT 0,
                access_denied_count INTEGER NOT NULL DEFAULT 0,
                skipped_reparse_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );

            CREATE TABLE IF NOT EXISTS file_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scan_run_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                parent_path TEXT,
                name TEXT NOT NULL,
                extension TEXT,
                is_directory INTEGER NOT NULL,
                size INTEGER NOT NULL,
                modified_at_ms INTEGER,
                created_at_ms INTEGER,
                accessed_at_ms INTEGER,
                attributes INTEGER NOT NULL,
                is_reparse_point INTEGER NOT NULL,
                is_placeholder INTEGER NOT NULL,
                is_system INTEGER NOT NULL,
                ntfs_file_id TEXT,
                usn INTEGER,
                FOREIGN KEY (scan_run_id) REFERENCES scan_runs(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_file_entries_run_path
                ON file_entries(scan_run_id, path);
            CREATE INDEX IF NOT EXISTS idx_file_entries_run_parent
                ON file_entries(scan_run_id, parent_path);
            DROP INDEX IF EXISTS idx_file_entries_run_parent_size;
            CREATE INDEX IF NOT EXISTS idx_file_entries_run_parent_lookup
                ON file_entries(scan_run_id, parent_path, is_directory, name);
            CREATE INDEX IF NOT EXISTS idx_file_entries_run_extension
                ON file_entries(scan_run_id, extension);
            CREATE INDEX IF NOT EXISTS idx_file_entries_run_name
                ON file_entries(scan_run_id, name);
            CREATE INDEX IF NOT EXISTS idx_file_entries_run_file_size
                ON file_entries(scan_run_id, is_directory, size DESC);

            CREATE TABLE IF NOT EXISTS directory_rollups (
                scan_run_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                total_size INTEGER NOT NULL,
                file_count INTEGER NOT NULL,
                directory_count INTEGER NOT NULL,
                PRIMARY KEY (scan_run_id, path),
                FOREIGN KEY (scan_run_id) REFERENCES scan_runs(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_directory_rollups_run_size
                ON directory_rollups(scan_run_id, total_size DESC);

            CREATE TABLE IF NOT EXISTS action_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action_type TEXT NOT NULL,
                status TEXT NOT NULL,
                target_path TEXT NOT NULL,
                original_path TEXT,
                destination_path TEXT,
                scan_run_id INTEGER,
                explanation TEXT,
                created_at_ms INTEGER NOT NULL,
                completed_at_ms INTEGER,
                undo_token TEXT,
                FOREIGN KEY (scan_run_id) REFERENCES scan_runs(id) ON DELETE SET NULL
            );

            CREATE INDEX IF NOT EXISTS idx_action_history_created
                ON action_history(created_at_ms DESC);
            CREATE INDEX IF NOT EXISTS idx_action_history_status
                ON action_history(status);

            CREATE TABLE IF NOT EXISTS offload_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scan_run_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                destination_label TEXT NOT NULL,
                drive_folder_id TEXT,
                total_items INTEGER NOT NULL,
                completed_items INTEGER NOT NULL DEFAULT 0,
                failed_items INTEGER NOT NULL DEFAULT 0,
                total_bytes INTEGER NOT NULL,
                uploaded_bytes INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                started_at_ms INTEGER,
                completed_at_ms INTEGER,
                last_error TEXT,
                FOREIGN KEY (scan_run_id) REFERENCES scan_runs(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_offload_jobs_created
                ON offload_jobs(created_at_ms DESC);
            CREATE INDEX IF NOT EXISTS idx_offload_jobs_status
                ON offload_jobs(status);

            CREATE TABLE IF NOT EXISTS offload_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                job_id INTEGER NOT NULL,
                entry_id INTEGER NOT NULL,
                candidate_id TEXT NOT NULL,
                source_path TEXT NOT NULL,
                size INTEGER NOT NULL,
                status TEXT NOT NULL,
                drive_file_id TEXT,
                drive_web_url TEXT,
                uploaded_bytes INTEGER NOT NULL DEFAULT 0,
                md5_checksum TEXT,
                error TEXT,
                recycle_status TEXT,
                created_at_ms INTEGER NOT NULL,
                completed_at_ms INTEGER,
                FOREIGN KEY (job_id) REFERENCES offload_jobs(id) ON DELETE CASCADE,
                UNIQUE(job_id, candidate_id)
            );

            CREATE INDEX IF NOT EXISTS idx_offload_items_job
                ON offload_items(job_id, status);
            "#,
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version, applied_at_ms) VALUES (?1, ?2)",
            params![CURRENT_SCHEMA_VERSION, unix_millis()],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> anyhow::Result<Option<String>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT version FROM schema_migrations ORDER BY applied_at_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("read schema version")
    }

    pub fn table_names(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read sqlite table names")
    }

    pub fn create_scan_run(
        &self,
        root_path: &str,
        file_system: Option<&str>,
        mode: ScanMode,
        attempted_mode: Option<ScanMode>,
        fallback_reason: Option<&str>,
        started_at_ms: i64,
    ) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            INSERT INTO scan_runs (
                root_path,
                file_system,
                scanner_mode,
                attempted_mode,
                fallback_reason,
                status,
                started_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6)
            "#,
            params![
                root_path,
                file_system,
                mode.as_str(),
                attempted_mode.map(|mode| mode.as_str()),
                fallback_reason,
                started_at_ms
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_scan_run(
        &self,
        summary: &ScanSummary,
        status: &str,
        last_error: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            UPDATE scan_runs
            SET status = ?2,
                finished_at_ms = ?3,
                files_indexed = ?4,
                directories_indexed = ?5,
                bytes_indexed = ?6,
                access_denied_count = ?7,
                skipped_reparse_count = ?8,
                error_count = ?9,
                last_error = ?10
            WHERE id = ?1
            "#,
            params![
                summary.run_id,
                status,
                summary.finished_at_ms,
                summary.files_indexed,
                summary.directories_indexed,
                summary.bytes_indexed,
                summary.access_denied_count,
                summary.skipped_reparse_count,
                summary.error_count,
                last_error
            ],
        )?;
        Ok(())
    }

    pub fn insert_file_entries(&self, run_id: i64, entries: &[IndexedEntry]) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO file_entries (
                    scan_run_id,
                    path,
                    parent_path,
                    name,
                    extension,
                    is_directory,
                    size,
                    modified_at_ms,
                    created_at_ms,
                    accessed_at_ms,
                    attributes,
                    is_reparse_point,
                    is_placeholder,
                    is_system,
                    ntfs_file_id,
                    usn
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                "#,
            )?;

            for entry in entries {
                stmt.execute(params![
                    run_id,
                    entry.path,
                    entry.parent_path,
                    entry.name,
                    entry.extension,
                    entry.is_directory,
                    entry.size,
                    entry.modified_at_ms,
                    entry.created_at_ms,
                    entry.accessed_at_ms,
                    entry.attributes,
                    entry.is_reparse_point,
                    entry.is_placeholder,
                    entry.is_system,
                    Option::<String>::None,
                    Option::<i64>::None
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn replace_directory_rollups(
        &self,
        run_id: i64,
        rollups: &HashMap<PathBuf, DirectoryRollup>,
    ) -> anyhow::Result<()> {
        let mut conn = self.connect()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM directory_rollups WHERE scan_run_id = ?1",
            params![run_id],
        )?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO directory_rollups (
                    scan_run_id,
                    path,
                    total_size,
                    file_count,
                    directory_count
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )?;

            for (path, rollup) in rollups {
                stmt.execute(params![
                    run_id,
                    path.to_string_lossy(),
                    rollup.total_size,
                    rollup.file_count,
                    rollup.directory_count
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_scan_runs(&self, limit: u32) -> anyhow::Result<Vec<ScanRunRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, root_path, file_system, scanner_mode, attempted_mode, fallback_reason,
                   status, started_at_ms, finished_at_ms, files_indexed, directories_indexed,
                   bytes_indexed, access_denied_count, skipped_reparse_count, error_count,
                   last_error
            FROM scan_runs
            ORDER BY started_at_ms DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit.clamp(1, 200)], scan_run_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read scan runs")
    }

    pub fn scan_run(&self, run_id: i64) -> anyhow::Result<Option<ScanRunRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            r#"
            SELECT id, root_path, file_system, scanner_mode, attempted_mode, fallback_reason,
                   status, started_at_ms, finished_at_ms, files_indexed, directories_indexed,
                   bytes_indexed, access_denied_count, skipped_reparse_count, error_count,
                   last_error
            FROM scan_runs
            WHERE id = ?1
            "#,
            params![run_id],
            scan_run_from_row,
        )
        .optional()
        .context("read scan run")
    }

    pub fn latest_completed_run_id(&self) -> anyhow::Result<Option<i64>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM scan_runs WHERE status = 'completed' ORDER BY finished_at_ms DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        Ok(rows.next()?.map(|row| row.get::<_, i64>(0)).transpose()?)
    }

    pub fn directory_children(
        &self,
        run_id: i64,
        parent_path: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<DirectoryChild>> {
        let parent_path = match parent_path {
            Some(path) => path.to_string(),
            None => self
                .scan_run(run_id)?
                .map(|run| run.root_path)
                .context("scan run not found")?,
        };
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT fe.id, fe.path, fe.parent_path, fe.name, fe.extension, fe.is_directory,
                   fe.size, COALESCE(dr.total_size, fe.size), COALESCE(dr.file_count, 0),
                   COALESCE(dr.directory_count, 0), fe.modified_at_ms,
                   fe.is_reparse_point, fe.is_placeholder, fe.is_system
            FROM file_entries fe
            LEFT JOIN directory_rollups dr
                ON dr.scan_run_id = fe.scan_run_id AND dr.path = fe.path
            WHERE fe.scan_run_id = ?1 AND fe.parent_path = ?2
            ORDER BY fe.is_directory DESC, COALESCE(dr.total_size, fe.size) DESC, fe.name ASC
            LIMIT ?3
            "#,
        )?;
        let rows = stmt.query_map(params![run_id, parent_path, limit.clamp(1, 500)], |row| {
            Ok(DirectoryChild {
                id: row.get(0)?,
                path: row.get(1)?,
                parent_path: row.get(2)?,
                name: row.get(3)?,
                extension: row.get(4)?,
                is_directory: row.get::<_, bool>(5)?,
                size: row.get::<_, i64>(6)? as u64,
                total_size: row.get::<_, i64>(7)? as u64,
                file_count: row.get::<_, i64>(8)? as u64,
                directory_count: row.get::<_, i64>(9)? as u64,
                modified_at_ms: row.get(10)?,
                is_reparse_point: row.get::<_, bool>(11)?,
                is_placeholder: row.get::<_, bool>(12)?,
                is_system: row.get::<_, bool>(13)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read directory children")
    }

    pub fn file_search(
        &self,
        run_id: i64,
        query: Option<&str>,
        extension: Option<&str>,
        min_size: Option<u64>,
        limit: u32,
    ) -> anyhow::Result<Vec<FileSearchResult>> {
        let query = format!("%{}%", query.unwrap_or_default());
        let extension = extension
            .map(|value| value.trim().trim_start_matches('.').to_lowercase())
            .filter(|value| !value.is_empty());
        let min_size = min_size.unwrap_or(0);
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, path, parent_path, name, extension, size, modified_at_ms,
                   is_reparse_point, is_placeholder, is_system
            FROM file_entries
            WHERE scan_run_id = ?1
              AND is_directory = 0
              AND (?2 IS NULL OR LOWER(extension) = ?2)
              AND size >= ?3
              AND (?4 = '%%' OR name LIKE ?4 OR path LIKE ?4)
            ORDER BY size DESC, path ASC
            LIMIT ?5
            "#,
        )?;
        let rows = stmt.query_map(
            params![
                run_id,
                extension,
                min_size as i64,
                query,
                limit.clamp(1, 1000)
            ],
            |row| {
                Ok(FileSearchResult {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    parent_path: row.get(2)?,
                    name: row.get(3)?,
                    extension: row.get(4)?,
                    size: row.get::<_, i64>(5)? as u64,
                    modified_at_ms: row.get(6)?,
                    is_reparse_point: row.get::<_, bool>(7)?,
                    is_placeholder: row.get::<_, bool>(8)?,
                    is_system: row.get::<_, bool>(9)?,
                })
            },
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("search files")
    }

    pub fn top_directories(&self, run_id: i64, limit: u32) -> anyhow::Result<Vec<TopDirectory>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT path, total_size, file_count, directory_count
            FROM directory_rollups
            WHERE scan_run_id = ?1
            ORDER BY total_size DESC, path ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![run_id, limit], |row| {
            Ok(TopDirectory {
                path: row.get(0)?,
                total_size: row.get::<_, i64>(1)? as u64,
                file_count: row.get::<_, i64>(2)? as u64,
                directory_count: row.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read top directory rollups")
    }

    pub fn extension_summary(
        &self,
        run_id: i64,
        limit: u32,
    ) -> anyhow::Result<Vec<ExtensionSummary>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT NULLIF(LOWER(COALESCE(extension, '')), '') AS ext,
                   COUNT(*) AS file_count,
                   COALESCE(SUM(size), 0) AS total_size
            FROM file_entries
            WHERE scan_run_id = ?1 AND is_directory = 0
            GROUP BY ext
            ORDER BY total_size DESC, file_count DESC, ext ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![run_id, limit.clamp(1, 200)], |row| {
            Ok(ExtensionSummary {
                extension: row.get(0)?,
                file_count: row.get::<_, i64>(1)? as u64,
                total_size: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read extension summary")
    }

    pub fn category_summary(&self, run_id: i64) -> anyhow::Result<Vec<CategorySummary>> {
        let extensions = self.extension_summary(run_id, 500)?;
        let mut categories: HashMap<String, CategorySummary> = HashMap::new();
        for extension in extensions {
            let category = category_for_extension(extension.extension.as_deref()).to_string();
            let entry = categories
                .entry(category.clone())
                .or_insert(CategorySummary {
                    category,
                    file_count: 0,
                    total_size: 0,
                });
            entry.file_count += extension.file_count;
            entry.total_size += extension.total_size;
        }
        let mut rows = categories.into_values().collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .total_size
                .cmp(&left.total_size)
                .then_with(|| left.category.cmp(&right.category))
        });
        Ok(rows)
    }

    pub fn cleanup_file_records(
        &self,
        run_id: i64,
        limit: u32,
    ) -> anyhow::Result<Vec<CleanupFileRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, path, name, extension, size, modified_at_ms,
                   is_reparse_point, is_placeholder, is_system
            FROM file_entries
            WHERE scan_run_id = ?1
              AND is_directory = 0
              AND size > 0
            ORDER BY size DESC, path ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![run_id, limit.clamp(1, 5000)], |row| {
            Ok(CleanupFileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                extension: row.get(3)?,
                size: row.get::<_, i64>(4)? as u64,
                modified_at_ms: row.get(5)?,
                is_reparse_point: row.get::<_, bool>(6)?,
                is_placeholder: row.get::<_, bool>(7)?,
                is_system: row.get::<_, bool>(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read cleanup candidate files")
    }

    pub fn recommendation_file_records(
        &self,
        run_id: i64,
        limit: u32,
    ) -> anyhow::Result<Vec<CleanupFileRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, path, name, extension, size, modified_at_ms,
                   is_reparse_point, is_placeholder, is_system
            FROM file_entries
            WHERE scan_run_id = ?1
              AND is_directory = 0
              AND size > 0
            ORDER BY size DESC, path ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![run_id, limit.clamp(1, 20_000)], |row| {
            Ok(CleanupFileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                extension: row.get(3)?,
                size: row.get::<_, i64>(4)? as u64,
                modified_at_ms: row.get(5)?,
                is_reparse_point: row.get::<_, bool>(6)?,
                is_placeholder: row.get::<_, bool>(7)?,
                is_system: row.get::<_, bool>(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read recommendation files")
    }

    pub fn record_cleanup_action(&self, action: CleanupActionInsert) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            INSERT INTO action_history (
                action_type,
                status,
                target_path,
                original_path,
                destination_path,
                scan_run_id,
                explanation,
                created_at_ms,
                completed_at_ms,
                undo_token
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                action.action_type,
                action.status,
                action.target_path,
                action.original_path,
                action.destination_path,
                action.scan_run_id,
                action.explanation,
                action.created_at_ms,
                action.completed_at_ms,
                action.undo_token
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_cleanup_action(
        &self,
        id: i64,
        status: &str,
        completed_at_ms: Option<i64>,
        explanation: Option<&str>,
        undo_token: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            UPDATE action_history
            SET status = ?2,
                completed_at_ms = ?3,
                explanation = COALESCE(?4, explanation),
                undo_token = COALESCE(?5, undo_token)
            WHERE id = ?1
            "#,
            params![id, status, completed_at_ms, explanation, undo_token],
        )?;
        Ok(())
    }

    pub fn cleanup_history(&self, limit: u32) -> anyhow::Result<Vec<CleanupActionRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, action_type, status, target_path, original_path, destination_path,
                   scan_run_id, explanation, created_at_ms, completed_at_ms, undo_token
            FROM action_history
            ORDER BY created_at_ms DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit.clamp(1, 500)], cleanup_action_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read cleanup history")
    }

    pub fn cleanup_action(&self, id: i64) -> anyhow::Result<Option<CleanupActionRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            r#"
            SELECT id, action_type, status, target_path, original_path, destination_path,
                   scan_run_id, explanation, created_at_ms, completed_at_ms, undo_token
            FROM action_history
            WHERE id = ?1
            "#,
            params![id],
            cleanup_action_from_row,
        )
        .optional()
        .context("read cleanup action")
    }

    pub fn create_offload_job(&self, job: OffloadJobInsert) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            INSERT INTO offload_jobs (
                scan_run_id,
                status,
                destination_label,
                total_items,
                total_bytes,
                created_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                job.scan_run_id,
                job.status,
                job.destination_label,
                job.total_items as i64,
                job.total_bytes as i64,
                job.created_at_ms
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn insert_offload_item(&self, item: OffloadItemInsert) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            INSERT INTO offload_items (
                job_id,
                entry_id,
                candidate_id,
                source_path,
                size,
                status,
                created_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6)
            "#,
            params![
                item.job_id,
                item.entry_id,
                item.candidate_id,
                item.source_path,
                item.size as i64,
                item.created_at_ms
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_offload_job(
        &self,
        job_id: i64,
        status: &str,
        drive_folder_id: Option<&str>,
        started_at_ms: Option<i64>,
        completed_at_ms: Option<i64>,
        last_error: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            UPDATE offload_jobs
            SET status = ?2,
                drive_folder_id = COALESCE(?3, drive_folder_id),
                started_at_ms = COALESCE(?4, started_at_ms),
                completed_at_ms = ?5,
                last_error = ?6
            WHERE id = ?1
            "#,
            params![
                job_id,
                status,
                drive_folder_id,
                started_at_ms,
                completed_at_ms,
                last_error
            ],
        )?;
        self.recalculate_offload_job(job_id)
    }

    pub fn update_offload_item(
        &self,
        item_id: i64,
        status: &str,
        drive_file_id: Option<&str>,
        drive_web_url: Option<&str>,
        uploaded_bytes: u64,
        md5_checksum: Option<&str>,
        error: Option<&str>,
        recycle_status: Option<&str>,
        completed_at_ms: Option<i64>,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            UPDATE offload_items
            SET status = ?2,
                drive_file_id = COALESCE(?3, drive_file_id),
                drive_web_url = COALESCE(?4, drive_web_url),
                uploaded_bytes = ?5,
                md5_checksum = COALESCE(?6, md5_checksum),
                error = ?7,
                recycle_status = COALESCE(?8, recycle_status),
                completed_at_ms = ?9
            WHERE id = ?1
            "#,
            params![
                item_id,
                status,
                drive_file_id,
                drive_web_url,
                uploaded_bytes as i64,
                md5_checksum,
                error,
                recycle_status,
                completed_at_ms
            ],
        )?;
        let job_id = conn.query_row(
            "SELECT job_id FROM offload_items WHERE id = ?1",
            params![item_id],
            |row| row.get::<_, i64>(0),
        )?;
        self.recalculate_offload_job(job_id)
    }

    pub fn offload_job(&self, job_id: i64) -> anyhow::Result<Option<OffloadJobRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            r#"
            SELECT id, scan_run_id, status, destination_label, drive_folder_id,
                   total_items, completed_items, failed_items, total_bytes,
                   uploaded_bytes, created_at_ms, started_at_ms, completed_at_ms, last_error
            FROM offload_jobs
            WHERE id = ?1
            "#,
            params![job_id],
            offload_job_from_row,
        )
        .optional()
        .context("read offload job")
    }

    pub fn offload_items(&self, job_id: i64) -> anyhow::Result<Vec<OffloadItemRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, job_id, entry_id, candidate_id, source_path, size, status,
                   drive_file_id, drive_web_url, uploaded_bytes, md5_checksum,
                   error, recycle_status, created_at_ms, completed_at_ms
            FROM offload_items
            WHERE job_id = ?1
            ORDER BY id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![job_id], offload_item_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read offload items")
    }

    pub fn pending_offload_count(&self) -> anyhow::Result<u64> {
        let conn = self.connect()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM offload_jobs WHERE status IN ('queued', 'running')",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count as u64)
    }

    fn recalculate_offload_job(&self, job_id: i64) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            r#"
            UPDATE offload_jobs
            SET completed_items = (
                    SELECT COUNT(*) FROM offload_items
                    WHERE job_id = ?1 AND status = 'completed'
                ),
                failed_items = (
                    SELECT COUNT(*) FROM offload_items
                    WHERE job_id = ?1 AND status IN ('failed', 'skipped')
                ),
                uploaded_bytes = (
                    SELECT COALESCE(SUM(uploaded_bytes), 0) FROM offload_items
                    WHERE job_id = ?1
                )
            WHERE id = ?1
            "#,
            params![job_id],
        )?;
        Ok(())
    }

    pub fn pending_action_count(&self) -> anyhow::Result<u64> {
        let conn = self.connect()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM action_history WHERE status IN ('planned', 'running', 'restore_requested')",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count as u64)
    }

    fn connect(&self) -> anyhow::Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }

        let conn = Connection::open(&self.path)
            .with_context(|| format!("open sqlite database {}", self.path.display()))?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(conn)
    }
}

fn scan_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScanRunRecord> {
    Ok(ScanRunRecord {
        id: row.get(0)?,
        root_path: row.get(1)?,
        file_system: row.get(2)?,
        scanner_mode: row.get(3)?,
        attempted_mode: row.get(4)?,
        fallback_reason: row.get(5)?,
        status: row.get(6)?,
        started_at_ms: row.get(7)?,
        finished_at_ms: row.get(8)?,
        files_indexed: row.get::<_, i64>(9)? as u64,
        directories_indexed: row.get::<_, i64>(10)? as u64,
        bytes_indexed: row.get::<_, i64>(11)? as u64,
        access_denied_count: row.get::<_, i64>(12)? as u64,
        skipped_reparse_count: row.get::<_, i64>(13)? as u64,
        error_count: row.get::<_, i64>(14)? as u64,
        last_error: row.get(15)?,
    })
}

fn cleanup_action_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CleanupActionRecord> {
    Ok(CleanupActionRecord {
        id: row.get(0)?,
        action_type: row.get(1)?,
        status: row.get(2)?,
        target_path: row.get(3)?,
        original_path: row.get(4)?,
        destination_path: row.get(5)?,
        scan_run_id: row.get(6)?,
        explanation: row.get(7)?,
        created_at_ms: row.get(8)?,
        completed_at_ms: row.get(9)?,
        undo_token: row.get(10)?,
    })
}

fn offload_job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OffloadJobRecord> {
    Ok(OffloadJobRecord {
        id: row.get(0)?,
        scan_run_id: row.get(1)?,
        status: row.get(2)?,
        destination_label: row.get(3)?,
        drive_folder_id: row.get(4)?,
        total_items: row.get::<_, i64>(5)? as u64,
        completed_items: row.get::<_, i64>(6)? as u64,
        failed_items: row.get::<_, i64>(7)? as u64,
        total_bytes: row.get::<_, i64>(8)? as u64,
        uploaded_bytes: row.get::<_, i64>(9)? as u64,
        created_at_ms: row.get(10)?,
        started_at_ms: row.get(11)?,
        completed_at_ms: row.get(12)?,
        last_error: row.get(13)?,
    })
}

fn offload_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OffloadItemRecord> {
    Ok(OffloadItemRecord {
        id: row.get(0)?,
        job_id: row.get(1)?,
        entry_id: row.get(2)?,
        candidate_id: row.get(3)?,
        source_path: row.get(4)?,
        size: row.get::<_, i64>(5)? as u64,
        status: row.get(6)?,
        drive_file_id: row.get(7)?,
        drive_web_url: row.get(8)?,
        uploaded_bytes: row.get::<_, i64>(9)? as u64,
        md5_checksum: row.get(10)?,
        error: row.get(11)?,
        recycle_status: row.get(12)?,
        created_at_ms: row.get(13)?,
        completed_at_ms: row.get(14)?,
    })
}

fn category_for_extension(extension: Option<&str>) -> &'static str {
    match extension.unwrap_or_default().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic" | "tif" | "tiff" => "Images",
        "mp4" | "mov" | "mkv" | "avi" | "wmv" | "webm" => "Videos",
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" => "Audio",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "rtf" | "csv" => {
            "Documents"
        }
        "zip" | "7z" | "rar" | "tar" | "gz" | "iso" => "Archives",
        "exe" | "msi" | "dll" | "sys" | "com" => "Applications",
        "tmp" | "temp" | "log" | "bak" | "old" | "dmp" => "Cleanup candidates",
        "" => "No extension",
        _ => "Other",
    }
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
