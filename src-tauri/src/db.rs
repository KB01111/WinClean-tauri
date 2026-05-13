use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Context;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::scan::{DirectoryRollup, IndexedEntry, ScanMode, ScanSummary};

#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopDirectory {
    pub path: String,
    pub total_size: u64,
    pub file_count: u64,
    pub directory_count: u64,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self {
            path: path.as_ref().to_path_buf(),
        })
    }

    pub fn migrate(&self) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            r#"
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
            CREATE INDEX IF NOT EXISTS idx_file_entries_run_extension
                ON file_entries(scan_run_id, extension);

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
            "#,
        )?;
        Ok(())
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

    pub fn latest_completed_run_id(&self) -> anyhow::Result<Option<i64>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM scan_runs WHERE status = 'completed' ORDER BY finished_at_ms DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        Ok(rows.next()?.map(|row| row.get::<_, i64>(0)).transpose()?)
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

    fn connect(&self) -> anyhow::Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }

        Connection::open(&self.path)
            .with_context(|| format!("open sqlite database {}", self.path.display()))
    }
}
