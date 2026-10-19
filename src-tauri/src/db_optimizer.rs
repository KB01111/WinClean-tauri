//! SQLite persistence for the WinClean 2.0 optimizer: snapshots, audit trail,
//! power telemetry, settings and measurement sessions. Every table is created
//! with `IF NOT EXISTS`, so existing 1.x databases upgrade in place.

use anyhow::Context;
use rusqlite::{params, OptionalExtension};

use crate::db::{unix_millis, Database};
use crate::optimizer::battery::TelemetrySample;

/// Additive schema executed by `Database::migrate`.
pub(crate) const OPTIMIZER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS optimizer_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    rolled_back_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS optimizer_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL,
    detail TEXT,
    snapshot_id INTEGER,
    created_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    FOREIGN KEY (snapshot_id) REFERENCES optimizer_snapshots(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS power_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms INTEGER NOT NULL,
    percent INTEGER NOT NULL,
    on_ac INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS power_telemetry (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms INTEGER NOT NULL,
    percent INTEGER,
    on_ac INTEGER NOT NULL,
    rate_mw INTEGER,
    voltage_mv INTEGER,
    capacity_mwh INTEGER,
    profile_id TEXT,
    workload TEXT
);
CREATE INDEX IF NOT EXISTS idx_power_telemetry_at ON power_telemetry(at_ms);

CREATE TABLE IF NOT EXISTS optimizer_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS power_measurements (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    label TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    baseline_start_ms INTEGER NOT NULL,
    switch_ms INTEGER,
    ended_ms INTEGER,
    result_json TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_optimizer_audit_status ON optimizer_audit(status);
"#;

/// Telemetry older than this is pruned.
const TELEMETRY_RETENTION_MS: i64 = 14 * 24 * 3_600_000;
/// Hard cap on stored telemetry rows regardless of age.
const TELEMETRY_MAX_ROWS: i64 = 60_000;

/// A stored measurement session (see `optimizer::measure`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementRecord {
    pub id: i64,
    pub label: String,
    pub profile_id: String,
    pub baseline_start_ms: i64,
    pub switch_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    pub result_json: Option<String>,
    pub created_at_ms: i64,
}

impl Database {
    pub fn insert_optimizer_snapshot(&self, kind: &str, target: &str, payload_json: &str) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO optimizer_snapshots (kind, target, payload_json, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![kind, target, payload_json, unix_millis()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_optimizer_snapshot(&self, id: i64) -> anyhow::Result<Option<crate::optimizer::OptimizationSnapshot>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, kind, target, payload_json, created_at_ms, rolled_back_at_ms FROM optimizer_snapshots WHERE id = ?1",
            params![id],
            |row| Ok(crate::optimizer::OptimizationSnapshot {
                id: row.get(0)?, kind: row.get(1)?, target: row.get(2)?,
                payload_json: row.get(3)?, created_at_ms: row.get(4)?, rolled_back_at_ms: row.get(5)?,
            }),
        ).optional().context("read optimizer snapshot")
    }

    pub fn mark_snapshot_rolled_back(&self, id: i64) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE optimizer_snapshots SET rolled_back_at_ms = ?2 WHERE id = ?1", params![id, unix_millis()])?;
        Ok(())
    }

    /// Replaces a snapshot's payload (used to add the processes a power
    /// profile touched after the profile has been applied).
    pub fn update_optimizer_snapshot_payload(&self, id: i64, payload_json: &str) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE optimizer_snapshots SET payload_json = ?2 WHERE id = ?1", params![id, payload_json])?;
        Ok(())
    }

    /// Processes WinClean currently owns EcoQoS throttling for: audit
    /// targets whose latest completed EcoQoS action is `ecoqos:on`. Returns
    /// `(target, created_at_ms)` so callers can reject a reused PID.
    pub fn ecoqos_owned_processes(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT a.target, a.created_at_ms
            FROM optimizer_audit a
            WHERE a.kind = 'process' AND a.status = 'completed' AND a.action = 'ecoqos:on'
              AND a.id = (
                SELECT MAX(b.id) FROM optimizer_audit b
                WHERE b.kind = 'process' AND b.status = 'completed'
                  AND b.action IN ('ecoqos:on', 'ecoqos:off')
                  AND b.target = a.target COLLATE NOCASE
              )
            ORDER BY a.id
            "#,
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("read owned EcoQoS processes")
    }

    /// Completed per-process EcoQoS changes a power profile made, linked to
    /// its snapshot (`action` is `ecoqos:on` or `ecoqos:off`). Written as each
    /// process changes, so rollback does not depend on the single payload
    /// update that follows the loop.
    pub fn ecoqos_changes_for_snapshot(&self, snapshot_id: i64, action: &str) -> anyhow::Result<Vec<(String, i64)>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT target, created_at_ms FROM optimizer_audit WHERE kind = 'process' AND status = 'completed' AND action = ?2 AND snapshot_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![snapshot_id, action], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("read EcoQoS changes for snapshot")
    }

    pub fn insert_optimizer_audit(&self, kind: &str, target: &str, action: &str, status: &str, detail: Option<&str>, snapshot_id: Option<i64>) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        let done = if status == "pending" { None } else { Some(unix_millis()) };
        conn.execute(
            "INSERT INTO optimizer_audit (kind, target, action, status, detail, snapshot_id, created_at_ms, completed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![kind, target, action, status, detail, snapshot_id, unix_millis(), done],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Finalizes a pending audit row in place (no duplicate terminal row).
    pub fn complete_optimizer_audit(&self, id: i64, status: &str, detail: Option<&str>) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE optimizer_audit SET status = ?2, detail = ?3, completed_at_ms = ?4 WHERE id = ?1 AND status = 'pending'",
            params![id, status, detail, unix_millis()],
        )?;
        Ok(())
    }

    pub fn list_optimizer_audit(&self, limit: u32) -> anyhow::Result<Vec<crate::optimizer::OptimizationAudit>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, target, action, status, detail, snapshot_id, created_at_ms, completed_at_ms FROM optimizer_audit ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| Ok(crate::optimizer::OptimizationAudit {
            id: row.get(0)?, kind: row.get(1)?, target: row.get(2)?, action: row.get(3)?,
            status: row.get(4)?, detail: row.get(5)?, snapshot_id: row.get(6)?,
            created_at_ms: row.get(7)?, completed_at_ms: row.get(8)?,
        }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("list optimizer audit")
    }

    pub fn list_optimizer_snapshots(&self, limit: u32) -> anyhow::Result<Vec<crate::optimizer::OptimizationSnapshot>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, target, payload_json, created_at_ms, rolled_back_at_ms FROM optimizer_snapshots ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| Ok(crate::optimizer::OptimizationSnapshot {
            id: row.get(0)?, kind: row.get(1)?, target: row.get(2)?,
            payload_json: row.get(3)?, created_at_ms: row.get(4)?, rolled_back_at_ms: row.get(5)?,
        }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("list optimizer snapshots")
    }

    pub fn record_power_sample(&self, percent: u8, on_ac: bool) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute("INSERT INTO power_history (at_ms, percent, on_ac) VALUES (?1, ?2, ?3)", params![unix_millis(), percent as i64, on_ac as i64])?;
        // Keep the table bounded: last 2000 samples.
        conn.execute("DELETE FROM power_history WHERE id NOT IN (SELECT id FROM power_history ORDER BY id DESC LIMIT 2000)", [])?;
        Ok(())
    }

    pub fn power_history(&self, limit: u32) -> anyhow::Result<Vec<crate::optimizer::power::PowerSample>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare("SELECT at_ms, percent, on_ac FROM power_history ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit], |row| Ok(crate::optimizer::power::PowerSample {
            at_ms: row.get(0)?, percent: row.get::<_, i64>(1)? as u8, on_ac: row.get::<_, i64>(2)? != 0,
        }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("read power history")
    }

    // --- audit helpers used by crash recovery and history ---

    /// Audit rows that were started but never finalized (the app or helper
    /// died mid-operation).
    pub fn pending_optimizer_audit(&self) -> anyhow::Result<Vec<crate::optimizer::OptimizationAudit>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, target, action, status, detail, snapshot_id, created_at_ms, completed_at_ms FROM optimizer_audit WHERE status = 'pending' ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| Ok(crate::optimizer::OptimizationAudit {
            id: row.get(0)?, kind: row.get(1)?, target: row.get(2)?, action: row.get(3)?,
            status: row.get(4)?, detail: row.get(5)?, snapshot_id: row.get(6)?,
            created_at_ms: row.get(7)?, completed_at_ms: row.get(8)?,
        }))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("list pending optimizer audit")
    }

    /// Finalizes a pending row with an explicit status (used by recovery,
    /// which may need `interrupted` rather than `failed`).
    pub fn finalize_pending_audit(&self, id: i64, status: &str, detail: &str) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let n = conn.execute(
            "UPDATE optimizer_audit SET status = ?2, detail = ?3, completed_at_ms = ?4 WHERE id = ?1 AND status = 'pending'",
            params![id, status, detail, unix_millis()],
        )?;
        Ok(n > 0)
    }

    // --- telemetry ---

    pub fn record_power_telemetry(&self, sample: &TelemetrySample) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO power_telemetry (at_ms, percent, on_ac, rate_mw, voltage_mv, capacity_mwh, profile_id, workload) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                sample.at_ms,
                sample.percent.map(|p| p as i64),
                sample.on_ac as i64,
                sample.rate_mw,
                sample.voltage_mv,
                sample.capacity_mwh,
                sample.profile_id,
                sample.workload
            ],
        )?;
        // Cheap bounded retention: prune roughly every 200th insert.
        if conn.last_insert_rowid() % 200 == 0 {
            conn.execute("DELETE FROM power_telemetry WHERE at_ms < ?1", params![sample.at_ms - TELEMETRY_RETENTION_MS])?;
            conn.execute(
                "DELETE FROM power_telemetry WHERE id <= (SELECT MAX(id) FROM power_telemetry) - ?1",
                params![TELEMETRY_MAX_ROWS],
            )?;
        }
        Ok(())
    }

    /// Samples with `start_ms <= at_ms <= end_ms`, oldest first.
    pub fn power_telemetry_between(&self, start_ms: i64, end_ms: i64, limit: u32) -> anyhow::Result<Vec<TelemetrySample>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT at_ms, percent, on_ac, rate_mw, voltage_mv, capacity_mwh, profile_id, workload FROM power_telemetry WHERE at_ms >= ?1 AND at_ms <= ?2 ORDER BY at_ms LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![start_ms, end_ms, limit], telemetry_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("read power telemetry range")
    }

    /// The newest `limit` samples, returned oldest first.
    pub fn recent_power_telemetry(&self, limit: u32) -> anyhow::Result<Vec<TelemetrySample>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT at_ms, percent, on_ac, rate_mw, voltage_mv, capacity_mwh, profile_id, workload FROM power_telemetry ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], telemetry_from_row)?;
        let mut out = rows.collect::<rusqlite::Result<Vec<_>>>().context("read recent power telemetry")?;
        out.reverse();
        Ok(out)
    }

    pub fn power_telemetry_count(&self) -> anyhow::Result<u64> {
        let conn = self.connect()?;
        Ok(conn.query_row("SELECT COUNT(*) FROM power_telemetry", [], |row| row.get::<_, i64>(0))? as u64)
    }

    // --- settings ---

    pub fn get_optimizer_setting(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.connect()?;
        conn.query_row("SELECT value_json FROM optimizer_settings WHERE key = ?1", params![key], |row| row.get(0))
            .optional()
            .context("read optimizer setting")
    }

    pub fn set_optimizer_setting(&self, key: &str, value_json: &str) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO optimizer_settings (key, value_json, updated_at_ms) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
            params![key, value_json, unix_millis()],
        )?;
        Ok(())
    }

    // --- measurement sessions ---

    pub fn create_measurement(&self, label: &str, profile_id: &str, baseline_start_ms: i64) -> anyhow::Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO power_measurements (label, profile_id, baseline_start_ms, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![label, profile_id, baseline_start_ms, unix_millis()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_measurement(&self, id: i64, switch_ms: Option<i64>, ended_ms: Option<i64>, result_json: Option<&str>) -> anyhow::Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE power_measurements SET switch_ms = COALESCE(?2, switch_ms), ended_ms = COALESCE(?3, ended_ms), result_json = COALESCE(?4, result_json) WHERE id = ?1",
            params![id, switch_ms, ended_ms, result_json],
        )?;
        Ok(())
    }

    pub fn get_measurement(&self, id: i64) -> anyhow::Result<Option<MeasurementRecord>> {
        let conn = self.connect()?;
        conn.query_row(
            "SELECT id, label, profile_id, baseline_start_ms, switch_ms, ended_ms, result_json, created_at_ms FROM power_measurements WHERE id = ?1",
            params![id],
            measurement_from_row,
        ).optional().context("read measurement")
    }

    pub fn list_measurements(&self, limit: u32) -> anyhow::Result<Vec<MeasurementRecord>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, label, profile_id, baseline_start_ms, switch_ms, ended_ms, result_json, created_at_ms FROM power_measurements ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], measurement_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().context("list measurements")
    }
}

fn telemetry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TelemetrySample> {
    Ok(TelemetrySample {
        at_ms: row.get(0)?,
        percent: row.get::<_, Option<i64>>(1)?.map(|p| p.clamp(0, 100) as u8),
        on_ac: row.get::<_, i64>(2)? != 0,
        rate_mw: row.get(3)?,
        voltage_mv: row.get(4)?,
        capacity_mwh: row.get(5)?,
        profile_id: row.get(6)?,
        workload: row.get(7)?,
    })
}

fn measurement_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeasurementRecord> {
    Ok(MeasurementRecord {
        id: row.get(0)?,
        label: row.get(1)?,
        profile_id: row.get(2)?,
        baseline_start_ms: row.get(3)?,
        switch_ms: row.get(4)?,
        ended_ms: row.get(5)?,
        result_json: row.get(6)?,
        created_at_ms: row.get(7)?,
    })
}
