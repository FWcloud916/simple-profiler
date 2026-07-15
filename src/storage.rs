use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::warn;

use crate::{config::RetentionConfig, model::MetricBatch};

const CURRENT_SCHEMA_VERSION: i64 = 2;
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
"#;

pub struct Storage {
    connection: Connection,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetStatus {
    pub row_count: i64,
    pub oldest_ms: Option<i64>,
    pub newest_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageStatus {
    pub schema_version: i64,
    pub raw: DatasetStatus,
    pub minute: DatasetStatus,
    pub quarter_hour: DatasetStatus,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub free_page_bytes: u64,
    pub minute_watermark_ms: Option<i64>,
    pub quarter_hour_watermark_ms: Option<i64>,
    pub last_maintenance_ms: Option<i64>,
    pub last_maintenance_result: Option<String>,
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
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "busy_timeout", 5_000_i64)?;
        migrate(&mut connection)?;
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
            database_bytes: file_size(&self.path),
            wal_bytes: file_size(&PathBuf::from(format!("{}-wal", self.path.display()))),
            free_page_bytes: u64::try_from(page_size.saturating_mul(free_pages)).unwrap_or(0),
            minute_watermark_ms: get_state_integer(&self.connection, MINUTE_WATERMARK)?,
            quarter_hour_watermark_ms: get_state_integer(&self.connection, QUARTER_HOUR_WATERMARK)?,
            last_maintenance_ms: get_state_integer(&self.connection, LAST_MAINTENANCE_MS)?,
            last_maintenance_result: get_state_text(&self.connection, LAST_MAINTENANCE_RESULT)?,
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
    transaction.execute_batch(INDEX_SCHEMA)?;
    transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
    transaction.commit()?;
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
    mut receiver: mpsc::Receiver<MetricBatch>,
) -> JoinHandle<Result<()>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut storage = Storage::open(&path)?;
        let maintenance_interval = Duration::from_secs(retention.maintenance_interval_seconds);
        let mut last_maintenance = Instant::now()
            .checked_sub(maintenance_interval)
            .unwrap_or_else(Instant::now);
        while let Some(batch) = receiver.blocking_recv() {
            storage.insert_batch(&batch)?;
            if last_maintenance.elapsed() >= maintenance_interval {
                if let Err(error) = storage.run_maintenance(&retention, Utc::now()) {
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
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use super::*;
    use crate::model::Metric;

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
