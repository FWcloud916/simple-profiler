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
    let mut summaries = BTreeMap::new();
    for order_column in ["peak_cpu", "peak_memory"] {
        let sql = format!(
            "SELECT pid, process_start_time_seconds, MAX(name),
                    MAX(cpu_usage_percent) AS peak_cpu,
                    MAX(memory_bytes) AS peak_memory,
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
                    sample_count: row.get(5)?,
                    first_seen_ms: row.get(6)?,
                    last_seen_ms: row.get(7)?,
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

fn query_process_series(
    connection: &Connection,
    range: ReportRange,
) -> Result<(i64, Option<u64>, Vec<DashboardProcessSeries>)> {
    let process_bucket_span_ms = process_chart_bucket_span_ms(range);
    let mut selected: BTreeMap<(u32, u64), ProcessSeriesSelection> = BTreeMap::new();
    for (order_column, cpu_dimension) in [("peak_cpu", true), ("peak_memory", false)] {
        let sql = format!(
            "SELECT pid, process_start_time_seconds, MAX(name),
                    MAX(cpu_usage_percent) AS peak_cpu,
                    MAX(memory_bytes) AS peak_memory
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
                });
            let rank = Some(u8::try_from(index + 1).unwrap_or(u8::MAX));
            if cpu_dimension {
                selection.cpu_rank = rank;
            } else {
                selection.memory_rank = rank;
            }
        }
    }

    let mut process_series = Vec::with_capacity(selected.len());
    for selection in selected.into_values() {
        let mut statement = connection.prepare(
            "SELECT ((collected_at_ms - ?1) / ?5) * ?5 + ?1 AS chart_bucket,
                    AVG(cpu_usage_percent), MAX(cpu_usage_percent),
                    AVG(memory_bytes), MAX(memory_bytes)
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
                })
            },
        )?;
        process_series.push(DashboardProcessSeries {
            pid: selection.pid,
            process_start_time_seconds: selection.process_start_time_seconds,
            name: selection.name,
            cpu_rank: selection.cpu_rank,
            memory_rank: selection.memory_rank,
            points: rows.collect::<rusqlite::Result<Vec<_>>>()?,
        });
    }

    let memory_total = connection
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

    Ok((process_bucket_span_ms, memory_total, process_series))
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
        "SELECT MIN(collected_at_ms), MAX(collected_at_ms)
         FROM process_samples WHERE collected_at_ms >= ?1 AND collected_at_ms < ?2",
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
