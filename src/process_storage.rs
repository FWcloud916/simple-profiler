use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::{config::ProcessConfig, model::ProcessSnapshot};

const PROCESS_MINUTE_WATERMARK: &str = "process_rollup_60_watermark_ms";
const PROCESS_QUARTER_WATERMARK: &str = "process_rollup_900_watermark_ms";
const MINUTE_MS: i64 = 60_000;
const QUARTER_HOUR_MS: i64 = 900_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSort {
    Cpu,
    Memory,
    DiskRead,
    DiskWrite,
    NetworkReceive,
    NetworkTransmit,
    Gpu,
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
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub disk_read_bytes_per_second: f64,
    pub disk_write_bytes_per_second: f64,
    pub network_receive_bytes: Option<u64>,
    pub network_transmit_bytes: Option<u64>,
    pub network_receive_bytes_per_second: Option<f64>,
    pub network_transmit_bytes_per_second: Option<f64>,
    pub gpu_time_ns: Option<u64>,
    pub gpu_usage_percent: Option<f64>,
    pub cpu_rank: Option<u32>,
    pub memory_rank: Option<u32>,
    pub disk_read_rank: Option<u32>,
    pub disk_write_rank: Option<u32>,
    pub network_receive_rank: Option<u32>,
    pub network_transmit_rank: Option<u32>,
    pub gpu_rank: Option<u32>,
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
    DiskRead,
    DiskWrite,
    NetworkReceive,
    NetworkTransmit,
    Gpu,
}

pub(crate) fn insert_samples(
    transaction: &Transaction<'_>,
    samples: &ProcessSnapshot,
) -> Result<()> {
    let mut statement = transaction.prepare_cached(
        "INSERT INTO process_samples
         (collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
          executable_path, cpu_usage_percent, memory_bytes,
          disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
          network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
          network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
          cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
          network_transmit_rank, gpu_rank)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
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
            u64_to_integer(sample.disk_read_bytes),
            u64_to_integer(sample.disk_write_bytes),
            sample.disk_read_bytes_per_second,
            sample.disk_write_bytes_per_second,
            sample.network_receive_bytes.map(u64_to_integer),
            sample.network_transmit_bytes.map(u64_to_integer),
            sample.network_receive_bytes_per_second,
            sample.network_transmit_bytes_per_second,
            sample.gpu_time_ns.map(u64_to_integer),
            sample.gpu_usage_percent,
            sample.cpu_rank.map(i64::from),
            sample.memory_rank.map(i64::from),
            sample.disk_read_rank.map(i64::from),
            sample.disk_write_rank.map(i64::from),
            sample.network_receive_rank.map(i64::from),
            sample.network_transmit_rank.map(i64::from),
            sample.gpu_rank.map(i64::from),
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
          name, executable_path, cpu_usage_percent, memory_bytes,
          disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
          network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
          network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
          cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
          network_transmit_rank, gpu_rank)
         SELECT ?1, 'prelude', collected_at_ms, pid, process_start_time_seconds, parent_pid,
                name, executable_path, cpu_usage_percent, memory_bytes,
                disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
                network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
                network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
                cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
                network_transmit_rank, gpu_rank
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
          name, executable_path, cpu_usage_percent, memory_bytes,
          disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
          network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
          network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
          cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
          network_transmit_rank, gpu_rank)
         SELECT ?1, ?2, collected_at_ms, pid, process_start_time_seconds, parent_pid,
                name, executable_path, cpu_usage_percent, memory_bytes,
                disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
                network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
                network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
                cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
                network_transmit_rank, gpu_rank
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
        "disk.io.read.rate" => Some(AttributionDimension::DiskRead),
        "disk.io.write.rate" => Some(AttributionDimension::DiskWrite),
        "network.receive.rate" => Some(AttributionDimension::NetworkReceive),
        "network.transmit.rate" => Some(AttributionDimension::NetworkTransmit),
        "gpu.device.usage" | "gpu.renderer.usage" | "gpu.tiler.usage" => {
            Some(AttributionDimension::Gpu)
        }
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
        AttributionDimension::DiskRead => ("disk_read_rank", "disk_read_rank"),
        AttributionDimension::DiskWrite => ("disk_write_rank", "disk_write_rank"),
        AttributionDimension::NetworkReceive => ("network_receive_rank", "network_receive_rank"),
        AttributionDimension::NetworkTransmit => ("network_transmit_rank", "network_transmit_rank"),
        AttributionDimension::Gpu => ("gpu_rank", "gpu_rank"),
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProcessMaintenance {
    pub raw: usize,
    pub minute: usize,
    pub quarter_hour: usize,
}

pub(crate) fn run_maintenance(
    transaction: &Transaction<'_>,
    config: &ProcessConfig,
    now_ms: i64,
    late_arrival_grace_seconds: u64,
    rollup_batch_buckets: usize,
    delete_batch_rows: usize,
) -> Result<ProcessMaintenance> {
    let minute_watermark = roll_up_process_minutes(
        transaction,
        now_ms,
        late_arrival_grace_seconds,
        rollup_batch_buckets,
    )?;
    let quarter_watermark = roll_up_process_quarters(
        transaction,
        now_ms,
        late_arrival_grace_seconds,
        rollup_batch_buckets,
        minute_watermark,
    )?;
    let raw_cutoff = now_ms
        .saturating_sub(hours_ms(config.raw_retention_hours))
        .min(minute_watermark.unwrap_or(i64::MIN));
    let raw = delete_process_raw(transaction, raw_cutoff, delete_batch_rows)?;
    let minute_cutoff = now_ms.saturating_sub(days_ms(config.minute_retention_days));
    let minute = if let Some(watermark) = quarter_watermark {
        delete_process_rollups(
            transaction,
            60,
            minute_cutoff.min(watermark),
            delete_batch_rows,
        )?
    } else {
        0
    };
    let quarter_hour = delete_process_rollups(
        transaction,
        900,
        now_ms.saturating_sub(days_ms(config.quarter_hour_retention_days)),
        delete_batch_rows,
    )?;
    Ok(ProcessMaintenance {
        raw,
        minute,
        quarter_hour,
    })
}

fn roll_up_process_minutes(
    connection: &Connection,
    now_ms: i64,
    grace_seconds: u64,
    batch_buckets: usize,
) -> Result<Option<i64>> {
    let Some(mut cursor) = process_watermark(
        connection,
        PROCESS_MINUTE_WATERMARK,
        "SELECT MIN(collected_at_ms) FROM process_samples",
        MINUTE_MS,
    )?
    else {
        return Ok(None);
    };
    let cutoff = floor_bucket(now_ms.saturating_sub(seconds_ms(grace_seconds)), MINUTE_MS);
    for _ in 0..batch_buckets {
        if cursor >= cutoff {
            break;
        }
        aggregate_process_raw_bucket(connection, cursor, cursor + MINUTE_MS, 60)?;
        cursor += MINUTE_MS;
        set_process_watermark(connection, PROCESS_MINUTE_WATERMARK, cursor)?;
    }
    Ok(Some(cursor))
}

fn roll_up_process_quarters(
    connection: &Connection,
    now_ms: i64,
    grace_seconds: u64,
    batch_buckets: usize,
    minute_watermark: Option<i64>,
) -> Result<Option<i64>> {
    let Some(minute_watermark) = minute_watermark else {
        return get_process_watermark(connection, PROCESS_QUARTER_WATERMARK);
    };
    let Some(mut cursor) = process_watermark(
        connection,
        PROCESS_QUARTER_WATERMARK,
        "SELECT MIN(bucket_start_ms) FROM process_metric_rollups WHERE resolution_seconds = 60",
        QUARTER_HOUR_MS,
    )?
    else {
        return Ok(None);
    };
    let cutoff = floor_bucket(
        now_ms.saturating_sub(seconds_ms(grace_seconds)),
        QUARTER_HOUR_MS,
    )
    .min(floor_bucket(minute_watermark, QUARTER_HOUR_MS));
    for _ in 0..batch_buckets {
        if cursor >= cutoff {
            break;
        }
        aggregate_process_rollup_bucket(connection, cursor, cursor + QUARTER_HOUR_MS)?;
        cursor += QUARTER_HOUR_MS;
        set_process_watermark(connection, PROCESS_QUARTER_WATERMARK, cursor)?;
    }
    Ok(Some(cursor))
}

const PROCESS_METRICS: &[(&str, &str, &str)] = &[
    ("process.cpu.usage", "cpu_usage_percent", "cpu_rank"),
    ("process.memory.bytes", "memory_bytes", "memory_rank"),
    (
        "process.disk.read.rate",
        "disk_read_bytes_per_second",
        "disk_read_rank",
    ),
    (
        "process.disk.write.rate",
        "disk_write_bytes_per_second",
        "disk_write_rank",
    ),
    (
        "process.network.receive.rate",
        "network_receive_bytes_per_second",
        "network_receive_rank",
    ),
    (
        "process.network.transmit.rate",
        "network_transmit_bytes_per_second",
        "network_transmit_rank",
    ),
    ("process.gpu.usage", "gpu_usage_percent", "gpu_rank"),
];

fn process_metric_unit(name: &str) -> &'static str {
    match name {
        "process.cpu.usage" | "process.gpu.usage" => "percent",
        "process.memory.bytes" => "bytes",
        _ => "bytes_per_second",
    }
}

fn aggregate_process_raw_bucket(
    connection: &Connection,
    start_ms: i64,
    end_ms: i64,
    resolution_seconds: i64,
) -> Result<()> {
    for (metric_name, value_column, rank_column) in PROCESS_METRICS {
        let sql = format!(
            "INSERT INTO process_metric_rollups
             (bucket_start_ms, resolution_seconds, pid, process_start_time_seconds, name,
              metric_name, unit, sample_count, min_value, max_value, sum_value,
              average_value, last_value, peak_rank)
             SELECT ?1, ?2, pid, process_start_time_seconds, MAX(name), ?3, ?4,
                    COUNT({value_column}), MIN({value_column}), MAX({value_column}),
                    SUM({value_column}), AVG({value_column}), AVG({value_column}), MIN({rank_column})
             FROM process_samples
             WHERE collected_at_ms >= ?1 AND collected_at_ms < ?5
               AND {value_column} IS NOT NULL
             GROUP BY pid, process_start_time_seconds
             ON CONFLICT(resolution_seconds, bucket_start_ms, pid, process_start_time_seconds, metric_name)
             DO UPDATE SET name=excluded.name, unit=excluded.unit,
               sample_count=excluded.sample_count, min_value=excluded.min_value,
               max_value=excluded.max_value, sum_value=excluded.sum_value,
               average_value=excluded.average_value, last_value=excluded.last_value,
               peak_rank=excluded.peak_rank"
        );
        connection.execute(
            &sql,
            params![
                start_ms,
                resolution_seconds,
                metric_name,
                process_metric_unit(metric_name),
                end_ms
            ],
        )?;
    }
    Ok(())
}

fn aggregate_process_rollup_bucket(
    connection: &Connection,
    start_ms: i64,
    end_ms: i64,
) -> Result<()> {
    connection.execute(
        "INSERT INTO process_metric_rollups
         (bucket_start_ms, resolution_seconds, pid, process_start_time_seconds, name,
          metric_name, unit, sample_count, min_value, max_value, sum_value,
          average_value, last_value, peak_rank)
         SELECT ?1, 900, pid, process_start_time_seconds, MAX(name), metric_name, MAX(unit),
                SUM(sample_count), MIN(min_value), MAX(max_value), SUM(sum_value),
                SUM(sum_value) / SUM(sample_count), SUM(sum_value) / SUM(sample_count), MIN(peak_rank)
         FROM process_metric_rollups
         WHERE resolution_seconds = 60 AND bucket_start_ms >= ?1 AND bucket_start_ms < ?2
         GROUP BY pid, process_start_time_seconds, metric_name
         ON CONFLICT(resolution_seconds, bucket_start_ms, pid, process_start_time_seconds, metric_name)
         DO UPDATE SET name=excluded.name, unit=excluded.unit,
           sample_count=excluded.sample_count, min_value=excluded.min_value,
           max_value=excluded.max_value, sum_value=excluded.sum_value,
           average_value=excluded.average_value, last_value=excluded.last_value,
           peak_rank=excluded.peak_rank",
        params![start_ms, end_ms],
    )?;
    Ok(())
}

fn process_watermark(
    connection: &Connection,
    key: &str,
    sql: &str,
    bucket_ms: i64,
) -> Result<Option<i64>> {
    if let Some(value) = get_process_watermark(connection, key)? {
        return Ok(Some(value));
    }
    let minimum: Option<i64> = connection.query_row(sql, [], |row| row.get(0))?;
    Ok(minimum.map(|value| floor_bucket(value, bucket_ms)))
}

fn get_process_watermark(connection: &Connection, key: &str) -> Result<Option<i64>> {
    Ok(connection
        .query_row(
            "SELECT value_integer FROM maintenance_state WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

fn set_process_watermark(connection: &Connection, key: &str, value: i64) -> Result<()> {
    connection.execute("INSERT INTO maintenance_state(key, value_integer) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value_integer=excluded.value_integer", params![key, value])?;
    Ok(())
}

fn delete_process_raw(connection: &Connection, cutoff: i64, batch_rows: usize) -> Result<usize> {
    Ok(connection.execute("DELETE FROM process_samples WHERE id IN (SELECT id FROM process_samples WHERE collected_at_ms < ?1 ORDER BY collected_at_ms LIMIT ?2)", params![cutoff, usize_to_integer(batch_rows)])?)
}

fn delete_process_rollups(
    connection: &Connection,
    resolution: i64,
    cutoff: i64,
    batch_rows: usize,
) -> Result<usize> {
    Ok(connection.execute("DELETE FROM process_metric_rollups WHERE rowid IN (SELECT rowid FROM process_metric_rollups WHERE resolution_seconds = ?1 AND bucket_start_ms < ?2 ORDER BY bucket_start_ms LIMIT ?3)", params![resolution, cutoff, usize_to_integer(batch_rows)])?)
}

fn floor_bucket(value: i64, bucket_ms: i64) -> i64 {
    value.div_euclid(bucket_ms) * bucket_ms
}
fn seconds_ms(value: u64) -> i64 {
    u64_to_integer(value).saturating_mul(1_000)
}
fn hours_ms(value: u64) -> i64 {
    seconds_ms(value.saturating_mul(3_600))
}
fn days_ms(value: u64) -> i64 {
    hours_ms(value.saturating_mul(24))
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
        ProcessSort::DiskRead => AttributionDimension::DiskRead,
        ProcessSort::DiskWrite => AttributionDimension::DiskWrite,
        ProcessSort::NetworkReceive => AttributionDimension::NetworkReceive,
        ProcessSort::NetworkTransmit => AttributionDimension::NetworkTransmit,
        ProcessSort::Gpu => AttributionDimension::Gpu,
    });
    let sql = format!(
        "SELECT collected_at_ms, pid, process_start_time_seconds, parent_pid, name,
                executable_path, cpu_usage_percent, memory_bytes,
                disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
                network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
                network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
                cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
                network_transmit_rank, gpu_rank
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
                executable_path, cpu_usage_percent, memory_bytes,
                disk_read_bytes, disk_write_bytes, disk_read_bytes_per_second, disk_write_bytes_per_second,
                network_receive_bytes, network_transmit_bytes, network_receive_bytes_per_second,
                network_transmit_bytes_per_second, gpu_time_ns, gpu_usage_percent,
                cpu_rank, memory_rank, disk_read_rank, disk_write_rank, network_receive_rank,
                network_transmit_rank, gpu_rank
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
                disk_read_bytes: integer_to_u64(row.get(9)?),
                disk_write_bytes: integer_to_u64(row.get(10)?),
                disk_read_bytes_per_second: row.get(11)?,
                disk_write_bytes_per_second: row.get(12)?,
                network_receive_bytes: row.get::<_, Option<i64>>(13)?.map(integer_to_u64),
                network_transmit_bytes: row.get::<_, Option<i64>>(14)?.map(integer_to_u64),
                network_receive_bytes_per_second: row.get(15)?,
                network_transmit_bytes_per_second: row.get(16)?,
                gpu_time_ns: row.get::<_, Option<i64>>(17)?.map(integer_to_u64),
                gpu_usage_percent: row.get(18)?,
                cpu_rank: row.get::<_, Option<i64>>(19)?.map(integer_to_u32),
                memory_rank: row.get::<_, Option<i64>>(20)?.map(integer_to_u32),
                disk_read_rank: row.get::<_, Option<i64>>(21)?.map(integer_to_u32),
                disk_write_rank: row.get::<_, Option<i64>>(22)?.map(integer_to_u32),
                network_receive_rank: row.get::<_, Option<i64>>(23)?.map(integer_to_u32),
                network_transmit_rank: row.get::<_, Option<i64>>(24)?.map(integer_to_u32),
                gpu_rank: row.get::<_, Option<i64>>(25)?.map(integer_to_u32),
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
        disk_read_bytes: integer_to_u64(row.get(8)?),
        disk_write_bytes: integer_to_u64(row.get(9)?),
        disk_read_bytes_per_second: row.get(10)?,
        disk_write_bytes_per_second: row.get(11)?,
        network_receive_bytes: row.get::<_, Option<i64>>(12)?.map(integer_to_u64),
        network_transmit_bytes: row.get::<_, Option<i64>>(13)?.map(integer_to_u64),
        network_receive_bytes_per_second: row.get(14)?,
        network_transmit_bytes_per_second: row.get(15)?,
        gpu_time_ns: row.get::<_, Option<i64>>(16)?.map(integer_to_u64),
        gpu_usage_percent: row.get(17)?,
        cpu_rank: row.get::<_, Option<i64>>(18)?.map(integer_to_u32),
        memory_rank: row.get::<_, Option<i64>>(19)?.map(integer_to_u32),
        disk_read_rank: row.get::<_, Option<i64>>(20)?.map(integer_to_u32),
        disk_write_rank: row.get::<_, Option<i64>>(21)?.map(integer_to_u32),
        network_receive_rank: row.get::<_, Option<i64>>(22)?.map(integer_to_u32),
        network_transmit_rank: row.get::<_, Option<i64>>(23)?.map(integer_to_u32),
        gpu_rank: row.get::<_, Option<i64>>(24)?.map(integer_to_u32),
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
    fn system_metrics_map_to_the_matching_process_dimension() {
        assert!(matches!(
            attribution_dimension("cpu.total.usage"),
            Some(AttributionDimension::Cpu)
        ));
        assert!(matches!(
            attribution_dimension("memory.usage"),
            Some(AttributionDimension::Memory)
        ));
        assert!(matches!(
            attribution_dimension("disk.io.read.rate"),
            Some(AttributionDimension::DiskRead)
        ));
        assert!(matches!(
            attribution_dimension("network.transmit.rate"),
            Some(AttributionDimension::NetworkTransmit)
        ));
        assert!(matches!(
            attribution_dimension("gpu.device.usage"),
            Some(AttributionDimension::Gpu)
        ));
        assert!(attribution_dimension("disk.space.used.percent").is_none());
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
