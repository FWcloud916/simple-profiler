use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, params};

use crate::model::{CapabilityBatch, CapabilityState, CollectorCapability};

pub(crate) fn upsert(transaction: &Transaction<'_>, capabilities: &CapabilityBatch) -> Result<()> {
    let mut statement = transaction.prepare_cached(
        "INSERT INTO collector_capabilities
         (collector, resource, capability, state, provider, detail, checked_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(collector, resource, capability) DO UPDATE SET
             state = excluded.state,
             provider = excluded.provider,
             detail = excluded.detail,
             checked_at_ms = excluded.checked_at_ms",
    )?;
    for capability in capabilities {
        statement.execute(params![
            capability.collector,
            capability.resource,
            capability.capability,
            capability.state.as_str(),
            capability.provider,
            capability.detail,
            capability.checked_at_ms,
        ])?;
    }
    Ok(())
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<CollectorCapability>> {
    let mut statement = connection.prepare(
        "SELECT collector, resource, capability, state, provider, detail, checked_at_ms
         FROM collector_capabilities
         ORDER BY collector, resource, capability",
    )?;
    let rows = statement.query_map([], |row| {
        let state: String = row.get(3)?;
        let state = CapabilityState::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                format!("invalid capability state {state:?}").into(),
            )
        })?;
        Ok(CollectorCapability {
            collector: row.get(0)?,
            resource: row.get(1)?,
            capability: row.get(2)?,
            state,
            provider: row.get(4)?,
            detail: row.get(5)?,
            checked_at_ms: row.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to read collector capabilities")
}
