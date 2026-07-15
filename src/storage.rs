use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::model::MetricBatch;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS metric_samples (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    collected_at TEXT NOT NULL,
    collector    TEXT NOT NULL,
    metric_name  TEXT NOT NULL,
    value        REAL NOT NULL,
    unit         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_metric_samples_time
    ON metric_samples(collected_at);
CREATE INDEX IF NOT EXISTS idx_metric_samples_name_time
    ON metric_samples(metric_name, collected_at);
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

        let connection = Connection::open(path)
            .with_context(|| format!("failed to open SQLite database {}", path.display()))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "busy_timeout", 5_000_i64)?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    pub fn insert_batch(&mut self, batch: &MetricBatch) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO metric_samples
                 (collected_at, collector, metric_name, value, unit)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for metric in batch {
                statement.execute(params![
                    metric.collected_at.to_rfc3339(),
                    metric.collector,
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
}
