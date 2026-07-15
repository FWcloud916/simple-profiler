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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    pub collected_at: DateTime<Utc>,
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

pub type ProcessSnapshot = Vec<ProcessSample>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CollectionBatch {
    pub metrics: MetricBatch,
    pub processes: ProcessSnapshot,
}

impl CollectionBatch {
    pub fn metrics_only(metrics: MetricBatch) -> Self {
        Self {
            metrics,
            processes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.metrics.is_empty() && self.processes.is_empty()
    }
}
