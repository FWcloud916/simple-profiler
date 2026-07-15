use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::{
    anomaly::{AnomalyEngine, Evaluation, Phase, RuleState, Severity, StateKey, Transition},
    config::{AnomalyConfig, ProcessConfig},
    model::{CollectionBatch, MetricBatch},
    process_storage::{self, ProcessEventEvidence},
};

const PRELUDE_LIMIT: i64 = 120;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventSummary {
    pub id: i64,
    pub rule_id: String,
    pub metric_name: String,
    pub resource: String,
    pub status: String,
    pub severity: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub peak_value: f64,
    pub peak_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventEvidence {
    pub collected_at_ms: i64,
    pub value: f64,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventDetail {
    pub summary: EventSummary,
    pub collector: String,
    pub unit: String,
    pub detected_at_ms: i64,
    pub warning_threshold: f64,
    pub critical_threshold: f64,
    pub recovery_threshold: f64,
    pub last_value: f64,
    pub last_sample_ms: i64,
    pub sample_count: i64,
    pub data_gap_count: i64,
    pub evidence: Vec<EventEvidence>,
    pub process_evidence: Vec<ProcessEventEvidence>,
}

pub(crate) fn load_engine(
    connection: &Connection,
    config: &AnomalyConfig,
) -> Result<AnomalyEngine> {
    let mut engine = AnomalyEngine::new(config);
    let mut statement = connection.prepare(
        "SELECT rule_id, resource, phase, severity, pending_severity,
                pending_since_ms, pending_samples, critical_since_ms, critical_samples,
                recovery_since_ms, recovery_samples, event_id, last_sample_ms, last_value,
                peak_value, peak_at_ms, last_evidence_ms, data_gap_count
         FROM anomaly_states",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            StateKey {
                rule_id: row.get(0)?,
                resource: row.get(1)?,
            },
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            RuleState {
                phase: Phase::Normal,
                severity: None,
                pending_severity: None,
                pending_since_ms: row.get(5)?,
                pending_samples: integer_to_u64(row.get(6)?),
                critical_since_ms: row.get(7)?,
                critical_samples: integer_to_u64(row.get(8)?),
                recovery_since_ms: row.get(9)?,
                recovery_samples: integer_to_u64(row.get(10)?),
                event_id: row.get(11)?,
                last_sample_ms: row.get(12)?,
                last_value: row.get(13)?,
                peak_value: row.get(14)?,
                peak_at_ms: row.get(15)?,
                last_evidence_ms: row.get(16)?,
                data_gap_count: integer_to_u64(row.get(17)?),
            },
        ))
    })?;
    for row in rows {
        let (key, phase, severity, pending_severity, mut state) = row?;
        state.phase = Phase::parse(&phase)
            .with_context(|| format!("invalid anomaly phase {phase:?} for {}", key.rule_id))?;
        state.severity = parse_optional_severity(severity.as_deref(), &key)?;
        state.pending_severity = parse_optional_severity(pending_severity.as_deref(), &key)?;
        engine.restore(key, state);
    }
    Ok(engine)
}

pub(crate) fn insert_batch(
    connection: &mut Connection,
    batch: &CollectionBatch,
    engine: &mut AnomalyEngine,
    anomaly_config: &AnomalyConfig,
    process_config: &ProcessConfig,
) -> Result<()> {
    let mut next_engine = engine.clone();
    let evaluations = next_engine.evaluate_batch(&batch.metrics);
    let transaction = connection.transaction()?;
    insert_raw_metrics(&transaction, &batch.metrics)?;
    process_storage::insert_samples(&transaction, &batch.processes)?;
    for evaluation in &evaluations {
        apply_evaluation(
            &transaction,
            evaluation,
            &mut next_engine,
            anomaly_config,
            process_config,
        )?;
    }
    persist_states(&transaction, &next_engine)?;
    transaction.commit()?;
    *engine = next_engine;
    Ok(())
}

fn insert_raw_metrics(transaction: &Transaction<'_>, batch: &MetricBatch) -> Result<()> {
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
    Ok(())
}

fn apply_evaluation(
    transaction: &Transaction<'_>,
    evaluation: &Evaluation,
    engine: &mut AnomalyEngine,
    anomaly_config: &AnomalyConfig,
    process_config: &ProcessConfig,
) -> Result<()> {
    let timestamp_ms = evaluation.metric.collected_at.timestamp_millis();
    if evaluation.transition == Transition::Open {
        let severity = evaluation
            .state
            .severity
            .context("opened anomaly is missing severity")?;
        let started_at_ms = evaluation
            .event_started_at_ms
            .context("opened anomaly is missing its start time")?;
        transaction.execute(
            "INSERT INTO anomaly_events
             (rule_id, collector, metric_name, resource, unit, status, severity,
              started_at_ms, detected_at_ms, warning_threshold, critical_threshold,
              recovery_threshold, peak_value, peak_at_ms, last_value, last_sample_ms,
              sample_count, data_gap_count)
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                evaluation.rule.id,
                evaluation.metric.collector,
                evaluation.metric.name,
                evaluation.key.resource,
                evaluation.metric.unit,
                severity.as_str(),
                started_at_ms,
                timestamp_ms,
                evaluation.rule.warning_threshold,
                evaluation.rule.critical_threshold,
                evaluation.rule.recovery_threshold,
                evaluation
                    .state
                    .peak_value
                    .unwrap_or(evaluation.metric.value),
                evaluation.state.peak_at_ms.unwrap_or(timestamp_ms),
                evaluation.metric.value,
                timestamp_ms,
                u64_to_integer(evaluation.trigger_sample_count),
                u64_to_integer(evaluation.state.data_gap_count),
            ],
        )?;
        let event_id = transaction.last_insert_rowid();
        insert_prelude(transaction, event_id, evaluation, anomaly_config)?;
        insert_evidence(
            transaction,
            event_id,
            timestamp_ms,
            evaluation.metric.value,
            "trigger",
        )?;
        process_storage::capture_open_event(
            transaction,
            event_id,
            &evaluation.metric.name,
            timestamp_ms,
            anomaly_config.prelude_minutes,
            process_config,
        )?;
        engine.attach_event(&evaluation.key, event_id, timestamp_ms);
        return Ok(());
    }

    let Some(event_id) = evaluation.event_id else {
        return Ok(());
    };
    let severity = evaluation
        .state
        .severity
        .context("active anomaly is missing severity")?;
    transaction.execute(
        "UPDATE anomaly_events SET
           severity = ?2,
           last_value = ?3,
           last_sample_ms = ?4,
           sample_count = sample_count + 1,
           data_gap_count = data_gap_count + ?5,
           peak_value = MAX(peak_value, ?3),
           peak_at_ms = CASE WHEN ?3 > peak_value THEN ?4 ELSE peak_at_ms END
         WHERE id = ?1 AND status = 'open'",
        params![
            event_id,
            severity.as_str(),
            evaluation.metric.value,
            timestamp_ms,
            i64::from(evaluation.data_gap),
        ],
    )?;

    let evidence_kind = match evaluation.transition {
        Transition::Escalate => Some("escalation"),
        Transition::Close => Some("recovery"),
        Transition::None if evaluation.new_peak => Some("peak"),
        Transition::None if evaluation.periodic_evidence => Some("periodic"),
        Transition::None | Transition::Open => None,
    };
    if let Some(kind) = evidence_kind {
        if kind == "peak" {
            transaction.execute(
                "DELETE FROM anomaly_event_evidence WHERE event_id = ?1 AND kind = 'peak'",
                [event_id],
            )?;
        }
        insert_evidence(
            transaction,
            event_id,
            timestamp_ms,
            evaluation.metric.value,
            kind,
        )?;
        if kind != "peak" {
            process_storage::capture_checkpoint(
                transaction,
                event_id,
                &evaluation.metric.name,
                timestamp_ms,
                kind,
                process_config,
            )?;
        }
    }
    if evaluation.transition == Transition::Close {
        transaction.execute(
            "UPDATE anomaly_events SET status = 'closed', ended_at_ms = ?2 WHERE id = ?1",
            params![event_id, timestamp_ms],
        )?;
    }
    Ok(())
}

fn insert_prelude(
    transaction: &Transaction<'_>,
    event_id: i64,
    evaluation: &Evaluation,
    config: &AnomalyConfig,
) -> Result<()> {
    let detected_at_ms = evaluation.metric.collected_at.timestamp_millis();
    let prelude_ms = u64_to_integer(config.prelude_minutes)
        .saturating_mul(60)
        .saturating_mul(1_000);
    let mut statement = transaction.prepare(
        "SELECT collected_at_ms, value FROM metric_samples
         WHERE metric_name = ?1 AND COALESCE(resource, '') = ?2
           AND collected_at_ms >= ?3 AND collected_at_ms < ?4
         ORDER BY collected_at_ms DESC, id DESC LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![
            evaluation.metric.name,
            evaluation.key.resource,
            detected_at_ms.saturating_sub(prelude_ms),
            detected_at_ms,
            PRELUDE_LIMIT,
        ],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)),
    )?;
    let mut evidence = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    evidence.reverse();
    drop(statement);
    for (timestamp_ms, value) in evidence {
        insert_evidence(transaction, event_id, timestamp_ms, value, "prelude")?;
    }
    Ok(())
}

fn insert_evidence(
    transaction: &Transaction<'_>,
    event_id: i64,
    timestamp_ms: i64,
    value: f64,
    kind: &str,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO anomaly_event_evidence(event_id, collected_at_ms, value, kind)
         VALUES (?1, ?2, ?3, ?4)",
        params![event_id, timestamp_ms, value, kind],
    )?;
    Ok(())
}

fn persist_states(transaction: &Transaction<'_>, engine: &AnomalyEngine) -> Result<()> {
    let mut statement = transaction.prepare_cached(
        "INSERT INTO anomaly_states
         (rule_id, resource, phase, severity, pending_severity, pending_since_ms,
          pending_samples, critical_since_ms, critical_samples, recovery_since_ms,
          recovery_samples, event_id, last_sample_ms, last_value, peak_value, peak_at_ms,
          last_evidence_ms, data_gap_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18)
         ON CONFLICT(rule_id, resource) DO UPDATE SET
           phase = excluded.phase, severity = excluded.severity,
           pending_severity = excluded.pending_severity,
           pending_since_ms = excluded.pending_since_ms,
           pending_samples = excluded.pending_samples,
           critical_since_ms = excluded.critical_since_ms,
           critical_samples = excluded.critical_samples,
           recovery_since_ms = excluded.recovery_since_ms,
           recovery_samples = excluded.recovery_samples,
           event_id = excluded.event_id, last_sample_ms = excluded.last_sample_ms,
           last_value = excluded.last_value, peak_value = excluded.peak_value,
           peak_at_ms = excluded.peak_at_ms, last_evidence_ms = excluded.last_evidence_ms,
           data_gap_count = excluded.data_gap_count",
    )?;
    for (key, state) in engine.states() {
        statement.execute(params![
            key.rule_id,
            key.resource,
            state.phase.as_str(),
            state.severity.map(Severity::as_str),
            state.pending_severity.map(Severity::as_str),
            state.pending_since_ms,
            u64_to_integer(state.pending_samples),
            state.critical_since_ms,
            u64_to_integer(state.critical_samples),
            state.recovery_since_ms,
            u64_to_integer(state.recovery_samples),
            state.event_id,
            state.last_sample_ms,
            state.last_value,
            state.peak_value,
            state.peak_at_ms,
            state.last_evidence_ms,
            u64_to_integer(state.data_gap_count),
        ])?;
    }
    Ok(())
}

pub(crate) fn delete_closed_events(
    transaction: &Transaction<'_>,
    config: &AnomalyConfig,
    now_ms: i64,
) -> Result<usize> {
    let cutoff = now_ms.saturating_sub(
        u64_to_integer(config.event_retention_days)
            .saturating_mul(24)
            .saturating_mul(60)
            .saturating_mul(60)
            .saturating_mul(1_000),
    );
    let mut statement = transaction.prepare(
        "SELECT id FROM anomaly_events
         WHERE status = 'closed' AND ended_at_ms < ?1
         ORDER BY ended_at_ms LIMIT ?2",
    )?;
    let ids = statement
        .query_map(
            params![cutoff, u64_to_integer(config.delete_batch_rows as u64)],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for id in &ids {
        transaction.execute(
            "DELETE FROM anomaly_event_evidence WHERE event_id = ?1",
            [id],
        )?;
        transaction.execute(
            "DELETE FROM anomaly_event_process_evidence WHERE event_id = ?1",
            [id],
        )?;
        transaction.execute("DELETE FROM anomaly_events WHERE id = ?1", [id])?;
    }
    Ok(ids.len())
}

pub(crate) fn list_events(
    connection: &Connection,
    open_only: bool,
    limit: usize,
) -> Result<Vec<EventSummary>> {
    let mut statement = connection.prepare(
        "SELECT id, rule_id, metric_name, resource, status, severity, started_at_ms,
                ended_at_ms, peak_value, peak_at_ms
         FROM anomaly_events
         WHERE (?1 = 0 OR status = 'open')
         ORDER BY started_at_ms DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![i64::from(open_only), u64_to_integer(limit as u64)],
        read_summary,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn list_events_in_range(
    connection: &Connection,
    from_ms: i64,
    to_ms: i64,
    limit: usize,
) -> Result<(Vec<EventSummary>, bool)> {
    let mut statement = connection.prepare(
        "SELECT id, rule_id, metric_name, resource, status, severity, started_at_ms,
                ended_at_ms, peak_value, peak_at_ms
         FROM anomaly_events
         WHERE started_at_ms < ?2 AND COALESCE(ended_at_ms, ?2) >= ?1
         ORDER BY started_at_ms DESC LIMIT ?3",
    )?;
    let mut events = statement
        .query_map(
            params![from_ms, to_ms, u64_to_integer((limit + 1) as u64)],
            read_summary,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = events.len() > limit;
    events.truncate(limit);
    Ok((events, truncated))
}

pub(crate) fn get_event(connection: &Connection, id: i64) -> Result<Option<EventDetail>> {
    let row = connection
        .query_row(
            "SELECT id, rule_id, metric_name, resource, status, severity, started_at_ms,
                    ended_at_ms, peak_value, peak_at_ms, collector, unit, detected_at_ms,
                    warning_threshold, critical_threshold, recovery_threshold, last_value,
                    last_sample_ms, sample_count, data_gap_count
             FROM anomaly_events WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    EventSummary {
                        id: row.get(0)?,
                        rule_id: row.get(1)?,
                        metric_name: row.get(2)?,
                        resource: row.get(3)?,
                        status: row.get(4)?,
                        severity: row.get(5)?,
                        started_at_ms: row.get(6)?,
                        ended_at_ms: row.get(7)?,
                        peak_value: row.get(8)?,
                        peak_at_ms: row.get(9)?,
                    },
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, f64>(13)?,
                    row.get::<_, f64>(14)?,
                    row.get::<_, f64>(15)?,
                    row.get::<_, f64>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, i64>(18)?,
                    row.get::<_, i64>(19)?,
                ))
            },
        )
        .optional()?;
    let Some((
        summary,
        collector,
        unit,
        detected_at_ms,
        warning_threshold,
        critical_threshold,
        recovery_threshold,
        last_value,
        last_sample_ms,
        sample_count,
        data_gap_count,
    )) = row
    else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT collected_at_ms, value, kind FROM anomaly_event_evidence
         WHERE event_id = ?1 ORDER BY collected_at_ms, id",
    )?;
    let evidence = statement
        .query_map([id], |row| {
            Ok(EventEvidence {
                collected_at_ms: row.get(0)?,
                value: row.get(1)?,
                kind: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let process_evidence = process_storage::event_evidence(connection, id)?;
    Ok(Some(EventDetail {
        summary,
        collector,
        unit,
        detected_at_ms,
        warning_threshold,
        critical_threshold,
        recovery_threshold,
        last_value,
        last_sample_ms,
        sample_count,
        data_gap_count,
        evidence,
        process_evidence,
    }))
}

fn read_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventSummary> {
    Ok(EventSummary {
        id: row.get(0)?,
        rule_id: row.get(1)?,
        metric_name: row.get(2)?,
        resource: row.get(3)?,
        status: row.get(4)?,
        severity: row.get(5)?,
        started_at_ms: row.get(6)?,
        ended_at_ms: row.get(7)?,
        peak_value: row.get(8)?,
        peak_at_ms: row.get(9)?,
    })
}

fn parse_optional_severity(value: Option<&str>, key: &StateKey) -> Result<Option<Severity>> {
    value
        .map(|value| {
            Severity::parse(value)
                .with_context(|| format!("invalid anomaly severity {value:?} for {}", key.rule_id))
        })
        .transpose()
}

fn integer_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn u64_to_integer(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
