use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    anomaly_storage, capability_storage,
    report::{
        DashboardProcessPoint, DashboardProcessSeries, DashboardSnapshot, MAX_CHART_POINTS,
        ReportData, ReportPoint, ReportProcessSummary, ReportRange, ReportResolution, ReportSeries,
    },
};

const EVENT_LIMIT: usize = 200;
const PROCESS_LIMIT_PER_DIMENSION: usize = 20;
const PROCESS_CHART_LIMIT_PER_DIMENSION: usize = 3;
const MAX_PROCESS_CHART_POINTS: i64 = 360;
const MIN_PROCESS_BUCKET_MS: i64 = 15_000;
const REPORT_METRICS: &str = r#"
    'cpu.total.usage',
    'memory.usage',
    'disk.space.usage',
    'disk.io.read.rate',
    'disk.io.write.rate',
    'network.receive.rate',
    'network.transmit.rate',
    'gpu.device.usage',
    'gpu.renderer.usage',
    'gpu.tiler.usage',
    'gpu.memory.used',
    'gpu.memory.allocated'
"#;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SeriesKey {
    collector: String,
    resource: String,
    metric_name: String,
    unit: String,
}

#[derive(Debug)]
struct ProcessSeriesSelection {
    pid: u32,
    process_start_time_seconds: u64,
    name: String,
    cpu_rank: Option<u8>,
    memory_rank: Option<u8>,
    disk_read_rank: Option<u8>,
    disk_write_rank: Option<u8>,
    network_receive_rank: Option<u8>,
    network_transmit_rank: Option<u8>,
    gpu_rank: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
enum ProcessDimension {
    Cpu,
    Memory,
    DiskRead,
    DiskWrite,
    NetworkReceive,
    NetworkTransmit,
    Gpu,
}

pub(crate) fn load_report(connection: &Connection, range: ReportRange) -> Result<ReportData> {
    let resolution = select_resolution(connection, range)?;
    let bucket_span_ms = chart_bucket_span_ms(range, resolution);
    let series = query_series(connection, range, resolution, bucket_span_ms)?;
    let metric_oldest_ms = series
        .iter()
        .flat_map(|series| series.points.first())
        .map(|point| point.timestamp_ms)
        .min();
    let metric_newest_ms = series
        .iter()
        .flat_map(|series| series.points.last())
        .map(|point| point.timestamp_ms)
        .max();
    let (events, events_truncated) = query_events(connection, range)?;
    let processes = query_processes(connection, range)?;
    let (process_oldest_ms, process_newest_ms) = process_coverage(connection, range)?;
    Ok(ReportData {
        generated_at_ms: Utc::now().timestamp_millis(),
        range,
        resolution,
        bucket_span_ms,
        metric_oldest_ms,
        metric_newest_ms,
        process_oldest_ms,
        process_newest_ms,
        series,
        events,
        events_truncated,
        processes,
        capabilities: capability_storage::list(connection)?,
    })
}

pub(crate) fn load_dashboard_snapshot(
    connection: &Connection,
    range: ReportRange,
) -> Result<DashboardSnapshot> {
    let resolution = select_resolution(connection, range)?;
    let bucket_span_ms = chart_bucket_span_ms(range, resolution);
    let series = query_series(connection, range, resolution, bucket_span_ms)?;
    let metric_oldest_ms = series
        .iter()
        .flat_map(|series| series.points.first())
        .map(|point| point.timestamp_ms)
        .min();
    let metric_newest_ms = series
        .iter()
        .flat_map(|series| series.points.last())
        .map(|point| point.timestamp_ms)
        .max();
    let (events, events_truncated) =
        anomaly_storage::list_events_in_range(connection, range.from_ms, range.to_ms, EVENT_LIMIT)?;
    let processes = query_processes(connection, range)?;
    let (process_bucket_span_ms, system_memory_bytes, process_series) =
        query_process_series(connection, range)?;
    let (process_oldest_ms, process_newest_ms) = process_coverage(connection, range)?;
    Ok(DashboardSnapshot {
        generated_at_ms: Utc::now().timestamp_millis(),
        range,
        resolution,
        bucket_span_ms,
        metric_oldest_ms,
        metric_newest_ms,
        process_oldest_ms,
        process_newest_ms,
        series,
        events,
        events_truncated,
        processes,
        process_bucket_span_ms,
        system_memory_bytes,
        process_series,
    })
}

fn select_resolution(connection: &Connection, range: ReportRange) -> Result<ReportResolution> {
    let preferred = range.preferred_resolution();
    let candidates = match preferred {
        ReportResolution::Raw => [
            ReportResolution::Raw,
            ReportResolution::Minute,
            ReportResolution::QuarterHour,
        ],
        ReportResolution::Minute => [
            ReportResolution::Minute,
            ReportResolution::QuarterHour,
            ReportResolution::Raw,
        ],
        ReportResolution::QuarterHour => [
            ReportResolution::QuarterHour,
            ReportResolution::Minute,
            ReportResolution::Raw,
        ],
    };
    for resolution in candidates {
        if has_metric_data(connection, range, resolution)? {
            return Ok(resolution);
        }
    }
    Ok(preferred)
}

fn has_metric_data(
    connection: &Connection,
    range: ReportRange,
    resolution: ReportResolution,
) -> Result<bool> {
    let count: i64 = match resolution {
        ReportResolution::Raw => connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM metric_samples
                 WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
                   AND metric_name IN ({REPORT_METRICS})"
            ),
            params![range.from_ms, range.to_ms],
            |row| row.get(0),
        )?,
        ReportResolution::Minute | ReportResolution::QuarterHour => connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM metric_rollups
                 WHERE resolution_seconds = ?1
                   AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3
                   AND metric_name IN ({REPORT_METRICS})"
            ),
            params![resolution.seconds(), range.from_ms, range.to_ms],
            |row| row.get(0),
        )?,
    };
    Ok(count > 0)
}

fn chart_bucket_span_ms(range: ReportRange, resolution: ReportResolution) -> i64 {
    let base_ms = resolution.seconds() * 1_000;
    let needed = range.duration_ms().saturating_add(MAX_CHART_POINTS - 1) / MAX_CHART_POINTS;
    let multiples = needed.saturating_add(base_ms - 1) / base_ms;
    base_ms.saturating_mul(multiples.max(1))
}

fn query_series(
    connection: &Connection,
    range: ReportRange,
    resolution: ReportResolution,
    bucket_span_ms: i64,
) -> Result<Vec<ReportSeries>> {
    let mut grouped: BTreeMap<SeriesKey, Vec<ReportPoint>> = BTreeMap::new();
    match resolution {
        ReportResolution::Raw => {
            let sql = format!(
                "SELECT ((collected_at_ms - ?1) / ?3) * ?3 + ?1 AS report_bucket,
                        collector, COALESCE(resource, ''), metric_name, unit,
                        COUNT(*), MIN(value), MAX(value), SUM(value)
                 FROM metric_samples
                 WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
                   AND metric_name IN ({REPORT_METRICS})
                 GROUP BY report_bucket, collector, COALESCE(resource, ''), metric_name, unit
                 ORDER BY report_bucket"
            );
            let mut statement = connection.prepare(&sql)?;
            let mut rows = statement.query(params![range.from_ms, range.to_ms, bucket_span_ms])?;
            while let Some(row) = rows.next()? {
                push_point(&mut grouped, row)?;
            }
        }
        ReportResolution::Minute | ReportResolution::QuarterHour => {
            let sql = format!(
                "SELECT ((bucket_start_ms - ?2) / ?4) * ?4 + ?2 AS report_bucket,
                        collector, resource, metric_name, unit,
                        SUM(sample_count), MIN(min_value), MAX(max_value), SUM(sum_value)
                 FROM metric_rollups
                 WHERE resolution_seconds = ?1
                   AND bucket_start_ms >= ?2 AND bucket_start_ms < ?3
                   AND metric_name IN ({REPORT_METRICS})
                 GROUP BY report_bucket, collector, resource, metric_name, unit
                 ORDER BY report_bucket"
            );
            let mut statement = connection.prepare(&sql)?;
            let mut rows = statement.query(params![
                resolution.seconds(),
                range.from_ms,
                range.to_ms,
                bucket_span_ms
            ])?;
            while let Some(row) = rows.next()? {
                push_point(&mut grouped, row)?;
            }
        }
    }
    Ok(grouped
        .into_iter()
        .filter_map(|(key, points)| build_series(key, points))
        .collect())
}

fn push_point(
    grouped: &mut BTreeMap<SeriesKey, Vec<ReportPoint>>,
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<()> {
    let count: i64 = row.get(5)?;
    let sum: f64 = row.get(8)?;
    let key = SeriesKey {
        collector: row.get(1)?,
        resource: row.get(2)?,
        metric_name: row.get(3)?,
        unit: row.get(4)?,
    };
    grouped.entry(key).or_default().push(ReportPoint {
        timestamp_ms: row.get(0)?,
        sample_count: count,
        min_value: row.get(6)?,
        max_value: row.get(7)?,
        average_value: if count == 0 { 0.0 } else { sum / count as f64 },
    });
    Ok(())
}

fn build_series(key: SeriesKey, points: Vec<ReportPoint>) -> Option<ReportSeries> {
    let sample_count: i64 = points.iter().map(|point| point.sample_count).sum();
    if sample_count == 0 {
        return None;
    }
    let min_value = points
        .iter()
        .map(|point| point.min_value)
        .reduce(f64::min)?;
    let max_value = points
        .iter()
        .map(|point| point.max_value)
        .reduce(f64::max)?;
    let sum: f64 = points
        .iter()
        .map(|point| point.average_value * point.sample_count as f64)
        .sum();
    Some(ReportSeries {
        collector: key.collector,
        resource: key.resource,
        metric_name: key.metric_name,
        unit: key.unit,
        sample_count,
        min_value,
        max_value,
        average_value: sum / sample_count as f64,
        points,
    })
}

fn query_events(
    connection: &Connection,
    range: ReportRange,
) -> Result<(Vec<crate::storage::EventDetail>, bool)> {
    let mut statement = connection.prepare(
        "SELECT id FROM anomaly_events
         WHERE started_at_ms < ?2 AND COALESCE(ended_at_ms, ?2) >= ?1
         ORDER BY started_at_ms DESC LIMIT ?3",
    )?;
    let ids = statement
        .query_map(
            params![
                range.from_ms,
                range.to_ms,
                i64::try_from(EVENT_LIMIT + 1).unwrap_or(i64::MAX)
            ],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let truncated = ids.len() > EVENT_LIMIT;
    let mut events = Vec::with_capacity(ids.len().min(EVENT_LIMIT));
    for id in ids.into_iter().take(EVENT_LIMIT) {
        if let Some(event) = anomaly_storage::get_event(connection, id)? {
            events.push(event);
        }
    }
    Ok((events, truncated))
}

fn query_processes(
    connection: &Connection,
    range: ReportRange,
) -> Result<Vec<ReportProcessSummary>> {
    if let Some(resolution) = select_process_rollup_resolution(connection, range)? {
        return query_rollup_processes(connection, range, resolution);
    }
    let mut summaries = BTreeMap::new();
    for order_column in [
        "peak_cpu",
        "peak_memory",
        "peak_disk_read",
        "peak_disk_write",
        "peak_network_receive",
        "peak_network_transmit",
        "peak_gpu",
    ] {
        let sql = format!(
            "SELECT pid, process_start_time_seconds, MAX(name),
                    MAX(cpu_usage_percent) AS peak_cpu,
                    MAX(memory_bytes) AS peak_memory,
                    MAX(disk_read_bytes_per_second) AS peak_disk_read,
                    MAX(disk_write_bytes_per_second) AS peak_disk_write,
                    MAX(network_receive_bytes_per_second) AS peak_network_receive,
                    MAX(network_transmit_bytes_per_second) AS peak_network_transmit,
                    MAX(gpu_usage_percent) AS peak_gpu,
                    COUNT(*), MIN(collected_at_ms), MAX(collected_at_ms)
             FROM process_samples
             WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
             GROUP BY pid, process_start_time_seconds
             ORDER BY {order_column} DESC, pid
             LIMIT ?3"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                range.from_ms,
                range.to_ms,
                i64::try_from(PROCESS_LIMIT_PER_DIMENSION).unwrap_or(i64::MAX)
            ],
            |row| {
                let pid_value: i64 = row.get(0)?;
                let start_value: i64 = row.get(1)?;
                let memory_value: i64 = row.get(4)?;
                Ok(ReportProcessSummary {
                    pid: u32::try_from(pid_value).unwrap_or(0),
                    process_start_time_seconds: u64::try_from(start_value).unwrap_or(0),
                    name: row.get(2)?,
                    peak_cpu_percent: row.get(3)?,
                    peak_memory_bytes: u64::try_from(memory_value).unwrap_or(0),
                    peak_disk_read_bytes_per_second: row.get(5)?,
                    peak_disk_write_bytes_per_second: row.get(6)?,
                    peak_network_receive_bytes_per_second: row.get(7)?,
                    peak_network_transmit_bytes_per_second: row.get(8)?,
                    peak_gpu_usage_percent: row.get(9)?,
                    sample_count: row.get(10)?,
                    first_seen_ms: row.get(11)?,
                    last_seen_ms: row.get(12)?,
                })
            },
        )?;
        for summary in rows {
            let summary = summary?;
            summaries.insert((summary.pid, summary.process_start_time_seconds), summary);
        }
    }
    let selected: BTreeSet<_> = summaries.keys().copied().collect();
    let mut values: Vec<_> = selected
        .into_iter()
        .filter_map(|key| summaries.remove(&key))
        .collect();
    values.sort_by(|left, right| {
        right
            .peak_cpu_percent
            .total_cmp(&left.peak_cpu_percent)
            .then_with(|| right.peak_memory_bytes.cmp(&left.peak_memory_bytes))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    Ok(values)
}

fn select_process_rollup_resolution(
    connection: &Connection,
    range: ReportRange,
) -> Result<Option<i64>> {
    let raw_oldest: Option<i64> = connection.query_row(
        "SELECT MIN(collected_at_ms) FROM process_samples WHERE collected_at_ms < ?1",
        [range.to_ms],
        |row| row.get(0),
    )?;
    if raw_oldest.is_some_and(|oldest| oldest <= range.from_ms) {
        return Ok(None);
    }
    for resolution in [60_i64, 900] {
        let oldest: Option<i64> = connection.query_row(
            "SELECT MIN(bucket_start_ms) FROM process_metric_rollups
             WHERE resolution_seconds = ?1 AND bucket_start_ms < ?2",
            params![resolution, range.to_ms],
            |row| row.get(0),
        )?;
        if oldest.is_some_and(|oldest| oldest <= range.from_ms) {
            return Ok(Some(resolution));
        }
    }
    Ok(None)
}

fn query_rollup_processes(
    connection: &Connection,
    range: ReportRange,
    resolution: i64,
) -> Result<Vec<ReportProcessSummary>> {
    let mut summaries = BTreeMap::new();
    for order_column in [
        "peak_cpu",
        "peak_memory",
        "peak_disk_read",
        "peak_disk_write",
        "peak_network_receive",
        "peak_network_transmit",
        "peak_gpu",
    ] {
        let sql = format!(
            "SELECT pid, process_start_time_seconds, MAX(name),
                      MAX(CASE WHEN metric_name='process.cpu.usage' THEN max_value END),
                      MAX(CASE WHEN metric_name='process.memory.bytes' THEN max_value END),
                      MAX(CASE WHEN metric_name='process.disk.read.rate' THEN max_value END),
                      MAX(CASE WHEN metric_name='process.disk.write.rate' THEN max_value END),
                      MAX(CASE WHEN metric_name='process.network.receive.rate' THEN max_value END),
                      MAX(CASE WHEN metric_name='process.network.transmit.rate' THEN max_value END),
                      MAX(CASE WHEN metric_name='process.gpu.usage' THEN max_value END),
                      SUM(CASE WHEN metric_name='process.cpu.usage' THEN sample_count ELSE 0 END),
                      MIN(bucket_start_ms), MAX(bucket_start_ms),
                      MAX(CASE WHEN metric_name='process.cpu.usage' THEN max_value END) AS peak_cpu,
                      MAX(CASE WHEN metric_name='process.memory.bytes' THEN max_value END) AS peak_memory,
                      MAX(CASE WHEN metric_name='process.disk.read.rate' THEN max_value END) AS peak_disk_read,
                      MAX(CASE WHEN metric_name='process.disk.write.rate' THEN max_value END) AS peak_disk_write,
                      MAX(CASE WHEN metric_name='process.network.receive.rate' THEN max_value END) AS peak_network_receive,
                      MAX(CASE WHEN metric_name='process.network.transmit.rate' THEN max_value END) AS peak_network_transmit,
                      MAX(CASE WHEN metric_name='process.gpu.usage' THEN max_value END) AS peak_gpu
               FROM process_metric_rollups
               WHERE resolution_seconds=?1 AND bucket_start_ms>=?2 AND bucket_start_ms<?3
               GROUP BY pid, process_start_time_seconds
               ORDER BY {order_column} DESC, pid LIMIT ?4"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                resolution,
                range.from_ms,
                range.to_ms,
                i64::try_from(PROCESS_LIMIT_PER_DIMENSION).unwrap_or(i64::MAX)
            ],
            read_rollup_process_summary,
        )?;
        for summary in rows {
            let summary = summary?;
            summaries.insert((summary.pid, summary.process_start_time_seconds), summary);
        }
    }
    let mut values: Vec<_> = summaries.into_values().collect();
    values.sort_by(|left, right| {
        right
            .peak_cpu_percent
            .total_cmp(&left.peak_cpu_percent)
            .then_with(|| right.peak_memory_bytes.cmp(&left.peak_memory_bytes))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    Ok(values)
}

fn read_rollup_process_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReportProcessSummary> {
    let memory = row.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
    Ok(ReportProcessSummary {
        pid: u32::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
        process_start_time_seconds: u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
        name: row.get(2)?,
        peak_cpu_percent: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
        peak_memory_bytes: float_to_u64(memory).unwrap_or(0),
        peak_disk_read_bytes_per_second: row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
        peak_disk_write_bytes_per_second: row.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
        peak_network_receive_bytes_per_second: row.get(7)?,
        peak_network_transmit_bytes_per_second: row.get(8)?,
        peak_gpu_usage_percent: row.get(9)?,
        sample_count: row.get(10)?,
        first_seen_ms: row.get(11)?,
        last_seen_ms: row.get(12)?,
    })
}

fn query_process_series(
    connection: &Connection,
    range: ReportRange,
) -> Result<(i64, Option<u64>, Vec<DashboardProcessSeries>)> {
    let process_bucket_span_ms = process_chart_bucket_span_ms(range);
    if let Some(resolution) = select_process_rollup_resolution(connection, range)? {
        return query_rollup_process_series(connection, range, resolution, process_bucket_span_ms);
    }
    let mut selected: BTreeMap<(u32, u64), ProcessSeriesSelection> = BTreeMap::new();
    for (order_column, dimension) in [
        ("peak_cpu", ProcessDimension::Cpu),
        ("peak_memory", ProcessDimension::Memory),
        ("peak_disk_read", ProcessDimension::DiskRead),
        ("peak_disk_write", ProcessDimension::DiskWrite),
        ("peak_network_receive", ProcessDimension::NetworkReceive),
        ("peak_network_transmit", ProcessDimension::NetworkTransmit),
        ("peak_gpu", ProcessDimension::Gpu),
    ] {
        let sql = format!(
            "SELECT pid, process_start_time_seconds, MAX(name),
                    MAX(cpu_usage_percent) AS peak_cpu,
                    MAX(memory_bytes) AS peak_memory,
                    MAX(disk_read_bytes_per_second) AS peak_disk_read,
                    MAX(disk_write_bytes_per_second) AS peak_disk_write,
                    MAX(network_receive_bytes_per_second) AS peak_network_receive,
                    MAX(network_transmit_bytes_per_second) AS peak_network_transmit,
                    MAX(gpu_usage_percent) AS peak_gpu
             FROM process_samples
             WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
             GROUP BY pid, process_start_time_seconds
             ORDER BY {order_column} DESC, pid
             LIMIT ?3"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                range.from_ms,
                range.to_ms,
                i64::try_from(PROCESS_CHART_LIMIT_PER_DIMENSION).unwrap_or(i64::MAX)
            ],
            |row| {
                let pid: i64 = row.get(0)?;
                let start: i64 = row.get(1)?;
                Ok((
                    u32::try_from(pid).unwrap_or(0),
                    u64::try_from(start).unwrap_or(0),
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        for (index, row) in rows.enumerate() {
            let (pid, process_start_time_seconds, name) = row?;
            let selection = selected
                .entry((pid, process_start_time_seconds))
                .or_insert_with(|| ProcessSeriesSelection {
                    pid,
                    process_start_time_seconds,
                    name,
                    cpu_rank: None,
                    memory_rank: None,
                    disk_read_rank: None,
                    disk_write_rank: None,
                    network_receive_rank: None,
                    network_transmit_rank: None,
                    gpu_rank: None,
                });
            let rank = Some(u8::try_from(index + 1).unwrap_or(u8::MAX));
            match dimension {
                ProcessDimension::Cpu => selection.cpu_rank = rank,
                ProcessDimension::Memory => selection.memory_rank = rank,
                ProcessDimension::DiskRead => selection.disk_read_rank = rank,
                ProcessDimension::DiskWrite => selection.disk_write_rank = rank,
                ProcessDimension::NetworkReceive => selection.network_receive_rank = rank,
                ProcessDimension::NetworkTransmit => selection.network_transmit_rank = rank,
                ProcessDimension::Gpu => selection.gpu_rank = rank,
            }
        }
    }

    let mut process_series = Vec::with_capacity(selected.len());
    for selection in selected.into_values() {
        let mut statement = connection.prepare(
            "SELECT ((collected_at_ms - ?1) / ?5) * ?5 + ?1 AS chart_bucket,
                    AVG(cpu_usage_percent), MAX(cpu_usage_percent),
                    AVG(memory_bytes), MAX(memory_bytes),
                    AVG(disk_read_bytes_per_second), MAX(disk_read_bytes_per_second),
                    AVG(disk_write_bytes_per_second), MAX(disk_write_bytes_per_second),
                    AVG(network_receive_bytes_per_second), MAX(network_receive_bytes_per_second),
                    AVG(network_transmit_bytes_per_second), MAX(network_transmit_bytes_per_second),
                    AVG(gpu_usage_percent), MAX(gpu_usage_percent)
             FROM process_samples
             WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
               AND pid = ?3 AND process_start_time_seconds = ?4
             GROUP BY chart_bucket
             ORDER BY chart_bucket",
        )?;
        let rows = statement.query_map(
            params![
                range.from_ms,
                range.to_ms,
                i64::from(selection.pid),
                i64::try_from(selection.process_start_time_seconds).unwrap_or(i64::MAX),
                process_bucket_span_ms,
            ],
            |row| {
                let peak_memory: i64 = row.get(4)?;
                Ok(DashboardProcessPoint {
                    timestamp_ms: row.get(0)?,
                    average_cpu_percent: row.get(1)?,
                    peak_cpu_percent: row.get(2)?,
                    average_memory_bytes: row.get(3)?,
                    peak_memory_bytes: u64::try_from(peak_memory).unwrap_or(0),
                    average_disk_read_bytes_per_second: row.get(5)?,
                    peak_disk_read_bytes_per_second: row.get(6)?,
                    average_disk_write_bytes_per_second: row.get(7)?,
                    peak_disk_write_bytes_per_second: row.get(8)?,
                    average_network_receive_bytes_per_second: row.get(9)?,
                    peak_network_receive_bytes_per_second: row.get(10)?,
                    average_network_transmit_bytes_per_second: row.get(11)?,
                    peak_network_transmit_bytes_per_second: row.get(12)?,
                    average_gpu_usage_percent: row.get(13)?,
                    peak_gpu_usage_percent: row.get(14)?,
                })
            },
        )?;
        process_series.push(DashboardProcessSeries {
            pid: selection.pid,
            process_start_time_seconds: selection.process_start_time_seconds,
            name: selection.name,
            cpu_rank: selection.cpu_rank,
            memory_rank: selection.memory_rank,
            disk_read_rank: selection.disk_read_rank,
            disk_write_rank: selection.disk_write_rank,
            network_receive_rank: selection.network_receive_rank,
            network_transmit_rank: selection.network_transmit_rank,
            gpu_rank: selection.gpu_rank,
            points: rows.collect::<rusqlite::Result<Vec<_>>>()?,
        });
    }

    let memory_total = query_memory_total(connection, range)?;

    Ok((process_bucket_span_ms, memory_total, process_series))
}

fn query_memory_total(connection: &Connection, range: ReportRange) -> Result<Option<u64>> {
    let raw = connection
        .query_row(
            "SELECT value FROM metric_samples
             WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
               AND metric_name = 'memory.total'
             ORDER BY collected_at_ms DESC LIMIT 1",
            params![range.from_ms, range.to_ms],
            |row| row.get::<_, f64>(0),
        )
        .optional()?
        .and_then(float_to_u64);
    if raw.is_some() {
        return Ok(raw);
    }
    Ok(connection
        .query_row(
            "SELECT last_value FROM metric_rollups
         WHERE bucket_start_ms >= ?1 AND bucket_start_ms < ?2
           AND metric_name='memory.total'
         ORDER BY bucket_start_ms DESC LIMIT 1",
            params![range.from_ms, range.to_ms],
            |row| row.get::<_, f64>(0),
        )
        .optional()?
        .and_then(float_to_u64))
}

fn query_rollup_process_series(
    connection: &Connection,
    range: ReportRange,
    resolution: i64,
    bucket_span_ms: i64,
) -> Result<(i64, Option<u64>, Vec<DashboardProcessSeries>)> {
    let summaries = query_rollup_processes(connection, range, resolution)?;
    let mut selected: BTreeMap<_, (ReportProcessSummary, [Option<u8>; 7])> = BTreeMap::new();
    for dimension in [
        ProcessDimension::Cpu,
        ProcessDimension::Memory,
        ProcessDimension::DiskRead,
        ProcessDimension::DiskWrite,
        ProcessDimension::NetworkReceive,
        ProcessDimension::NetworkTransmit,
        ProcessDimension::Gpu,
    ] {
        let mut ranked: Vec<_> = summaries
            .iter()
            .filter(|summary| rollup_dimension_available(summary, dimension))
            .collect();
        ranked.sort_by(|left, right| {
            rollup_dimension_value(right, dimension)
                .total_cmp(&rollup_dimension_value(left, dimension))
                .then_with(|| left.pid.cmp(&right.pid))
        });
        for (index, process) in ranked
            .into_iter()
            .take(PROCESS_CHART_LIMIT_PER_DIMENSION)
            .enumerate()
        {
            let entry = selected
                .entry((process.pid, process.process_start_time_seconds))
                .or_insert_with(|| (process.clone(), [None; 7]));
            entry.1[process_dimension_index(dimension)] =
                Some(u8::try_from(index + 1).unwrap_or(u8::MAX));
        }
    }
    let mut series = Vec::new();
    for (process, ranks) in selected.into_values() {
        let mut statement = connection.prepare(
            "SELECT ((bucket_start_ms - ?1) / ?5) * ?5 + ?1 AS chart_bucket,
                    SUM(CASE WHEN metric_name='process.cpu.usage' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.cpu.usage' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.cpu.usage' THEN max_value END),
                    SUM(CASE WHEN metric_name='process.memory.bytes' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.memory.bytes' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.memory.bytes' THEN max_value END),
                    SUM(CASE WHEN metric_name='process.disk.read.rate' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.disk.read.rate' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.disk.read.rate' THEN max_value END),
                    SUM(CASE WHEN metric_name='process.disk.write.rate' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.disk.write.rate' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.disk.write.rate' THEN max_value END),
                    SUM(CASE WHEN metric_name='process.network.receive.rate' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.network.receive.rate' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.network.receive.rate' THEN max_value END),
                    SUM(CASE WHEN metric_name='process.network.transmit.rate' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.network.transmit.rate' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.network.transmit.rate' THEN max_value END),
                    SUM(CASE WHEN metric_name='process.gpu.usage' THEN sum_value END) / NULLIF(SUM(CASE WHEN metric_name='process.gpu.usage' THEN sample_count END),0),
                    MAX(CASE WHEN metric_name='process.gpu.usage' THEN max_value END)
             FROM process_metric_rollups
             WHERE resolution_seconds=?6 AND bucket_start_ms>=?1 AND bucket_start_ms<?2
               AND pid=?3 AND process_start_time_seconds=?4
             GROUP BY chart_bucket ORDER BY chart_bucket"
        )?;
        let points = statement
            .query_map(
                params![
                    range.from_ms,
                    range.to_ms,
                    i64::from(process.pid),
                    i64::try_from(process.process_start_time_seconds).unwrap_or(i64::MAX),
                    bucket_span_ms,
                    resolution
                ],
                |row| {
                    Ok(DashboardProcessPoint {
                        timestamp_ms: row.get(0)?,
                        average_cpu_percent: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                        peak_cpu_percent: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                        average_memory_bytes: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        peak_memory_bytes: float_to_u64(
                            row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                        )
                        .unwrap_or(0),
                        average_disk_read_bytes_per_second: row
                            .get::<_, Option<f64>>(5)?
                            .unwrap_or(0.0),
                        peak_disk_read_bytes_per_second: row
                            .get::<_, Option<f64>>(6)?
                            .unwrap_or(0.0),
                        average_disk_write_bytes_per_second: row
                            .get::<_, Option<f64>>(7)?
                            .unwrap_or(0.0),
                        peak_disk_write_bytes_per_second: row
                            .get::<_, Option<f64>>(8)?
                            .unwrap_or(0.0),
                        average_network_receive_bytes_per_second: row.get(9)?,
                        peak_network_receive_bytes_per_second: row.get(10)?,
                        average_network_transmit_bytes_per_second: row.get(11)?,
                        peak_network_transmit_bytes_per_second: row.get(12)?,
                        average_gpu_usage_percent: row.get(13)?,
                        peak_gpu_usage_percent: row.get(14)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        series.push(DashboardProcessSeries {
            pid: process.pid,
            process_start_time_seconds: process.process_start_time_seconds,
            name: process.name,
            cpu_rank: ranks[0],
            memory_rank: ranks[1],
            disk_read_rank: ranks[2],
            disk_write_rank: ranks[3],
            network_receive_rank: ranks[4],
            network_transmit_rank: ranks[5],
            gpu_rank: ranks[6],
            points,
        });
    }
    let memory_total = query_memory_total(connection, range)?;

    Ok((bucket_span_ms, memory_total, series))
}

fn process_dimension_index(dimension: ProcessDimension) -> usize {
    match dimension {
        ProcessDimension::Cpu => 0,
        ProcessDimension::Memory => 1,
        ProcessDimension::DiskRead => 2,
        ProcessDimension::DiskWrite => 3,
        ProcessDimension::NetworkReceive => 4,
        ProcessDimension::NetworkTransmit => 5,
        ProcessDimension::Gpu => 6,
    }
}

fn rollup_dimension_available(process: &ReportProcessSummary, dimension: ProcessDimension) -> bool {
    match dimension {
        ProcessDimension::NetworkReceive => process.peak_network_receive_bytes_per_second.is_some(),
        ProcessDimension::NetworkTransmit => {
            process.peak_network_transmit_bytes_per_second.is_some()
        }
        ProcessDimension::Gpu => process.peak_gpu_usage_percent.is_some(),
        _ => true,
    }
}

fn rollup_dimension_value(process: &ReportProcessSummary, dimension: ProcessDimension) -> f64 {
    match dimension {
        ProcessDimension::Cpu => process.peak_cpu_percent,
        ProcessDimension::Memory => process.peak_memory_bytes as f64,
        ProcessDimension::DiskRead => process.peak_disk_read_bytes_per_second,
        ProcessDimension::DiskWrite => process.peak_disk_write_bytes_per_second,
        ProcessDimension::NetworkReceive => {
            process.peak_network_receive_bytes_per_second.unwrap_or(0.0)
        }
        ProcessDimension::NetworkTransmit => process
            .peak_network_transmit_bytes_per_second
            .unwrap_or(0.0),
        ProcessDimension::Gpu => process.peak_gpu_usage_percent.unwrap_or(0.0),
    }
}

fn process_chart_bucket_span_ms(range: ReportRange) -> i64 {
    let needed = range
        .duration_ms()
        .saturating_add(MAX_PROCESS_CHART_POINTS - 1)
        / MAX_PROCESS_CHART_POINTS;
    let rounded = needed.saturating_add(999) / 1_000 * 1_000;
    rounded.max(MIN_PROCESS_BUCKET_MS)
}

fn float_to_u64(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0 && value <= u64::MAX as f64).then(|| value.round() as u64)
}

fn process_coverage(
    connection: &Connection,
    range: ReportRange,
) -> Result<(Option<i64>, Option<i64>)> {
    Ok(connection.query_row(
        "SELECT MIN(timestamp_ms), MAX(timestamp_ms) FROM (
           SELECT collected_at_ms AS timestamp_ms FROM process_samples
           WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2
           UNION ALL
           SELECT bucket_start_ms FROM process_metric_rollups
           WHERE bucket_start_ms >= ?1 AND bucket_start_ms < ?2
         )",
        params![range.from_ms, range.to_ms],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_buckets_stay_bounded() {
        let range = ReportRange::new(0, 365 * 24 * 60 * 60 * 1_000).expect("range");
        let bucket = chart_bucket_span_ms(range, ReportResolution::QuarterHour);

        assert!(range.duration_ms() / bucket <= MAX_CHART_POINTS);
        assert_eq!(bucket % (15 * 60 * 1_000), 0);
    }

    #[test]
    fn process_chart_buckets_stay_bounded() {
        let range = ReportRange::new(0, 24 * 60 * 60 * 1_000).expect("range");
        let bucket = process_chart_bucket_span_ms(range);

        assert!(range.duration_ms() / bucket <= MAX_PROCESS_CHART_POINTS);
        assert!(bucket >= MIN_PROCESS_BUCKET_MS);
    }
}
