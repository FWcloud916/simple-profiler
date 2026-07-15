mod system;

use async_trait::async_trait;
use thiserror::Error;

use crate::model::MetricBatch;

pub use system::SystemCollector;

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error("collector returned no metrics")]
    EmptySample,
}

#[async_trait]
pub trait Collector: Send {
    fn name(&self) -> &'static str;

    async fn collect(&mut self) -> Result<MetricBatch, CollectorError>;
}
