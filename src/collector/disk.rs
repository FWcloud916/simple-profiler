use async_trait::async_trait;
use sysinfo::Disks;

use super::{CollectionContext, Collector, CollectorError};
use crate::model::{Metric, MetricBatch};

pub struct DiskCollector {
    disks: Disks,
}

impl DiskCollector {
    pub fn new() -> Self {
        Self {
            disks: Disks::new_with_refreshed_list(),
        }
    }
}

impl Default for DiskCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Collector for DiskCollector {
    fn name(&self) -> &'static str {
        "disk"
    }

    async fn collect(
        &mut self,
        context: &CollectionContext,
    ) -> Result<MetricBatch, CollectorError> {
        self.disks.refresh(true);
        if self.disks.is_empty() {
            return Err(CollectorError::Unavailable(
                "no disks were reported by the operating system".to_owned(),
            ));
        }

        let metrics_per_disk = if context.elapsed.is_some() { 8 } else { 4 };
        let mut metrics = Vec::with_capacity(self.disks.len() * metrics_per_disk);
        for disk in self.disks.list() {
            let resource = disk.mount_point().to_string_lossy().into_owned();
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let usage_percent = percentage(used, total);

            metrics.extend([
                resource_metric(
                    context,
                    &resource,
                    "disk.space.total",
                    total as f64,
                    "bytes",
                ),
                resource_metric(
                    context,
                    &resource,
                    "disk.space.available",
                    available as f64,
                    "bytes",
                ),
                resource_metric(context, &resource, "disk.space.used", used as f64, "bytes"),
                resource_metric(
                    context,
                    &resource,
                    "disk.space.usage",
                    usage_percent,
                    "percent",
                ),
            ]);

            if let Some(elapsed) = context.elapsed.filter(|elapsed| !elapsed.is_zero()) {
                let usage = disk.usage();
                metrics.extend([
                    resource_metric(
                        context,
                        &resource,
                        "disk.io.read.delta",
                        usage.read_bytes as f64,
                        "bytes",
                    ),
                    resource_metric(
                        context,
                        &resource,
                        "disk.io.write.delta",
                        usage.written_bytes as f64,
                        "bytes",
                    ),
                    resource_metric(
                        context,
                        &resource,
                        "disk.io.read.rate",
                        per_second(usage.read_bytes, elapsed),
                        "bytes_per_second",
                    ),
                    resource_metric(
                        context,
                        &resource,
                        "disk.io.write.rate",
                        per_second(usage.written_bytes, elapsed),
                        "bytes_per_second",
                    ),
                ]);
            }
        }
        Ok(metrics)
    }
}

fn resource_metric(
    context: &CollectionContext,
    resource: &str,
    name: &str,
    value: f64,
    unit: &str,
) -> Metric {
    Metric::for_resource(context.collected_at, "disk", resource, name, value, unit)
}

fn percentage(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64 * 100.0
    }
}

fn per_second(value: u64, elapsed: std::time::Duration) -> f64 {
    value as f64 / elapsed.as_secs_f64()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn calculates_percentages_without_dividing_by_zero() {
        assert_eq!(percentage(50, 200), 25.0);
        assert_eq!(percentage(50, 0), 0.0);
    }

    #[test]
    fn calculates_bytes_per_second_from_elapsed_time() {
        assert_eq!(per_second(1_000, Duration::from_millis(500)), 2_000.0);
    }

    #[tokio::test]
    async fn warms_up_io_rates_before_emitting_them() {
        let mut collector = DiskCollector::new();
        if collector.disks.is_empty() {
            return;
        }
        let collected_at = chrono::Utc::now();
        let warm_up = CollectionContext {
            collected_at,
            elapsed: None,
        };
        let first = collector.collect(&warm_up).await.expect("disk metrics");
        assert!(
            first
                .iter()
                .all(|metric| !metric.name.starts_with("disk.io."))
        );
        assert!(first.iter().all(|metric| metric.resource.is_some()));

        let sampled = CollectionContext {
            collected_at,
            elapsed: Some(Duration::from_secs(1)),
        };
        let second = collector.collect(&sampled).await.expect("disk metrics");
        assert!(
            second
                .iter()
                .any(|metric| metric.name == "disk.io.read.rate")
        );
        assert!(
            second
                .iter()
                .all(|metric| metric.collected_at == collected_at)
        );
    }
}
