use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub collected_at: DateTime<Utc>,
    pub collector: String,
    pub resource: Option<String>,
    pub name: String,
    pub value: f64,
    pub unit: String,
}

impl Metric {
    pub fn new(
        collected_at: DateTime<Utc>,
        collector: impl Into<String>,
        name: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
    ) -> Self {
        Self {
            collected_at,
            collector: collector.into(),
            resource: None,
            name: name.into(),
            value,
            unit: unit.into(),
        }
    }

    pub fn for_resource(
        collected_at: DateTime<Utc>,
        collector: impl Into<String>,
        resource: impl Into<String>,
        name: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
    ) -> Self {
        Self {
            collected_at,
            collector: collector.into(),
            resource: Some(resource.into()),
            name: name.into(),
            value,
            unit: unit.into(),
        }
    }
}

pub type MetricBatch = Vec<Metric>;
