mod disk;
mod network;
mod system;

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::model::MetricBatch;

pub use disk::DiskCollector;
pub use network::NetworkCollector;
pub use system::SystemCollector;

#[derive(Debug, Clone, Copy)]
pub struct CollectionContext {
    pub collected_at: DateTime<Utc>,
    pub elapsed: Option<Duration>,
}

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error("collector returned no metrics")]
    EmptySample,

    #[error("capability unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait Collector: Send {
    fn name(&self) -> &'static str;

    async fn collect(&mut self, context: &CollectionContext)
    -> Result<MetricBatch, CollectorError>;
}
