use anyhow::Result;
use rusqlite::{Connection, Transaction, params};
use serde::Serialize;

use crate::{config::ProcessConfig, model::ProcessSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSort {
    Cpu,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredProcessSample {
    pub collected_at_ms: i64,
    pub pid: u32,
    pub process_start_time_seconds: u64,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub executable_path: Option<String>,
    pub cpu_usage_percent: f64,
    pub memory_bytes: u64,
    pub cpu_rank: Option<u32>,
    pub memory_rank: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessEventEvidence {
    pub kind: String,
    pub sample: StoredProcessSample,
}

#[derive(Debug, Clone, Copy)]
enum AttributionDimension {
    Cpu,
    Memory,
}

pub(crate) fn insert_samples(
    transaction: &Transaction<'_>,
    samples: &ProcessSnapshot,
) -> Result<()> {
    let mut statement = transaction.prepare_cached(
        "INSERT INTO process_samples
         (collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
          executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for sample in samples {
        statement.execute(params![
            sample.collected_at.timestamp_millis(),
            i64::from(sample.pid),
            u64_to_integer(sample.process_start_time_seconds),
            sample.parent_pid.map(i64::from),
            sample.name,
            sample.executable_path,
            sample.cpu_usage_percent,
            u64_to_integer(sample.memory_bytes),
            sample.cpu_rank.map(i64::from),
            sample.memory_rank.map(i64::from),
        ])?;
    }
    Ok(())
}

pub(crate) fn capture_open_event(
    transaction: &Transaction<'_>,
    event_id: i64,
    metric_name: &str,
    detected_at_ms: i64,
    prelude_minutes: u64,
    config: &ProcessConfig,
) -> Result<()> {
    let Some(dimension) = attribution_dimension(metric_name) else {
        return Ok(());
    };
    let Some(trigger_time) = latest_snapshot_at_or_before(transaction, detected_at_ms)? else {
        return Ok(());
    };
    if !snapshot_is_fresh(trigger_time, detected_at_ms, config) {
        return Ok(());
    }
    insert_snapshot_evidence(
        transaction,
        event_id,
        dimension,
        trigger_time,
        "trigger",
        config,
    )?;
    let remaining = remaining_evidence_rows(transaction, event_id, config)?;
    if remaining == 0 {
        return Ok(());
    }
    let prelude_start = detected_at_ms.saturating_sub(
        u64_to_integer(prelude_minutes)
            .saturating_mul(60)
            .saturating_mul(1_000),
    );
    let (rank_column, rank_order) = dimension_sql(dimension);
    let sql = format!(
        "INSERT OR IGNORE INTO anomaly_event_process_evidence
         (event_id, kind, collected_at_ms, pid, process_start_time_seconds, parent_pid,
          name, executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank)
         SELECT ?1, 'prelude', collected_at_ms, pid, process_start_time_seconds, parent_pid,
                name, executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank
         FROM process_samples
         WHERE collected_at_ms >= ?2 AND collected_at_ms < ?3
           AND {rank_column} IS NOT NULL AND {rank_column} <= ?4
         ORDER BY collected_at_ms DESC, {rank_order}, pid
         LIMIT ?5"
    );
    transaction.execute(
        &sql,
        params![
            event_id,
            prelude_start,
            trigger_time,
            usize_to_integer(config.event_top_n),
            usize_to_integer(remaining),
        ],
    )?;
    Ok(())
}

pub(crate) fn capture_checkpoint(
    transaction: &Transaction<'_>,
    event_id: i64,
    metric_name: &str,
    timestamp_ms: i64,
    kind: &str,
    config: &ProcessConfig,
) -> Result<()> {
    let Some(dimension) = attribution_dimension(metric_name) else {
        return Ok(());
    };
    let Some(snapshot_time) = latest_snapshot_at_or_before(transaction, timestamp_ms)? else {
        return Ok(());
    };
    if !snapshot_is_fresh(snapshot_time, timestamp_ms, config) {
        return Ok(());
    }
    insert_snapshot_evidence(
        transaction,
        event_id,
        dimension,
        snapshot_time,
        kind,
        config,
    )
}

fn insert_snapshot_evidence(
    transaction: &Transaction<'_>,
    event_id: i64,
    dimension: AttributionDimension,
    snapshot_time: i64,
    kind: &str,
    config: &ProcessConfig,
) -> Result<()> {
    let remaining = remaining_evidence_rows(transaction, event_id, config)?;
    if remaining == 0 {
        return Ok(());
    }
    let limit = remaining.min(config.event_top_n);
    let (rank_column, rank_order) = dimension_sql(dimension);
    let sql = format!(
        "INSERT OR IGNORE INTO anomaly_event_process_evidence
         (event_id, kind, collected_at_ms, pid, process_start_time_seconds, parent_pid,
          name, executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank)
         SELECT ?1, ?2, collected_at_ms, pid, process_start_time_seconds, parent_pid,
                name, executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank
         FROM process_samples
         WHERE collected_at_ms = ?3 AND {rank_column} IS NOT NULL AND {rank_column} <= ?4
         ORDER BY {rank_order}, pid LIMIT ?5"
    );
    transaction.execute(
        &sql,
        params![
            event_id,
            kind,
            snapshot_time,
            usize_to_integer(config.event_top_n),
            usize_to_integer(limit),
        ],
    )?;
    Ok(())
}

fn remaining_evidence_rows(
    transaction: &Transaction<'_>,
    event_id: i64,
    config: &ProcessConfig,
) -> Result<usize> {
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM anomaly_event_process_evidence WHERE event_id = ?1",
        [event_id],
        |row| row.get(0),
    )?;
    Ok(config
        .event_evidence_max_rows
        .saturating_sub(usize::try_from(count).unwrap_or(usize::MAX)))
}

fn latest_snapshot_at_or_before(connection: &Connection, timestamp_ms: i64) -> Result<Option<i64>> {
    Ok(connection.query_row(
        "SELECT MAX(collected_at_ms) FROM process_samples WHERE collected_at_ms <= ?1",
        [timestamp_ms],
        |row| row.get(0),
    )?)
}

fn attribution_dimension(metric_name: &str) -> Option<AttributionDimension> {
    match metric_name {
        "cpu.total.usage" => Some(AttributionDimension::Cpu),
        "memory.usage" => Some(AttributionDimension::Memory),
        _ => None,
    }
}

fn snapshot_is_fresh(snapshot_ms: i64, event_ms: i64, config: &ProcessConfig) -> bool {
    let maximum_age_ms = u64_to_integer(config.interval_seconds)
        .saturating_mul(2)
        .saturating_mul(1_000);
    event_ms.saturating_sub(snapshot_ms) <= maximum_age_ms
}

fn dimension_sql(dimension: AttributionDimension) -> (&'static str, &'static str) {
    match dimension {
        AttributionDimension::Cpu => ("cpu_rank", "cpu_rank"),
        AttributionDimension::Memory => ("memory_rank", "memory_rank"),
    }
}

pub(crate) fn delete_expired_samples(
    transaction: &Transaction<'_>,
    config: &ProcessConfig,
    now_ms: i64,
    batch_rows: usize,
) -> Result<usize> {
    let cutoff = now_ms.saturating_sub(
        u64_to_integer(config.raw_retention_hours)
            .saturating_mul(60)
            .saturating_mul(60)
            .saturating_mul(1_000),
    );
    Ok(transaction.execute(
        "DELETE FROM process_samples WHERE id IN (
             SELECT id FROM process_samples WHERE collected_at_ms < ?1
             ORDER BY collected_at_ms LIMIT ?2
         )",
        params![cutoff, usize_to_integer(batch_rows)],
    )?)
}

pub(crate) fn latest_top(
    connection: &Connection,
    sort: ProcessSort,
    limit: usize,
) -> Result<Vec<StoredProcessSample>> {
    let Some(timestamp_ms) = latest_snapshot_at_or_before(connection, i64::MAX)? else {
        return Ok(Vec::new());
    };
    let (rank_column, rank_order) = dimension_sql(match sort {
        ProcessSort::Cpu => AttributionDimension::Cpu,
        ProcessSort::Memory => AttributionDimension::Memory,
    });
    let sql = format!(
        "SELECT collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
                executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank
         FROM process_samples
         WHERE collected_at_ms = ?1 AND {rank_column} IS NOT NULL
         ORDER BY {rank_order}, pid LIMIT ?2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params![timestamp_ms, usize_to_integer(limit)], read_sample)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn event_evidence(
    connection: &Connection,
    event_id: i64,
) -> Result<Vec<ProcessEventEvidence>> {
    let mut statement = connection.prepare(
        "SELECT kind, collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
                executable_path, cpu_usage_percent, memory_bytes, cpu_rank, memory_rank
         FROM anomaly_event_process_evidence
         WHERE event_id = ?1
         ORDER BY collected_at_ms, kind, COALESCE(cpu_rank, memory_rank), pid",
    )?;
    let rows = statement.query_map([event_id], |row| {
        Ok(ProcessEventEvidence {
            kind: row.get(0)?,
            sample: StoredProcessSample {
                collected_at_ms: row.get(1)?,
                pid: integer_to_u32(row.get(2)?),
                process_start_time_seconds: integer_to_u64(row.get(3)?),
                parent_pid: row.get::<_, Option<i64>>(4)?.map(integer_to_u32),
                name: row.get(5)?,
                executable_path: row.get(6)?,
                cpu_usage_percent: row.get(7)?,
                memory_bytes: integer_to_u64(row.get(8)?),
                cpu_rank: row.get::<_, Option<i64>>(9)?.map(integer_to_u32),
                memory_rank: row.get::<_, Option<i64>>(10)?.map(integer_to_u32),
            },
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn read_sample(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredProcessSample> {
    Ok(StoredProcessSample {
        collected_at_ms: row.get(0)?,
        pid: integer_to_u32(row.get(1)?),
        process_start_time_seconds: integer_to_u64(row.get(2)?),
        parent_pid: row.get::<_, Option<i64>>(3)?.map(integer_to_u32),
        name: row.get(4)?,
        executable_path: row.get(5)?,
        cpu_usage_percent: row.get(6)?,
        memory_bytes: integer_to_u64(row.get(7)?),
        cpu_rank: row.get::<_, Option<i64>>(8)?.map(integer_to_u32),
        memory_rank: row.get::<_, Option<i64>>(9)?.map(integer_to_u32),
    })
}

fn integer_to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

fn integer_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn u64_to_integer(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn usize_to_integer(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cpu_and_memory_metrics_are_attributed_to_processes() {
        assert!(matches!(
            attribution_dimension("cpu.total.usage"),
            Some(AttributionDimension::Cpu)
        ));
        assert!(matches!(
            attribution_dimension("memory.usage"),
            Some(AttributionDimension::Memory)
        ));
        assert!(attribution_dimension("disk.space.used.percent").is_none());
        assert!(attribution_dimension("disk.io.read.bytes_per_second").is_none());
    }

    #[test]
    fn rejects_process_snapshots_older_than_two_collection_intervals() {
        let config = ProcessConfig {
            interval_seconds: 15,
            ..ProcessConfig::default()
        };

        assert!(snapshot_is_fresh(0, 30_000, &config));
        assert!(!snapshot_is_fresh(0, 30_001, &config));
    }
}
