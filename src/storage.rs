use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::model::MetricBatch;

const CURRENT_SCHEMA_VERSION: i64 = 1;
const TABLE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS metric_samples (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    collected_at TEXT NOT NULL,
    collector    TEXT NOT NULL,
    resource     TEXT,
    metric_name  TEXT NOT NULL,
    value        REAL NOT NULL,
    unit         TEXT NOT NULL
);
"#;
const INDEX_SCHEMA: &str = r#"
CREATE INDEX IF NOT EXISTS idx_metric_samples_time
    ON metric_samples(collected_at);
CREATE INDEX IF NOT EXISTS idx_metric_samples_name_time
    ON metric_samples(metric_name, collected_at);
CREATE INDEX IF NOT EXISTS idx_metric_samples_name_resource_time
    ON metric_samples(metric_name, resource, collected_at);
"#;

pub struct Storage {
    connection: Connection,
}

#[derive(Debug, PartialEq, Eq)]
pub struct StorageStatus {
    pub sample_count: i64,
    pub oldest_sample: Option<String>,
    pub newest_sample: Option<String>,
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
        Ok(Self { connection })
    }

    pub fn insert_batch(&mut self, batch: &MetricBatch) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO metric_samples
                 (collected_at, collector, resource, metric_name, value, unit)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for metric in batch {
                statement.execute(params![
                    metric.collected_at.to_rfc3339(),
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

    pub fn status(&self) -> Result<StorageStatus> {
        let mut statement = self
            .connection
            .prepare("SELECT COUNT(*), MIN(collected_at), MAX(collected_at) FROM metric_samples")?;
        let status = statement.query_row([], |row| {
            Ok(StorageStatus {
                sample_count: row.get(0)?,
                oldest_sample: row.get(1)?,
                newest_sample: row.get(2)?,
            })
        })?;
        Ok(status)
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
    transaction.execute_batch(TABLE_SCHEMA)?;
    if !table_has_column(&transaction, "metric_samples", "resource")? {
        transaction.execute("ALTER TABLE metric_samples ADD COLUMN resource TEXT", [])?;
    }
    transaction.execute_batch(INDEX_SCHEMA)?;
    transaction.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
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

pub fn spawn_writer(
    path: &Path,
    mut receiver: mpsc::Receiver<MetricBatch>,
) -> JoinHandle<Result<()>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut storage = Storage::open(&path)?;
        while let Some(batch) = receiver.blocking_recv() {
            storage.insert_batch(&batch)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::model::Metric;

    #[test]
    fn writes_a_batch_and_reports_status() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("metrics.sqlite3");
        let mut storage = Storage::open(&path).expect("open storage");
        let batch = vec![Metric::new(
            Utc::now(),
            "test",
            "cpu.total.usage",
            42.0,
            "percent",
        )];

        storage.insert_batch(&batch).expect("insert batch");
        let status = storage.status().expect("read status");

        assert_eq!(status.sample_count, 1);
        assert!(status.oldest_sample.is_some());
        assert!(status.newest_sample.is_some());
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
    fn upgrades_an_unversioned_database_without_losing_rows() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("legacy.sqlite3");
        let legacy = Connection::open(&path).expect("open legacy database");
        legacy
            .execute_batch(
                r#"
                CREATE TABLE metric_samples (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    collected_at TEXT NOT NULL,
                    collector    TEXT NOT NULL,
                    metric_name  TEXT NOT NULL,
                    value        REAL NOT NULL,
                    unit         TEXT NOT NULL
                );
                INSERT INTO metric_samples
                    (collected_at, collector, metric_name, value, unit)
                VALUES
                    ('2026-07-15T00:00:00Z', 'system', 'memory.used', 1.0, 'bytes');
                "#,
            )
            .expect("create legacy schema");
        drop(legacy);

        let storage = Storage::open(&path).expect("migrate storage");
        let version: i64 = storage
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        let resource: Option<String> = storage
            .connection
            .query_row("SELECT resource FROM metric_samples", [], |row| row.get(0))
            .expect("read legacy row");

        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        assert_eq!(storage.status().expect("status").sample_count, 1);
        assert_eq!(resource, None);
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
