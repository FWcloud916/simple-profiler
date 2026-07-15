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
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub disk_read_bytes_per_second: f64,
    pub disk_write_bytes_per_second: f64,
    pub network_receive_bytes: Option<u64>,
    pub network_transmit_bytes: Option<u64>,
    pub network_receive_bytes_per_second: Option<f64>,
    pub network_transmit_bytes_per_second: Option<f64>,
    pub cpu_rank: Option<u32>,
    pub memory_rank: Option<u32>,
    pub disk_read_rank: Option<u32>,
    pub disk_write_rank: Option<u32>,
    pub network_receive_rank: Option<u32>,
    pub network_transmit_rank: Option<u32>,
}

pub type ProcessSnapshot = Vec<ProcessSample>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Degraded,
    Unavailable,
}

impl CapabilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "degraded" => Some(Self::Degraded),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectorCapability {
    pub collector: String,
    pub resource: String,
    pub capability: String,
    pub state: CapabilityState,
    pub provider: String,
    pub detail: Option<String>,
    pub checked_at_ms: i64,
}

pub type CapabilityBatch = Vec<CollectorCapability>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CollectionBatch {
    pub metrics: MetricBatch,
    pub processes: ProcessSnapshot,
    pub capabilities: CapabilityBatch,
}

impl CollectionBatch {
    pub fn metrics_only(metrics: MetricBatch) -> Self {
        Self {
            metrics,
            processes: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.metrics.is_empty() && self.processes.is_empty() && self.capabilities.is_empty()
    }
}
