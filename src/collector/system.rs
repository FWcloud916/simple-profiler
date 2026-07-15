use async_trait::async_trait;
use chrono::Utc;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use super::{Collector, CollectorError};
use crate::model::{Metric, MetricBatch};

pub struct SystemCollector {
    system: System,
}

impl SystemCollector {
    pub fn new() -> Self {
        let refresh_kind = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything());
        Self {
            system: System::new_with_specifics(refresh_kind),
        }
    }
}

impl Default for SystemCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Collector for SystemCollector {
    fn name(&self) -> &'static str {
        "system"
    }

    async fn collect(&mut self) -> Result<MetricBatch, CollectorError> {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        let collected_at = Utc::now();
        let mut metrics = Vec::with_capacity(self.system.cpus().len() + 5);
        metrics.push(Metric::new(
            collected_at,
            self.name(),
            "cpu.total.usage",
            f64::from(self.system.global_cpu_usage()),
            "percent",
        ));

        for (index, cpu) in self.system.cpus().iter().enumerate() {
            metrics.push(Metric::new(
                collected_at,
                self.name(),
                format!("cpu.core.{index}.usage"),
                f64::from(cpu.cpu_usage()),
                "percent",
            ));
        }

        let total_memory = self.system.total_memory();
        let used_memory = self.system.used_memory();
        let available_memory = self.system.available_memory();
        let usage_percent = if total_memory == 0 {
            0.0
        } else {
            used_memory as f64 / total_memory as f64 * 100.0
        };

        metrics.extend([
            Metric::new(
                collected_at,
                self.name(),
                "memory.total",
                total_memory as f64,
                "bytes",
            ),
            Metric::new(
                collected_at,
                self.name(),
                "memory.used",
                used_memory as f64,
                "bytes",
            ),
            Metric::new(
                collected_at,
                self.name(),
                "memory.available",
                available_memory as f64,
                "bytes",
            ),
            Metric::new(
                collected_at,
                self.name(),
                "memory.usage",
                usage_percent,
                "percent",
            ),
        ]);

        if metrics.is_empty() {
            return Err(CollectorError::EmptySample);
        }
        Ok(metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collects_cpu_and_memory_metrics() {
        let mut collector = SystemCollector::new();
        let metrics = collector.collect().await.expect("system metrics");

        assert!(
            metrics
                .iter()
                .any(|metric| metric.name == "cpu.total.usage")
        );
        assert!(metrics.iter().any(|metric| metric.name == "memory.total"));
        assert!(metrics.iter().all(|metric| metric.value >= 0.0));
    }
}
