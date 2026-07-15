use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::Serialize;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::warn;

use crate::{
    anomaly::AnomalyEngine,
    anomaly_storage, capability_storage,
    config::{AnomalyConfig, ProcessConfig, RetentionConfig},
    model::{CollectionBatch, CollectorCapability, MetricBatch},
    process_storage,
    report::{DashboardSnapshot, ReportData, ReportRange},
    report_storage,
};

pub use crate::anomaly_storage::{EventDetail, EventEvidence, EventSummary};
pub use crate::process_storage::{ProcessEventEvidence, ProcessSort, StoredProcessSample};

const CURRENT_SCHEMA_VERSION: i64 = 7;
const MINUTE_MS: i64 = 60_000;
const QUARTER_HOUR_MS: i64 = 900_000;
const MINUTE_WATERMARK: &str = "rollup_60_watermark_ms";
const QUARTER_HOUR_WATERMARK: &str = "rollup_900_watermark_ms";
const LAST_MAINTENANCE_MS: &str = "last_maintenance_ms";
const LAST_MAINTENANCE_RESULT: &str = "last_maintenance_result";

const TABLE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS metric_samples (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    collected_at    TEXT NOT NULL,
    collected_at_ms INTEGER NOT NULL,
    collector       TEXT NOT NULL,
    resource        TEXT,
    metric_name     TEXT NOT NULL,
    value           REAL NOT NULL,
    unit            TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS metric_rollups (
    bucket_start_ms   INTEGER NOT NULL,
    resolution_seconds INTEGER NOT NULL,
    collector         TEXT NOT NULL,
    resource          TEXT NOT NULL DEFAULT '',
    metric_name       TEXT NOT NULL,
    unit              TEXT NOT NULL,
    sample_count      INTEGER NOT NULL,
    min_value         REAL NOT NULL,
    max_value         REAL NOT NULL,
    sum_value         REAL NOT NULL,
    average_value     REAL NOT NULL,
    last_value        REAL NOT NULL,
    PRIMARY KEY (
        resolution_seconds, bucket_start_ms, collector, resource, metric_name
    )
);

CREATE TABLE IF NOT EXISTS maintenance_state (
    key           TEXT PRIMARY KEY,
    value_integer INTEGER,
    value_text    TEXT
);

CREATE TABLE IF NOT EXISTS anomaly_events (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id              TEXT NOT NULL,
    collector            TEXT NOT NULL,
    metric_name          TEXT NOT NULL,
    resource             TEXT NOT NULL DEFAULT '',
    unit                 TEXT NOT NULL,
    status               TEXT NOT NULL,
    severity             TEXT NOT NULL,
    started_at_ms        INTEGER NOT NULL,
    detected_at_ms       INTEGER NOT NULL,
    ended_at_ms          INTEGER,
    warning_threshold    REAL NOT NULL,
    critical_threshold   REAL NOT NULL,
    recovery_threshold   REAL NOT NULL,
    peak_value           REAL NOT NULL,
    peak_at_ms           INTEGER NOT NULL,
    last_value           REAL NOT NULL,
    last_sample_ms       INTEGER NOT NULL,
    sample_count         INTEGER NOT NULL,
    data_gap_count       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS anomaly_states (
    rule_id              TEXT NOT NULL,
    resource             TEXT NOT NULL DEFAULT '',
    phase                TEXT NOT NULL,
    severity             TEXT,
    pending_severity     TEXT,
    pending_since_ms     INTEGER,
    pending_samples      INTEGER NOT NULL,
    critical_since_ms    INTEGER,
    critical_samples     INTEGER NOT NULL,
    recovery_since_ms    INTEGER,
    recovery_samples     INTEGER NOT NULL,
    event_id             INTEGER,
    last_sample_ms       INTEGER,
    last_value           REAL,
    peak_value           REAL,
    peak_at_ms           INTEGER,
    last_evidence_ms     INTEGER,
    data_gap_count       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (rule_id, resource)
);

CREATE TABLE IF NOT EXISTS anomaly_event_evidence (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id             INTEGER NOT NULL,
    collected_at_ms      INTEGER NOT NULL,
    value                REAL NOT NULL,
    kind                 TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS process_samples (
    id                         INTEGER PRIMARY KEY AUTOINCREMENT,
    collected_at_ms            INTEGER NOT NULL,
    pid                        INTEGER NOT NULL,
    process_start_time_seconds INTEGER NOT NULL,
    parent_pid                 INTEGER,
    name                       TEXT NOT NULL,
    executable_path            TEXT,
    cpu_usage_percent          REAL NOT NULL,
    memory_bytes               INTEGER NOT NULL,
    disk_read_bytes            INTEGER NOT NULL DEFAULT 0,
    disk_write_bytes           INTEGER NOT NULL DEFAULT 0,
    disk_read_bytes_per_second REAL NOT NULL DEFAULT 0,
    disk_write_bytes_per_second REAL NOT NULL DEFAULT 0,
    network_receive_bytes      INTEGER,
    network_transmit_bytes     INTEGER,
    network_receive_bytes_per_second REAL,
    network_transmit_bytes_per_second REAL,
    cpu_rank                   INTEGER,
    memory_rank                INTEGER,
    disk_read_rank             INTEGER,
    disk_write_rank            INTEGER,
    network_receive_rank       INTEGER,
    network_transmit_rank      INTEGER
);

CREATE TABLE IF NOT EXISTS process_metric_rollups (
    bucket_start_ms            INTEGER NOT NULL,
    resolution_seconds        INTEGER NOT NULL,
    pid                        INTEGER NOT NULL,
    process_start_time_seconds INTEGER NOT NULL,
    name                       TEXT NOT NULL,
    metric_name                TEXT NOT NULL,
    unit                       TEXT NOT NULL,
    sample_count               INTEGER NOT NULL,
    min_value                  REAL NOT NULL,
    max_value                  REAL NOT NULL,
    sum_value                  REAL NOT NULL,
    average_value              REAL NOT NULL,
    last_value                 REAL NOT NULL,
    peak_rank                  INTEGER,
    PRIMARY KEY (resolution_seconds, bucket_start_ms, pid, process_start_time_seconds, metric_name)
);

CREATE TABLE IF NOT EXISTS anomaly_event_process_evidence (
    id                         INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id                   INTEGER NOT NULL,
    kind                       TEXT NOT NULL,
    collected_at_ms            INTEGER NOT NULL,
    pid                        INTEGER NOT NULL,
    process_start_time_seconds INTEGER NOT NULL,
    parent_pid                 INTEGER,
    name                       TEXT NOT NULL,
    executable_path            TEXT,
    cpu_usage_percent          REAL NOT NULL,
    memory_bytes               INTEGER NOT NULL,
    disk_read_bytes            INTEGER NOT NULL DEFAULT 0,
    disk_write_bytes           INTEGER NOT NULL DEFAULT 0,
    disk_read_bytes_per_second REAL NOT NULL DEFAULT 0,
    disk_write_bytes_per_second REAL NOT NULL DEFAULT 0,
    network_receive_bytes      INTEGER,
    network_transmit_bytes     INTEGER,
    network_receive_bytes_per_second REAL,
    network_transmit_bytes_per_second REAL,
    cpu_rank                   INTEGER,
    memory_rank                INTEGER,
    disk_read_rank             INTEGER,
    disk_write_rank            INTEGER,
    network_receive_rank       INTEGER,
    network_transmit_rank      INTEGER,
    UNIQUE(event_id, collected_at_ms, pid, process_start_time_seconds)
);

CREATE TABLE IF NOT EXISTS collector_capabilities (
    collector       TEXT NOT NULL,
    resource        TEXT NOT NULL DEFAULT '',
    capability      TEXT NOT NULL,
    state           TEXT NOT NULL,
    provider        TEXT NOT NULL,
    detail          TEXT,
    checked_at_ms   INTEGER NOT NULL,
    PRIMARY KEY (collector, resource, capability)
);
"#;

const INDEX_SCHEMA: &str = r#"
CREATE INDEX IF NOT EXISTS idx_metric_samples_time_ms
    ON metric_samples(collected_at_ms);
CREATE INDEX IF NOT EXISTS idx_metric_samples_name_time_ms
    ON metric_samples(metric_name, collected_at_ms);
CREATE INDEX IF NOT EXISTS idx_metric_samples_name_resource_time_ms
    ON metric_samples(metric_name, resource, collected_at_ms);
CREATE INDEX IF NOT EXISTS idx_metric_rollups_resolution_time
    ON metric_rollups(resolution_seconds, bucket_start_ms);
CREATE INDEX IF NOT EXISTS idx_anomaly_events_status_started
    ON anomaly_events(status, started_at_ms);
CREATE INDEX IF NOT EXISTS idx_anomaly_events_severity_started
    ON anomaly_events(severity, started_at_ms);
CREATE INDEX IF NOT EXISTS idx_anomaly_event_evidence_event_time
    ON anomaly_event_evidence(event_id, collected_at_ms);
CREATE INDEX IF NOT EXISTS idx_process_samples_time
    ON process_samples(collected_at_ms);
CREATE INDEX IF NOT EXISTS idx_process_samples_identity_time
    ON process_samples(pid, process_start_time_seconds, collected_at_ms);
CREATE INDEX IF NOT EXISTS idx_process_metric_rollups_resolution_time
    ON process_metric_rollups(resolution_seconds, bucket_start_ms);
CREATE INDEX IF NOT EXISTS idx_process_metric_rollups_metric_time
    ON process_metric_rollups(metric_name, resolution_seconds, bucket_start_ms);
CREATE INDEX IF NOT EXISTS idx_anomaly_event_process_evidence_event_time
    ON anomaly_event_process_evidence(event_id, collected_at_ms);
"#;

pub struct Storage {
    connection: Connection,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatasetStatus {
    pub row_count: i64,
    pub oldest_ms: Option<i64>,
    pub newest_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageStatus {
    pub schema_version: i64,
    pub raw: DatasetStatus,
    pub minute: DatasetStatus,
    pub quarter_hour: DatasetStatus,
    pub processes: DatasetStatus,
    pub process_minute: DatasetStatus,
    pub process_quarter_hour: DatasetStatus,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub free_page_bytes: u64,
    pub minute_watermark_ms: Option<i64>,
    pub quarter_hour_watermark_ms: Option<i64>,
    pub last_maintenance_ms: Option<i64>,
    pub last_maintenance_result: Option<String>,
    pub open_warning_count: i64,
    pub open_critical_count: i64,
    pub latest_event_ms: Option<i64>,
    pub capabilities: Vec<CollectorCapability>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let mut connection = Connection::open(path)
            .with_context(|| format!("failed to open SQLite database {}", path.display()))?;
        connection.pragma_update(None, "busy_timeout", 5_000_i64)?;
        let journal_mode: String =
            connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        migrate(&mut connection)?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    pub fn open_read_only(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| {
            format!(
                "failed to open SQLite database read-only {}",
                path.display()
            )
        })?;
        connection.pragma_update(None, "busy_timeout", 5_000_i64)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != CURRENT_SCHEMA_VERSION {
            bail!(
                "dashboard requires schema version {CURRENT_SCHEMA_VERSION}, but the database is version {version}; run a current write-capable command first"
            );
        }
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    pub fn insert_batch(&mut self, batch: &MetricBatch) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO metric_samples
                 (collected_at, collected_at_ms, collector, resource, metric_name, value, unit)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for metric in batch {
                statement.execute(params![
                    metric.collected_at.to_rfc3339(),
                    metric.collected_at.timestamp_millis(),
                    metric.collector,
                    metric.resource,
                    metric.name,
                    metric.value,
                    metric.unit,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_anomaly_engine(&self, config: &AnomalyConfig) -> Result<AnomalyEngine> {
        anomaly_storage::load_engine(&self.connection, config)
    }

    pub fn insert_batch_with_anomalies(
        &mut self,
        batch: &CollectionBatch,
        engine: &mut AnomalyEngine,
        anomaly_config: &AnomalyConfig,
        process_config: &ProcessConfig,
    ) -> Result<()> {
        anomaly_storage::insert_batch(
            &mut self.connection,
            batch,
            engine,
            anomaly_config,
            process_config,
        )
    }

    pub fn run_maintenance(
        &mut self,
        retention: &RetentionConfig,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let now_ms = now.timestamp_millis();
        let transaction = self.connection.transaction()?;
        let minute_watermark = roll_up_raw_minutes(&transaction, retention, now_ms)?;
        let quarter_watermark =
            roll_up_quarter_hours(&transaction, retention, now_ms, minute_watermark)?;
        let deleted_raw = delete_raw(&transaction, retention, now_ms, minute_watermark)?;
        let deleted_minute = delete_rollups(
            &transaction,
            60,
            now_ms.saturating_sub(days_ms(retention.minute_days)),
            quarter_watermark,
            retention.delete_batch_rows,
        )?;
        let deleted_quarter = delete_rollups(
            &transaction,
            900,
            now_ms.saturating_sub(days_ms(retention.quarter_hour_days)),
            None,
            retention.delete_batch_rows,
        )?;
        set_state_integer(&transaction, LAST_MAINTENANCE_MS, now_ms)?;
        set_state_text(
            &transaction,
            LAST_MAINTENANCE_RESULT,
            &format!(
                "ok: deleted raw={deleted_raw}, minute={deleted_minute}, quarter_hour={deleted_quarter}"
            ),
        )?;
        transaction.commit()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    pub fn run_maintenance_with_anomalies(
        &mut self,
        retention: &RetentionConfig,
        anomaly: &AnomalyConfig,
        process: &ProcessConfig,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let now_ms = now.timestamp_millis();
        let transaction = self.connection.transaction()?;
        let minute_watermark = roll_up_raw_minutes(&transaction, retention, now_ms)?;
        let quarter_watermark =
            roll_up_quarter_hours(&transaction, retention, now_ms, minute_watermark)?;
        let deleted_raw = delete_raw(&transaction, retention, now_ms, minute_watermark)?;
        let deleted_minute = delete_rollups(
            &transaction,
            60,
            now_ms.saturating_sub(days_ms(retention.minute_days)),
            quarter_watermark,
            retention.delete_batch_rows,
        )?;
        let deleted_quarter = delete_rollups(
            &transaction,
            900,
            now_ms.saturating_sub(days_ms(retention.quarter_hour_days)),
            None,
            retention.delete_batch_rows,
        )?;
        let deleted_events = anomaly_storage::delete_closed_events(&transaction, anomaly, now_ms)?;
        let process_maintenance = process_storage::run_maintenance(
            &transaction,
            process,
            now_ms,
            retention.late_arrival_grace_seconds,
            retention.rollup_batch_buckets,
            retention.delete_batch_rows,
        )?;
        set_state_integer(&transaction, LAST_MAINTENANCE_MS, now_ms)?;
        set_state_text(
            &transaction,
            LAST_MAINTENANCE_RESULT,
            &format!(
                "ok: deleted raw={deleted_raw}, minute={deleted_minute}, quarter_hour={deleted_quarter}, process_raw={}, process_minute={}, process_quarter_hour={}, events={deleted_events}",
                process_maintenance.raw,
                process_maintenance.minute,
                process_maintenance.quarter_hour,
            ),
        )?;
        transaction.commit()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    pub fn list_events(&self, open_only: bool, limit: usize) -> Result<Vec<EventSummary>> {
        anomaly_storage::list_events(&self.connection, open_only, limit)
    }

    pub fn event(&self, id: i64) -> Result<Option<EventDetail>> {
        anomaly_storage::get_event(&self.connection, id)
    }

    pub fn latest_processes(
        &self,
        sort: ProcessSort,
        limit: usize,
    ) -> Result<Vec<StoredProcessSample>> {
        process_storage::latest_top(&self.connection, sort, limit)
    }

    pub fn report(&self, range: ReportRange) -> Result<ReportData> {
        report_storage::load_report(&self.connection, range)
    }

    pub fn dashboard_snapshot(&self, range: ReportRange) -> Result<DashboardSnapshot> {
        report_storage::load_dashboard_snapshot(&self.connection, range)
    }

    pub fn status(&self) -> Result<StorageStatus> {
        let schema_version = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        let raw = dataset_status(
            &self.connection,
            "SELECT COUNT(*), MIN(collected_at_ms), MAX(collected_at_ms) FROM metric_samples",
            [],
        )?;
        let minute = rollup_status(&self.connection, 60)?;
        let quarter_hour = rollup_status(&self.connection, 900)?;
        let processes = dataset_status(
            &self.connection,
            "SELECT COUNT(*), MIN(collected_at_ms), MAX(collected_at_ms) FROM process_samples",
            [],
        )?;
        let process_minute = dataset_status(
            &self.connection,
            "SELECT COUNT(*), MIN(bucket_start_ms), MAX(bucket_start_ms) FROM process_metric_rollups WHERE resolution_seconds=60",
            [],
        )?;
        let process_quarter_hour = dataset_status(
            &self.connection,
            "SELECT COUNT(*), MIN(bucket_start_ms), MAX(bucket_start_ms) FROM process_metric_rollups WHERE resolution_seconds=900",
            [],
        )?;
        let page_size: i64 = self
            .connection
            .pragma_query_value(None, "page_size", |row| row.get(0))?;
        let free_pages: i64 =
            self.connection
                .pragma_query_value(None, "freelist_count", |row| row.get(0))?;

        Ok(StorageStatus {
            schema_version,
            raw,
            minute,
            quarter_hour,
            processes,
            process_minute,
            process_quarter_hour,
            database_bytes: file_size(&self.path),
            wal_bytes: file_size(&PathBuf::from(format!("{}-wal", self.path.display()))),
            free_page_bytes: u64::try_from(page_size.saturating_mul(free_pages)).unwrap_or(0),
            minute_watermark_ms: get_state_integer(&self.connection, MINUTE_WATERMARK)?,
            quarter_hour_watermark_ms: get_state_integer(&self.connection, QUARTER_HOUR_WATERMARK)?,
            last_maintenance_ms: get_state_integer(&self.connection, LAST_MAINTENANCE_MS)?,
            last_maintenance_result: get_state_text(&self.connection, LAST_MAINTENANCE_RESULT)?,
            open_warning_count: self.connection.query_row(
                "SELECT COUNT(*) FROM anomaly_events WHERE status = 'open' AND severity = 'warning'",
                [],
                |row| row.get(0),
            )?,
            open_critical_count: self.connection.query_row(
                "SELECT COUNT(*) FROM anomaly_events WHERE status = 'open' AND severity = 'critical'",
                [],
                |row| row.get(0),
            )?,
            latest_event_ms: self.connection.query_row(
                "SELECT MAX(started_at_ms) FROM anomaly_events",
                [],
                |row| row.get(0),
            )?,
            capabilities: capability_storage::list(&self.connection)?,
        })
    }
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > CURRENT_SCHEMA_VERSION {
        bail!(
            "database schema version {version} is newer than supported version \
             {CURRENT_SCHEMA_VERSION}"
        );
    }
    if version == CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    let transaction = connection.transaction()?;
    if !table_exists(&transaction, "metric_samples")? {
        transaction.execute_batch(TABLE_SCHEMA)?;
    } else {
        if !table_has_column(&transaction, "metric_samples", "resource")? {
            transaction.execute("ALTER TABLE metric_samples ADD COLUMN resource TEXT", [])?;
        }
        if !table_has_column(&transaction, "metric_samples", "collected_at_ms")? {
            transaction.execute(
                "ALTER TABLE metric_samples ADD COLUMN collected_at_ms INTEGER",
                [],
            )?;
            transaction.execute(
                "UPDATE metric_samples
                 SET collected_at_ms = CAST(strftime('%s', collected_at) AS INTEGER) * 1000
                 WHERE collected_at_ms IS NULL",
                [],
            )?;
            let missing_timestamps: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM metric_samples WHERE collected_at_ms IS NULL",
                [],
                |row| row.get(0),
            )?;
            if missing_timestamps != 0 {
                bail!("could not migrate {missing_timestamps} sample timestamps to milliseconds");
            }
        }
        transaction.execute_batch(TABLE_SCHEMA)?;
    }
    for (table, columns) in [
        ("process_samples", PROCESS_COLUMNS),
        ("anomaly_event_process_evidence", PROCESS_COLUMNS),
    ] {
        for (name, declaration) in columns {
            if !table_has_column(&transaction, table, name)? {
                transaction.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {name} {declaration}"),
                    [],
                )?;
            }
        }
    }
    if version < 7 {
        migrate_remove_gpu(&transaction)?;
    }
    transaction.execute_batch(TABLE_SCHEMA)?;
    transaction.execute_batch(INDEX_SCHEMA)?;
    transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

const PROCESS_COLUMNS: &[(&str, &str)] = &[
    ("disk_read_bytes", "INTEGER NOT NULL DEFAULT 0"),
    ("disk_write_bytes", "INTEGER NOT NULL DEFAULT 0"),
    ("disk_read_bytes_per_second", "REAL NOT NULL DEFAULT 0"),
    ("disk_write_bytes_per_second", "REAL NOT NULL DEFAULT 0"),
    ("network_receive_bytes", "INTEGER"),
    ("network_transmit_bytes", "INTEGER"),
    ("network_receive_bytes_per_second", "REAL"),
    ("network_transmit_bytes_per_second", "REAL"),
    ("disk_read_rank", "INTEGER"),
    ("disk_write_rank", "INTEGER"),
    ("network_receive_rank", "INTEGER"),
    ("network_transmit_rank", "INTEGER"),
];

fn migrate_remove_gpu(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "DELETE FROM anomaly_event_process_evidence
           WHERE event_id IN (SELECT id FROM anomaly_events
                              WHERE collector = 'gpu' OR metric_name LIKE 'gpu.%');
         DELETE FROM anomaly_event_evidence
           WHERE event_id IN (SELECT id FROM anomaly_events
                              WHERE collector = 'gpu' OR metric_name LIKE 'gpu.%');
         DELETE FROM anomaly_states
           WHERE rule_id IN (SELECT DISTINCT rule_id FROM anomaly_events
                             WHERE collector = 'gpu' OR metric_name LIKE 'gpu.%');
         DELETE FROM anomaly_events WHERE collector = 'gpu' OR metric_name LIKE 'gpu.%';
         DELETE FROM metric_samples WHERE collector = 'gpu' OR metric_name LIKE 'gpu.%';
         DELETE FROM metric_rollups WHERE collector = 'gpu' OR metric_name LIKE 'gpu.%';
         DELETE FROM process_metric_rollups WHERE metric_name LIKE 'process.gpu.%';
         DELETE FROM collector_capabilities
           WHERE collector = 'gpu' OR capability LIKE 'gpu.%' OR capability LIKE 'process.gpu%';",
    )?;
    rebuild_process_table_without_gpu(transaction, "process_samples")?;
    rebuild_process_table_without_gpu(transaction, "anomaly_event_process_evidence")?;
    Ok(())
}

fn rebuild_process_table_without_gpu(transaction: &Transaction<'_>, table: &str) -> Result<()> {
    if !table_has_column(transaction, table, "gpu_time_ns")?
        && !table_has_column(transaction, table, "gpu_usage_percent")?
        && !table_has_column(transaction, table, "gpu_rank")?
    {
        return Ok(());
    }
    let legacy = format!("{table}_with_gpu");
    transaction.execute(&format!("ALTER TABLE {table} RENAME TO {legacy}"), [])?;
    transaction.execute_batch(TABLE_SCHEMA)?;
    let (columns, selected_columns) = if table == "process_samples" {
        (
            "id, collected_at_ms, pid, process_start_time_seconds, parent_pid, name, executable_path,
             cpu_usage_percent, memory_bytes, disk_read_bytes, disk_write_bytes,
             disk_read_bytes_per_second, disk_write_bytes_per_second, network_receive_bytes,
             network_transmit_bytes, network_receive_bytes_per_second,
             network_transmit_bytes_per_second, cpu_rank, memory_rank, disk_read_rank,
             disk_write_rank, network_receive_rank, network_transmit_rank",
            "id, collected_at_ms, pid, process_start_time_seconds, parent_pid, name, executable_path,
             cpu_usage_percent, memory_bytes, disk_read_bytes, disk_write_bytes,
             disk_read_bytes_per_second, disk_write_bytes_per_second, network_receive_bytes,
             network_transmit_bytes, network_receive_bytes_per_second,
             network_transmit_bytes_per_second, cpu_rank, memory_rank, disk_read_rank,
             disk_write_rank, network_receive_rank, network_transmit_rank",
        )
    } else {
        (
            "id, event_id, kind, collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
             executable_path, cpu_usage_percent, memory_bytes, disk_read_bytes, disk_write_bytes,
             disk_read_bytes_per_second, disk_write_bytes_per_second, network_receive_bytes,
             network_transmit_bytes, network_receive_bytes_per_second,
             network_transmit_bytes_per_second, cpu_rank, memory_rank, disk_read_rank,
             disk_write_rank, network_receive_rank, network_transmit_rank",
            "id, event_id, kind, collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
             executable_path, cpu_usage_percent, memory_bytes, disk_read_bytes, disk_write_bytes,
             disk_read_bytes_per_second, disk_write_bytes_per_second, network_receive_bytes,
             network_transmit_bytes, network_receive_bytes_per_second,
             network_transmit_bytes_per_second, cpu_rank, memory_rank, disk_read_rank,
             disk_write_rank, network_receive_rank, network_transmit_rank",
        )
    };
    transaction.execute(
        &format!("INSERT INTO {table} ({columns}) SELECT {selected_columns} FROM {legacy}"),
        [],
    )?;
    transaction.execute(&format!("DROP TABLE {legacy}"), [])?;
    Ok(())
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let escaped_table = table.replace('"', "\"\"");
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{escaped_table}\")"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SeriesKey {
    collector: String,
    resource: String,
    metric_name: String,
    unit: String,
}

#[derive(Debug, Clone)]
struct Aggregate {
    count: i64,
    min: f64,
    max: f64,
    sum: f64,
    last: f64,
}

impl Aggregate {
    fn from_value(value: f64) -> Self {
        Self {
            count: 1,
            min: value,
            max: value,
            sum: value,
            last: value,
        }
    }

    fn add_value(&mut self, value: f64) {
        self.count += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        self.sum += value;
        self.last = value;
    }

    fn add_rollup(&mut self, count: i64, min: f64, max: f64, sum: f64, last: f64) {
        self.count += count;
        self.min = self.min.min(min);
        self.max = self.max.max(max);
        self.sum += sum;
        self.last = last;
    }

    fn average(&self) -> f64 {
        self.sum / self.count as f64
    }
}

fn roll_up_raw_minutes(
    transaction: &Transaction<'_>,
    retention: &RetentionConfig,
    now_ms: i64,
) -> Result<Option<i64>> {
    let Some(mut cursor) = initial_watermark(
        transaction,
        MINUTE_WATERMARK,
        "SELECT MIN(collected_at_ms) FROM metric_samples",
        MINUTE_MS,
    )?
    else {
        return Ok(None);
    };
    let cutoff = floor_bucket(
        now_ms.saturating_sub(seconds_ms(retention.late_arrival_grace_seconds)),
        MINUTE_MS,
    );
    for _ in 0..retention.rollup_batch_buckets {
        if cursor >= cutoff {
            break;
        }
        let aggregates = aggregate_raw(transaction, cursor, cursor + MINUTE_MS)?;
        upsert_rollups(transaction, cursor, 60, &aggregates)?;
        cursor += MINUTE_MS;
        set_state_integer(transaction, MINUTE_WATERMARK, cursor)?;
    }
    Ok(Some(cursor))
}

fn roll_up_quarter_hours(
    transaction: &Transaction<'_>,
    retention: &RetentionConfig,
    now_ms: i64,
    minute_watermark: Option<i64>,
) -> Result<Option<i64>> {
    let Some(minute_watermark) = minute_watermark else {
        return get_state_integer(transaction, QUARTER_HOUR_WATERMARK);
    };
    let Some(mut cursor) = initial_watermark(
        transaction,
        QUARTER_HOUR_WATERMARK,
        "SELECT MIN(bucket_start_ms) FROM metric_rollups WHERE resolution_seconds = 60",
        QUARTER_HOUR_MS,
    )?
    else {
        return Ok(None);
    };
    let time_cutoff = floor_bucket(
        now_ms.saturating_sub(seconds_ms(retention.late_arrival_grace_seconds)),
        QUARTER_HOUR_MS,
    );
    let cutoff = time_cutoff.min(floor_bucket(minute_watermark, QUARTER_HOUR_MS));
    for _ in 0..retention.rollup_batch_buckets {
        if cursor >= cutoff {
            break;
        }
        let aggregates = aggregate_rollups(transaction, cursor, cursor + QUARTER_HOUR_MS)?;
        upsert_rollups(transaction, cursor, 900, &aggregates)?;
        cursor += QUARTER_HOUR_MS;
        set_state_integer(transaction, QUARTER_HOUR_WATERMARK, cursor)?;
    }
    Ok(Some(cursor))
}

fn initial_watermark(
    connection: &Connection,
    key: &str,
    minimum_sql: &str,
    resolution_ms: i64,
) -> Result<Option<i64>> {
    if let Some(value) = get_state_integer(connection, key)? {
        return Ok(Some(value));
    }
    let minimum: Option<i64> = connection.query_row(minimum_sql, [], |row| row.get(0))?;
    Ok(minimum.map(|value| floor_bucket(value, resolution_ms)))
}

fn aggregate_raw(
    connection: &Connection,
    start_ms: i64,
    end_ms: i64,
) -> Result<HashMap<SeriesKey, Aggregate>> {
    let mut statement = connection.prepare(
        "SELECT collector, COALESCE(resource, ''), metric_name, unit, value
         FROM metric_samples
         WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
         ORDER BY collected_at_ms, id",
    )?;
    let mut rows = statement.query(params![start_ms, end_ms])?;
    let mut aggregates = HashMap::new();
    while let Some(row) = rows.next()? {
        let key = SeriesKey {
            collector: row.get(0)?,
            resource: row.get(1)?,
            metric_name: row.get(2)?,
            unit: row.get(3)?,
        };
        let value = row.get(4)?;
        aggregates
            .entry(key)
            .and_modify(|aggregate: &mut Aggregate| aggregate.add_value(value))
            .or_insert_with(|| Aggregate::from_value(value));
    }
    Ok(aggregates)
}

fn aggregate_rollups(
    connection: &Connection,
    start_ms: i64,
    end_ms: i64,
) -> Result<HashMap<SeriesKey, Aggregate>> {
    let mut statement = connection.prepare(
        "SELECT collector, resource, metric_name, unit,
                sample_count, min_value, max_value, sum_value, last_value
         FROM metric_rollups
         WHERE resolution_seconds = 60
           AND bucket_start_ms >= ?1 AND bucket_start_ms < ?2
         ORDER BY bucket_start_ms",
    )?;
    let mut rows = statement.query(params![start_ms, end_ms])?;
    let mut aggregates = HashMap::new();
    while let Some(row) = rows.next()? {
        let key = SeriesKey {
            collector: row.get(0)?,
            resource: row.get(1)?,
            metric_name: row.get(2)?,
            unit: row.get(3)?,
        };
        let count = row.get(4)?;
        let min = row.get(5)?;
        let max = row.get(6)?;
        let sum = row.get(7)?;
        let last = row.get(8)?;
        aggregates
            .entry(key)
            .and_modify(|aggregate: &mut Aggregate| {
                aggregate.add_rollup(count, min, max, sum, last);
            })
            .or_insert(Aggregate {
                count,
                min,
                max,
                sum,
                last,
            });
    }
    Ok(aggregates)
}

fn upsert_rollups(
    connection: &Connection,
    bucket_start_ms: i64,
    resolution_seconds: i64,
    aggregates: &HashMap<SeriesKey, Aggregate>,
) -> Result<()> {
    let mut statement = connection.prepare_cached(
        "INSERT INTO metric_rollups
         (bucket_start_ms, resolution_seconds, collector, resource, metric_name, unit,
          sample_count, min_value, max_value, sum_value, average_value, last_value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(resolution_seconds, bucket_start_ms, collector, resource, metric_name)
         DO UPDATE SET
           unit = excluded.unit,
           sample_count = excluded.sample_count,
           min_value = excluded.min_value,
           max_value = excluded.max_value,
           sum_value = excluded.sum_value,
           average_value = excluded.average_value,
           last_value = excluded.last_value",
    )?;
    for (key, aggregate) in aggregates {
        statement.execute(params![
            bucket_start_ms,
            resolution_seconds,
            key.collector,
            key.resource,
            key.metric_name,
            key.unit,
            aggregate.count,
            aggregate.min,
            aggregate.max,
            aggregate.sum,
            aggregate.average(),
            aggregate.last,
        ])?;
    }
    Ok(())
}

fn delete_raw(
    connection: &Connection,
    retention: &RetentionConfig,
    now_ms: i64,
    minute_watermark: Option<i64>,
) -> Result<usize> {
    let Some(watermark) = minute_watermark else {
        return Ok(0);
    };
    let cutoff = now_ms
        .saturating_sub(hours_ms(retention.raw_hours))
        .min(watermark);
    Ok(connection.execute(
        "DELETE FROM metric_samples WHERE id IN (
             SELECT id FROM metric_samples WHERE collected_at_ms < ?1
             ORDER BY collected_at_ms LIMIT ?2
         )",
        params![
            cutoff,
            i64::try_from(retention.delete_batch_rows).unwrap_or(i64::MAX)
        ],
    )?)
}

fn delete_rollups(
    connection: &Connection,
    resolution_seconds: i64,
    retention_cutoff: i64,
    downstream_watermark: Option<i64>,
    batch_rows: usize,
) -> Result<usize> {
    let cutoff = downstream_watermark.map_or(retention_cutoff, |watermark| {
        retention_cutoff.min(watermark)
    });
    if resolution_seconds == 60 && downstream_watermark.is_none() {
        return Ok(0);
    }
    Ok(connection.execute(
        "DELETE FROM metric_rollups WHERE rowid IN (
             SELECT rowid FROM metric_rollups
             WHERE resolution_seconds = ?1 AND bucket_start_ms < ?2
             ORDER BY bucket_start_ms LIMIT ?3
         )",
        params![
            resolution_seconds,
            cutoff,
            i64::try_from(batch_rows).unwrap_or(i64::MAX)
        ],
    )?)
}

fn dataset_status<P>(connection: &Connection, sql: &str, params: P) -> Result<DatasetStatus>
where
    P: rusqlite::Params,
{
    Ok(connection.query_row(sql, params, |row| {
        Ok(DatasetStatus {
            row_count: row.get(0)?,
            oldest_ms: row.get(1)?,
            newest_ms: row.get(2)?,
        })
    })?)
}

fn rollup_status(connection: &Connection, resolution: i64) -> Result<DatasetStatus> {
    dataset_status(
        connection,
        "SELECT COUNT(*), MIN(bucket_start_ms), MAX(bucket_start_ms)
         FROM metric_rollups WHERE resolution_seconds = ?1",
        [resolution],
    )
}

fn get_state_integer(connection: &Connection, key: &str) -> Result<Option<i64>> {
    Ok(connection
        .query_row(
            "SELECT value_integer FROM maintenance_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

fn get_state_text(connection: &Connection, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT value_text FROM maintenance_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

fn set_state_integer(connection: &Connection, key: &str, value: i64) -> Result<()> {
    connection.execute(
        "INSERT INTO maintenance_state(key, value_integer) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_integer = excluded.value_integer",
        params![key, value],
    )?;
    Ok(())
}

fn set_state_text(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO maintenance_state(key, value_text) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_text = excluded.value_text",
        params![key, value],
    )?;
    Ok(())
}

fn floor_bucket(timestamp_ms: i64, resolution_ms: i64) -> i64 {
    timestamp_ms.div_euclid(resolution_ms) * resolution_ms
}

fn seconds_ms(seconds: u64) -> i64 {
    i64::try_from(seconds)
        .unwrap_or(i64::MAX / 1_000)
        .saturating_mul(1_000)
}

fn hours_ms(hours: u64) -> i64 {
    seconds_ms(hours.saturating_mul(3_600))
}

fn days_ms(days: u64) -> i64 {
    hours_ms(days.saturating_mul(24))
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

pub fn spawn_writer(
    path: &Path,
    retention: RetentionConfig,
    anomaly: AnomalyConfig,
    process: ProcessConfig,
    mut receiver: mpsc::Receiver<CollectionBatch>,
) -> JoinHandle<Result<()>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut storage = Storage::open(&path)?;
        let mut anomaly_engine = storage.load_anomaly_engine(&anomaly)?;
        let maintenance_interval = Duration::from_secs(retention.maintenance_interval_seconds);
        let mut last_maintenance = Instant::now()
            .checked_sub(maintenance_interval)
            .unwrap_or_else(Instant::now);
        while let Some(batch) = receiver.blocking_recv() {
            storage.insert_batch_with_anomalies(&batch, &mut anomaly_engine, &anomaly, &process)?;
            if last_maintenance.elapsed() >= maintenance_interval {
                if let Err(error) = storage.run_maintenance_with_anomalies(
                    &retention,
                    &anomaly,
                    &process,
                    Utc::now(),
                ) {
                    warn!(%error, "storage maintenance failed; collection will continue");
                }
                last_maintenance = Instant::now();
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use super::*;
    use crate::{
        config::{AnomalyConfig, AnomalyRuleConfig},
        model::{CapabilityState, CollectorCapability, Metric, ProcessSample},
    };

    fn at(milliseconds: i64) -> DateTime<Utc> {
        Utc.timestamp_millis_opt(milliseconds)
            .single()
            .expect("time")
    }

    fn test_retention() -> RetentionConfig {
        RetentionConfig {
            late_arrival_grace_seconds: 0,
            rollup_batch_buckets: 1_000,
            ..RetentionConfig::default()
        }
    }

    fn test_anomaly_config() -> AnomalyConfig {
        AnomalyConfig {
            prelude_minutes: 5,
            evidence_interval_seconds: 10,
            rules: vec![AnomalyRuleConfig {
                id: "cpu-test".to_owned(),
                metric_name: "cpu.total.usage".to_owned(),
                warning_threshold: 90.0,
                critical_threshold: 97.0,
                recovery_threshold: 75.0,
                trigger_seconds: 10,
                critical_trigger_seconds: 5,
                recovery_seconds: 10,
                min_samples: 3,
                critical_min_samples: 2,
                recovery_min_samples: 3,
                max_sample_gap_seconds: 6,
                ..AnomalyRuleConfig::default()
            }],
            ..AnomalyConfig::default()
        }
    }

    fn cpu_metric(seconds: i64, value: f64) -> Metric {
        Metric::new(
            at(seconds * 1_000),
            "system",
            "cpu.total.usage",
            value,
            "percent",
        )
    }

    fn process_sample(
        seconds: i64,
        pid: u32,
        name: &str,
        cpu: f64,
        memory: u64,
        cpu_rank: Option<u32>,
        memory_rank: Option<u32>,
    ) -> ProcessSample {
        ProcessSample {
            collected_at: at(seconds * 1_000),
            pid,
            process_start_time_seconds: u64::from(pid) * 10,
            parent_pid: Some(1),
            name: name.to_owned(),
            executable_path: None,
            cpu_usage_percent: cpu,
            memory_bytes: memory,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
            disk_read_bytes_per_second: 0.0,
            disk_write_bytes_per_second: 0.0,
            network_receive_bytes: None,
            network_transmit_bytes: None,
            network_receive_bytes_per_second: None,
            network_transmit_bytes_per_second: None,
            cpu_rank,
            memory_rank,
            disk_read_rank: None,
            disk_write_rank: None,
            network_receive_rank: None,
            network_transmit_rank: None,
        }
    }

    #[test]
    fn writes_a_batch_and_reports_status() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let batch = vec![Metric::new(
            at(1_000),
            "test",
            "cpu.total.usage",
            42.0,
            "percent",
        )];

        storage.insert_batch(&batch).expect("insert batch");
        let status = storage.status().expect("read status");

        assert_eq!(status.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(status.raw.row_count, 1);
        assert_eq!(status.raw.oldest_ms, Some(1_000));
    }

    #[test]
    fn opens_a_current_database_concurrently() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        drop(Storage::open(&path).expect("initialize storage"));
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    Storage::open(&path)
                        .and_then(|storage| storage.status())
                        .expect("concurrent open")
                })
            })
            .collect();
        barrier.wait();

        for handle in handles {
            assert_eq!(
                handle.join().expect("thread").schema_version,
                CURRENT_SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn stores_resource_identity() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let batch = vec![Metric::for_resource(
            Utc::now(),
            "network",
            "en0",
            "network.receive.delta",
            128.0,
            "bytes",
        )];

        storage.insert_batch(&batch).expect("insert batch");
        let resource: Option<String> = storage
            .connection
            .query_row("SELECT resource FROM metric_samples", [], |row| row.get(0))
            .expect("read resource");

        assert_eq!(resource.as_deref(), Some("en0"));
    }

    #[test]
    fn upgrades_a_v1_database_without_losing_rows() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("legacy.sqlite3");
        let legacy = Connection::open(&path).expect("open legacy database");
        legacy
            .execute_batch(
                r#"
                CREATE TABLE metric_samples (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    collected_at TEXT NOT NULL,
                    collector TEXT NOT NULL,
                    resource TEXT,
                    metric_name TEXT NOT NULL,
                    value REAL NOT NULL,
                    unit TEXT NOT NULL
                );
                INSERT INTO metric_samples
                    (collected_at, collector, metric_name, value, unit)
                VALUES
                    ('2026-07-15T00:00:00Z', 'system', 'memory.used', 1.0, 'bytes');
                PRAGMA user_version = 1;
                "#,
            )
            .expect("create legacy schema");
        drop(legacy);

        let storage = Storage::open(&path).expect("migrate storage");
        let timestamp_ms: i64 = storage
            .connection
            .query_row("SELECT collected_at_ms FROM metric_samples", [], |row| {
                row.get(0)
            })
            .expect("backfilled timestamp");

        assert_eq!(storage.status().expect("status").raw.row_count, 1);
        assert_eq!(timestamp_ms, 1_784_073_600_000);
    }

    #[test]
    fn upgrades_a_v2_database_with_anomaly_tables() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("v2.sqlite3");
        let legacy = Connection::open(&path).expect("open legacy database");
        legacy
            .execute_batch(
                r#"
                CREATE TABLE metric_samples (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    collected_at TEXT NOT NULL,
                    collected_at_ms INTEGER NOT NULL,
                    collector TEXT NOT NULL,
                    resource TEXT,
                    metric_name TEXT NOT NULL,
                    value REAL NOT NULL,
                    unit TEXT NOT NULL
                );
                INSERT INTO metric_samples
                    (collected_at, collected_at_ms, collector, metric_name, value, unit)
                VALUES ('2026-07-15T00:00:00Z', 1784073600000, 'system',
                        'cpu.total.usage', 42.0, 'percent');
                PRAGMA user_version = 2;
                "#,
            )
            .expect("create v2 schema");
        drop(legacy);

        let storage = Storage::open(&path).expect("migrate storage");

        assert_eq!(
            storage.status().expect("status").schema_version,
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(storage.status().expect("status").raw.row_count, 1);
        assert!(table_exists(&storage.connection, "anomaly_events").expect("event table"));
        assert!(table_exists(&storage.connection, "anomaly_states").expect("state table"));
    }

    #[test]
    fn upgrades_a_v3_database_with_process_tables() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("v3.sqlite3");
        let storage = Storage::open(&path).expect("initialize storage");
        storage
            .connection
            .execute_batch(
                "DROP TABLE anomaly_event_process_evidence;
                 DROP TABLE process_samples;
                 PRAGMA user_version = 3;",
            )
            .expect("rewind to v3");
        drop(storage);

        let storage = Storage::open(&path).expect("migrate v3 storage");

        assert_eq!(
            storage.status().expect("status").schema_version,
            CURRENT_SCHEMA_VERSION
        );
        assert!(table_exists(&storage.connection, "process_samples").expect("process table"));
        assert!(
            table_exists(&storage.connection, "anomaly_event_process_evidence")
                .expect("process evidence table")
        );
    }

    #[test]
    fn upgrades_a_v4_database_with_capability_table() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("v4.sqlite3");
        let storage = Storage::open(&path).expect("initialize storage");
        storage
            .connection
            .execute_batch("DROP TABLE collector_capabilities; PRAGMA user_version = 4;")
            .expect("rewind to v4");
        drop(storage);

        let storage = Storage::open(&path).expect("migrate v4 storage");

        assert_eq!(
            storage.status().expect("status").schema_version,
            CURRENT_SCHEMA_VERSION
        );
        assert!(
            table_exists(&storage.connection, "collector_capabilities").expect("capability table")
        );
    }

    #[test]
    fn upgrades_a_v5_process_database_without_losing_samples() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("v5.sqlite3");
        let legacy = Connection::open(&path).expect("legacy");
        legacy.execute_batch(
            "CREATE TABLE metric_samples (
               id INTEGER PRIMARY KEY AUTOINCREMENT, collected_at TEXT NOT NULL,
               collected_at_ms INTEGER NOT NULL, collector TEXT NOT NULL, resource TEXT,
               metric_name TEXT NOT NULL, value REAL NOT NULL, unit TEXT NOT NULL);
             CREATE TABLE process_samples (
               id INTEGER PRIMARY KEY AUTOINCREMENT, collected_at_ms INTEGER NOT NULL,
               pid INTEGER NOT NULL, process_start_time_seconds INTEGER NOT NULL,
               parent_pid INTEGER, name TEXT NOT NULL, executable_path TEXT,
               cpu_usage_percent REAL NOT NULL, memory_bytes INTEGER NOT NULL,
               cpu_rank INTEGER, memory_rank INTEGER);
             CREATE TABLE anomaly_event_process_evidence (
               id INTEGER PRIMARY KEY AUTOINCREMENT, event_id INTEGER NOT NULL, kind TEXT NOT NULL,
               collected_at_ms INTEGER NOT NULL, pid INTEGER NOT NULL,
               process_start_time_seconds INTEGER NOT NULL, parent_pid INTEGER, name TEXT NOT NULL,
               executable_path TEXT, cpu_usage_percent REAL NOT NULL, memory_bytes INTEGER NOT NULL,
               cpu_rank INTEGER, memory_rank INTEGER,
               UNIQUE(event_id, collected_at_ms, pid, process_start_time_seconds));
             INSERT INTO process_samples
               (collected_at_ms,pid,process_start_time_seconds,name,cpu_usage_percent,memory_bytes,cpu_rank)
               VALUES (1000,42,10,'legacy',12.5,4096,1);
             PRAGMA user_version=5;"
        ).expect("v5 schema");
        drop(legacy);

        let storage = Storage::open(&path).expect("migrate v5");
        assert_eq!(
            storage.status().expect("status").schema_version,
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(storage.status().expect("status").processes.row_count, 1);
        assert!(
            table_has_column(
                &storage.connection,
                "process_samples",
                "network_receive_bytes_per_second"
            )
            .expect("column")
        );
        assert!(table_exists(&storage.connection, "process_metric_rollups").expect("rollup table"));
    }

    #[test]
    fn upgrades_v6_by_removing_gpu_data_and_columns() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("v6.sqlite3");
        let legacy = Connection::open(&path).expect("legacy");
        legacy.execute_batch(TABLE_SCHEMA).expect("base schema");
        legacy
            .execute_batch(
                "ALTER TABLE process_samples ADD COLUMN gpu_time_ns INTEGER;
                 ALTER TABLE process_samples ADD COLUMN gpu_usage_percent REAL;
                 ALTER TABLE process_samples ADD COLUMN gpu_rank INTEGER;
                 ALTER TABLE anomaly_event_process_evidence ADD COLUMN gpu_time_ns INTEGER;
                 ALTER TABLE anomaly_event_process_evidence ADD COLUMN gpu_usage_percent REAL;
                 ALTER TABLE anomaly_event_process_evidence ADD COLUMN gpu_rank INTEGER;
                 INSERT INTO process_samples
                   (collected_at_ms, pid, process_start_time_seconds, name,
                    cpu_usage_percent, memory_bytes, gpu_time_ns, gpu_usage_percent, gpu_rank)
                   VALUES (1000, 42, 10, 'legacy', 12.5, 4096, 100, 5.0, 1);
                 INSERT INTO metric_samples
                   (collected_at, collected_at_ms, collector, metric_name, value, unit)
                   VALUES ('1970-01-01T00:00:01Z', 1000, 'gpu', 'gpu.device.usage', 5.0, 'percent');
                 INSERT INTO process_metric_rollups
                   (bucket_start_ms, resolution_seconds, pid, process_start_time_seconds, name,
                    metric_name, unit, sample_count, min_value, max_value, sum_value,
                    average_value, last_value, peak_rank)
                   VALUES (0, 60, 42, 10, 'legacy', 'process.gpu.usage', 'percent',
                           1, 5.0, 5.0, 5.0, 5.0, 5.0, 1);
                 INSERT INTO collector_capabilities
                   (collector, resource, capability, state, provider, checked_at_ms)
                   VALUES ('gpu', 'Apple GPU 0', 'gpu.device.usage', 'available', 'apple_ioreg', 1000);
                 PRAGMA user_version=6;",
            )
            .expect("v6 schema and data");
        drop(legacy);

        let storage = Storage::open(&path).expect("migrate v6");
        assert_eq!(
            storage.status().expect("status").schema_version,
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(storage.status().expect("status").processes.row_count, 1);
        assert!(
            !table_has_column(&storage.connection, "process_samples", "gpu_usage_percent")
                .expect("column")
        );
        for table in [
            "metric_samples",
            "process_metric_rollups",
            "collector_capabilities",
        ] {
            let count: i64 = storage
                .connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("count");
            assert_eq!(count, 0, "{table}");
        }
    }

    #[test]
    fn capability_updates_commit_with_the_collection_batch() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("capabilities.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let anomaly = AnomalyConfig::default();
        let process = ProcessConfig::default();
        let mut engine = storage.load_anomaly_engine(&anomaly).expect("engine");
        let capability = CollectorCapability {
            collector: "process".to_owned(),
            resource: String::new(),
            capability: "process.network_io".to_owned(),
            state: CapabilityState::Available,
            provider: "macos-nettop".to_owned(),
            detail: None,
            checked_at_ms: 1_000,
        };

        storage
            .insert_batch_with_anomalies(
                &CollectionBatch {
                    capabilities: vec![capability.clone()],
                    ..CollectionBatch::default()
                },
                &mut engine,
                &anomaly,
                &process,
            )
            .expect("capability batch");

        assert_eq!(
            storage.status().expect("status").capabilities,
            vec![capability]
        );
    }

    #[test]
    fn stores_and_queries_latest_process_rankings() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let anomaly = AnomalyConfig::default();
        let process = ProcessConfig::default();
        let mut engine = storage.load_anomaly_engine(&anomaly).expect("engine");
        let batch = CollectionBatch {
            metrics: Vec::new(),
            processes: vec![
                process_sample(10, 10, "cpu-heavy", 150.0, 100, Some(1), Some(2)),
                process_sample(10, 20, "memory-heavy", 10.0, 2_000, Some(2), Some(1)),
            ],
            capabilities: Vec::new(),
        };

        storage
            .insert_batch_with_anomalies(&batch, &mut engine, &anomaly, &process)
            .expect("process snapshot");

        assert_eq!(storage.status().expect("status").processes.row_count, 2);
        assert_eq!(
            storage
                .latest_processes(ProcessSort::Cpu, 1)
                .expect("cpu top")[0]
                .name,
            "cpu-heavy"
        );
        assert_eq!(
            storage
                .latest_processes(ProcessSort::Memory, 1)
                .expect("memory top")[0]
                .name,
            "memory-heavy"
        );
    }

    #[test]
    fn report_uses_bounded_raw_series_and_escapes_resource_names() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let anomaly = AnomalyConfig::default();
        let process = ProcessConfig::default();
        let mut engine = storage.load_anomaly_engine(&anomaly).expect("engine");
        storage
            .insert_batch_with_anomalies(
                &CollectionBatch {
                    metrics: vec![
                        cpu_metric(10, 42.0),
                        Metric::for_resource(
                            at(10_000),
                            "disk",
                            "<script>alert(1)</script>",
                            "disk.space.usage",
                            75.0,
                            "percent",
                        ),
                    ],
                    processes: vec![process_sample(
                        10,
                        99,
                        "<script>process</script>",
                        25.0,
                        1_024,
                        Some(1),
                        Some(1),
                    )],
                    capabilities: Vec::new(),
                },
                &mut engine,
                &anomaly,
                &process,
            )
            .expect("report data");

        let report = storage
            .report(ReportRange::new(0, 60_000).expect("range"))
            .expect("report");
        let html = crate::report::render_html(&report);

        assert_eq!(report.resolution, crate::report::ReportResolution::Raw);
        assert_eq!(report.series.len(), 2);
        assert_eq!(report.processes.len(), 1);
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(
            report
                .series
                .iter()
                .all(|series| series.points.len() <= 1_200)
        );
    }

    #[test]
    fn report_prefers_minute_rollups_for_a_multi_hour_range() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        storage
            .insert_batch(&vec![cpu_metric(10, 20.0), cpu_metric(70, 40.0)])
            .expect("raw metrics");
        storage
            .run_maintenance(&test_retention(), at(3 * 60 * 60 * 1_000))
            .expect("rollups");

        let report = storage
            .report(ReportRange::new(0, 3 * 60 * 60 * 1_000).expect("range"))
            .expect("report");

        assert_eq!(report.resolution, crate::report::ReportResolution::Minute);
        assert!(!report.series.is_empty());
    }

    #[test]
    fn cpu_event_preserves_bounded_process_evidence_after_raw_process_retention() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let anomaly = test_anomaly_config();
        let process = ProcessConfig {
            interval_seconds: 5,
            event_top_n: 1,
            event_evidence_max_rows: 3,
            raw_retention_hours: 1,
            ..ProcessConfig::default()
        };
        let mut engine = storage.load_anomaly_engine(&anomaly).expect("engine");
        for (seconds, value, name) in [(0, 91.0, "p0"), (5, 92.0, "p5"), (10, 93.0, "p10")] {
            let batch = CollectionBatch {
                metrics: vec![cpu_metric(seconds, value)],
                processes: vec![process_sample(
                    seconds,
                    u32::try_from(seconds + 10).expect("pid"),
                    name,
                    value,
                    100,
                    Some(1),
                    Some(1),
                )],
                capabilities: Vec::new(),
            };
            storage
                .insert_batch_with_anomalies(&batch, &mut engine, &anomaly, &process)
                .expect("sample");
        }
        let event_id = storage.list_events(true, 1).expect("events")[0].id;
        let event = storage.event(event_id).expect("event").expect("open event");

        assert_eq!(event.process_evidence.len(), 3);
        assert_eq!(event.process_evidence[0].kind, "prelude");
        assert_eq!(event.process_evidence[2].kind, "trigger");

        storage
            .run_maintenance_with_anomalies(
                &test_retention(),
                &anomaly,
                &process,
                at(12 * 60 * 60 * 1_000),
            )
            .expect("process retention");

        assert_eq!(storage.status().expect("status").processes.row_count, 0);
        assert_eq!(
            storage
                .event(event_id)
                .expect("event")
                .expect("retained event")
                .process_evidence
                .len(),
            3
        );
    }

    #[test]
    fn anomaly_event_survives_restart_and_closes_with_evidence() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let config = test_anomaly_config();
        let event_id;
        {
            let mut storage = Storage::open(&path).expect("open storage");
            let mut engine = storage
                .load_anomaly_engine(&config)
                .expect("new anomaly engine");
            for metric in [
                cpu_metric(0, 91.0),
                cpu_metric(5, 92.0),
                cpu_metric(10, 93.0),
            ] {
                storage
                    .insert_batch_with_anomalies(
                        &CollectionBatch::metrics_only(vec![metric]),
                        &mut engine,
                        &config,
                        &ProcessConfig::default(),
                    )
                    .expect("evaluate sample");
            }
            let events = storage.list_events(true, 20).expect("open events");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].severity, "warning");
            event_id = events[0].id;
            let event = storage.event(event_id).expect("read event").expect("event");
            assert_eq!(event.sample_count, 3);
            assert_eq!(event.evidence.len(), 3);
            assert_eq!(event.evidence.last().expect("trigger").kind, "trigger");
        }

        let mut storage = Storage::open(&path).expect("reopen storage");
        let mut engine = storage
            .load_anomaly_engine(&config)
            .expect("restore anomaly engine");
        for metric in [
            cpu_metric(15, 74.0),
            cpu_metric(20, 74.0),
            cpu_metric(25, 74.0),
        ] {
            storage
                .insert_batch_with_anomalies(
                    &CollectionBatch::metrics_only(vec![metric]),
                    &mut engine,
                    &config,
                    &ProcessConfig::default(),
                )
                .expect("recovery sample");
        }

        assert!(
            storage
                .list_events(true, 20)
                .expect("open events")
                .is_empty()
        );
        let event = storage.event(event_id).expect("read event").expect("event");
        assert_eq!(event.summary.status, "closed");
        assert_eq!(event.sample_count, 6);
        assert_eq!(event.evidence.last().expect("recovery").kind, "recovery");
    }

    #[test]
    fn evidence_outlives_raw_samples_and_event_retention_removes_it() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let mut config = test_anomaly_config();
        config.event_retention_days = 1;
        let process = ProcessConfig {
            interval_seconds: 5,
            raw_retention_hours: 1,
            event_top_n: 1,
            ..ProcessConfig::default()
        };
        let mut engine = storage
            .load_anomaly_engine(&config)
            .expect("anomaly engine");
        for (seconds, value) in [
            (0, 91.0),
            (5, 92.0),
            (10, 93.0),
            (15, 74.0),
            (20, 74.0),
            (25, 74.0),
        ] {
            storage
                .insert_batch_with_anomalies(
                    &CollectionBatch {
                        metrics: vec![cpu_metric(seconds, value)],
                        processes: vec![process_sample(
                            seconds,
                            u32::try_from(seconds + 100).expect("pid"),
                            "test-process",
                            value,
                            100,
                            Some(1),
                            Some(1),
                        )],
                        capabilities: Vec::new(),
                    },
                    &mut engine,
                    &config,
                    &process,
                )
                .expect("sample");
        }
        let event_id = storage.list_events(false, 20).expect("events")[0].id;
        let retention = RetentionConfig {
            raw_hours: 1,
            late_arrival_grace_seconds: 0,
            rollup_batch_buckets: 1_000,
            ..RetentionConfig::default()
        };

        storage
            .run_maintenance_with_anomalies(&retention, &config, &process, at(12 * 60 * 60 * 1_000))
            .expect("raw retention");
        assert_eq!(storage.status().expect("status").raw.row_count, 0);
        assert_eq!(storage.status().expect("status").processes.row_count, 0);
        let retained_event = storage
            .event(event_id)
            .expect("event")
            .expect("retained event");
        assert!(!retained_event.evidence.is_empty());
        assert!(!retained_event.process_evidence.is_empty());

        storage
            .run_maintenance_with_anomalies(
                &retention,
                &config,
                &process,
                at(2 * 24 * 60 * 60 * 1_000),
            )
            .expect("event retention");
        assert!(storage.event(event_id).expect("event query").is_none());
        let evidence_rows: i64 = storage
            .connection
            .query_row("SELECT COUNT(*) FROM anomaly_event_evidence", [], |row| {
                row.get(0)
            })
            .expect("evidence count");
        assert_eq!(evidence_rows, 0);
        let process_evidence_rows: i64 = storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM anomaly_event_process_evidence",
                [],
                |row| row.get(0),
            )
            .expect("process evidence count");
        assert_eq!(process_evidence_rows, 0);
    }

    #[test]
    fn rollups_are_idempotent_and_quarter_hour_average_is_weighted() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        storage
            .insert_batch(&vec![
                Metric::new(at(10_000), "system", "cpu.total.usage", 10.0, "percent"),
                Metric::new(at(20_000), "system", "cpu.total.usage", 20.0, "percent"),
                Metric::new(at(70_000), "system", "cpu.total.usage", 40.0, "percent"),
            ])
            .expect("samples");

        let retention = test_retention();
        storage
            .run_maintenance(&retention, at(1_000_000))
            .expect("first maintenance");
        storage
            .connection
            .execute("DELETE FROM maintenance_state", [])
            .expect("rewind watermarks");
        storage
            .run_maintenance(&retention, at(1_000_000))
            .expect("second maintenance");

        let minute_rows: i64 = storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM metric_rollups WHERE resolution_seconds = 60",
                [],
                |row| row.get(0),
            )
            .expect("minute count");
        let (count, sum, average): (i64, f64, f64) = storage
            .connection
            .query_row(
                "SELECT sample_count, sum_value, average_value FROM metric_rollups
                 WHERE resolution_seconds = 900 AND metric_name = 'cpu.total.usage'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("quarter rollup");

        assert_eq!(minute_rows, 2);
        assert_eq!(count, 3);
        assert_eq!(sum, 70.0);
        assert!((average - (70.0 / 3.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn process_rollups_preserve_resource_attribution_after_raw_retention() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("process-rollups.sqlite3");
        let mut storage = Storage::open(&path).expect("storage");
        let anomaly = AnomalyConfig::default();
        let process = ProcessConfig {
            raw_retention_hours: 1,
            minute_retention_days: 1,
            quarter_hour_retention_days: 2,
            ..ProcessConfig::default()
        };
        let mut engine = storage.load_anomaly_engine(&anomaly).expect("engine");
        let mut writer = process_sample(10, 42, "writer", 30.0, 4096, Some(2), Some(1));
        writer.disk_write_bytes = 15_000;
        writer.disk_write_bytes_per_second = 1_000.0;
        writer.disk_write_rank = Some(1);
        let mut cpu_heavy = process_sample(10, 43, "cpu-heavy", 90.0, 2048, Some(1), Some(2));
        cpu_heavy.disk_write_bytes_per_second = 10.0;
        cpu_heavy.disk_write_rank = Some(2);
        storage
            .insert_batch_with_anomalies(
                &CollectionBatch {
                    processes: vec![writer, cpu_heavy],
                    ..CollectionBatch::default()
                },
                &mut engine,
                &anomaly,
                &process,
            )
            .expect("sample");

        storage
            .run_maintenance_with_anomalies(
                &test_retention(),
                &anomaly,
                &process,
                at(2 * 60 * 60 * 1_000),
            )
            .expect("maintenance");

        assert_eq!(storage.status().expect("status").processes.row_count, 0);
        let rollups: i64 = storage.connection.query_row(
            "SELECT COUNT(*) FROM process_metric_rollups WHERE metric_name='process.disk.write.rate'",
            [],
            |row| row.get(0),
        ).expect("rollups");
        assert!(rollups > 0);
        let snapshot = storage
            .dashboard_snapshot(ReportRange::new(0, 60 * 60 * 1_000).expect("range"))
            .expect("snapshot");
        assert!(snapshot.process_series.iter().any(|series| {
            series.name == "writer"
                && series.disk_write_rank == Some(1)
                && series
                    .points
                    .iter()
                    .any(|point| point.peak_disk_write_bytes_per_second == 1_000.0)
        }));
        assert!(
            snapshot
                .process_series
                .iter()
                .any(|series| series.name == "cpu-heavy" && series.cpu_rank == Some(1))
        );
    }

    #[test]
    fn raw_rows_are_not_deleted_before_their_minute_is_rolled_up() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        storage
            .insert_batch(&vec![Metric::new(
                at(0),
                "system",
                "memory.used",
                1.0,
                "bytes",
            )])
            .expect("sample");
        let retention = RetentionConfig {
            raw_hours: 1,
            late_arrival_grace_seconds: 0,
            rollup_batch_buckets: 1,
            ..RetentionConfig::default()
        };

        storage
            .run_maintenance(&retention, at(2 * 60 * MINUTE_MS))
            .expect("maintenance");

        assert_eq!(storage.status().expect("status").raw.row_count, 0);
        assert_eq!(storage.status().expect("status").minute.row_count, 1);
    }

    #[test]
    fn dashboard_opens_a_current_database_read_only() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("dashboard.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        storage
            .insert_batch(&vec![Metric::new(
                at(10_000),
                "system",
                "cpu.total.usage",
                42.0,
                "percent",
            )])
            .expect("sample");
        drop(storage);

        let read_only = Storage::open_read_only(&path).expect("read-only storage");
        let snapshot = read_only
            .dashboard_snapshot(ReportRange::new(0, 20_000).expect("range"))
            .expect("dashboard snapshot");

        assert_eq!(snapshot.series.len(), 1);
        assert_eq!(snapshot.series[0].average_value, 42.0);
        assert_eq!(
            read_only.status().expect("status").schema_version,
            CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn dashboard_returns_bounded_top_process_chart_series() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("dashboard-processes.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let anomaly = AnomalyConfig::default();
        let process = ProcessConfig::default();
        let mut engine = storage.load_anomaly_engine(&anomaly).expect("engine");
        for seconds in [10, 25] {
            storage
                .insert_batch_with_anomalies(
                    &CollectionBatch {
                        metrics: vec![
                            cpu_metric(seconds, 40.0),
                            Metric::new(
                                at(seconds * 1_000),
                                "system",
                                "memory.total",
                                16.0 * 1024.0 * 1024.0 * 1024.0,
                                "bytes",
                            ),
                            Metric::new(
                                at(seconds * 1_000),
                                "system",
                                "memory.usage",
                                50.0,
                                "percent",
                            ),
                        ],
                        processes: vec![
                            process_sample(
                                seconds,
                                10,
                                "cpu-heavy",
                                80.0,
                                512 * 1024 * 1024,
                                Some(1),
                                Some(2),
                            ),
                            process_sample(
                                seconds,
                                20,
                                "memory-heavy",
                                20.0,
                                2 * 1024 * 1024 * 1024,
                                Some(2),
                                Some(1),
                            ),
                        ],
                        capabilities: Vec::new(),
                    },
                    &mut engine,
                    &anomaly,
                    &process,
                )
                .expect("sample");
        }
        drop(storage);

        let read_only = Storage::open_read_only(&path).expect("read-only storage");
        let snapshot = read_only
            .dashboard_snapshot(ReportRange::new(0, 60_000).expect("range"))
            .expect("dashboard snapshot");

        assert_eq!(snapshot.system_memory_bytes, Some(16 * 1024 * 1024 * 1024));
        assert_eq!(snapshot.process_bucket_span_ms, 15_000);
        assert_eq!(snapshot.process_series.len(), 2);
        assert!(
            snapshot
                .process_series
                .iter()
                .all(|series| series.points.len() == 2)
        );
        assert_eq!(
            snapshot
                .process_series
                .iter()
                .find(|series| series.name == "cpu-heavy")
                .and_then(|series| series.cpu_rank),
            Some(1)
        );
        assert_eq!(
            snapshot
                .process_series
                .iter()
                .find(|series| series.name == "memory-heavy")
                .and_then(|series| series.memory_rank),
            Some(1)
        );
    }

    #[test]
    fn dashboard_does_not_migrate_an_old_database() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("old.sqlite3");
        let old = Connection::open(&path).expect("old database");
        old.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION - 1)
            .expect("old version");
        drop(old);

        let error = Storage::open_read_only(&path)
            .err()
            .expect("old schema is rejected");
        assert!(error.to_string().contains("requires schema version"));
        let unchanged: i64 = Connection::open(&path)
            .expect("reopen old database")
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(unchanged, CURRENT_SCHEMA_VERSION - 1);
    }

    #[test]
    fn rejects_a_database_from_a_newer_schema_version() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("future.sqlite3");
        let future = Connection::open(&path).expect("open future database");
        future
            .pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
            .expect("set future version");
        drop(future);

        let error = Storage::open(&path)
            .err()
            .expect("newer schema is rejected");
        assert!(error.to_string().contains("newer than supported"));
    }
}
