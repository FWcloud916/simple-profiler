use async_trait::async_trait;
use sysinfo::Networks;

use super::{CollectionContext, Collector, CollectorError};
use crate::model::{Metric, MetricBatch};

pub struct NetworkCollector {
    networks: Networks,
    suppress_idle_io: bool,
}

impl NetworkCollector {
    pub fn new() -> Self {
        Self::with_suppress_idle_io(true)
    }

    pub fn with_suppress_idle_io(suppress_idle_io: bool) -> Self {
        Self {
            networks: Networks::new_with_refreshed_list(),
            suppress_idle_io,
        }
    }
}

impl Default for NetworkCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Collector for NetworkCollector {
    fn name(&self) -> &'static str {
        "network"
    }

    async fn collect(
        &mut self,
        context: &CollectionContext,
    ) -> Result<MetricBatch, CollectorError> {
        self.networks.refresh(true);
        if self.networks.is_empty() {
            return Err(CollectorError::Unavailable(
                "no network interfaces were reported by the operating system".to_owned(),
            ));
        }

        let Some(elapsed) = context.elapsed.filter(|elapsed| !elapsed.is_zero()) else {
            return Ok(Vec::new());
        };
        let mut metrics = Vec::with_capacity(self.networks.len() * 8);
        for (interface, data) in self.networks.list() {
            let received = data.received();
            let transmitted = data.transmitted();
            let packets_received = data.packets_received();
            let packets_transmitted = data.packets_transmitted();
            let receive_errors = data.errors_on_received();
            let transmit_errors = data.errors_on_transmitted();
            if !should_emit_io(
                [
                    received,
                    transmitted,
                    packets_received,
                    packets_transmitted,
                    receive_errors,
                    transmit_errors,
                ],
                self.suppress_idle_io,
            ) {
                continue;
            }
            metrics.extend([
                resource_metric(
                    context,
                    interface,
                    "network.receive.delta",
                    received as f64,
                    "bytes",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.transmit.delta",
                    transmitted as f64,
                    "bytes",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.receive.rate",
                    per_second(received, elapsed),
                    "bytes_per_second",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.transmit.rate",
                    per_second(transmitted, elapsed),
                    "bytes_per_second",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.receive.packets.delta",
                    packets_received as f64,
                    "packets",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.transmit.packets.delta",
                    packets_transmitted as f64,
                    "packets",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.receive.errors.delta",
                    receive_errors as f64,
                    "errors",
                ),
                resource_metric(
                    context,
                    interface,
                    "network.transmit.errors.delta",
                    transmit_errors as f64,
                    "errors",
                ),
            ]);
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
    Metric::for_resource(context.collected_at, "network", resource, name, value, unit)
}

fn per_second(value: u64, elapsed: std::time::Duration) -> f64 {
    value as f64 / elapsed.as_secs_f64()
}

fn should_emit_io(counters: [u64; 6], suppress_idle_io: bool) -> bool {
    !suppress_idle_io || counters.into_iter().any(|value| value != 0)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn calculates_bytes_per_second_from_elapsed_time() {
        assert_eq!(per_second(3_000, Duration::from_secs(2)), 1_500.0);
    }

    #[test]
    fn suppresses_only_fully_idle_interfaces_when_enabled() {
        assert!(!should_emit_io([0; 6], true));
        assert!(should_emit_io([0, 0, 1, 0, 0, 0], true));
        assert!(should_emit_io([0; 6], false));
    }

    #[tokio::test]
    async fn warms_up_before_emitting_network_rates() {
        let mut collector = NetworkCollector::with_suppress_idle_io(false);
        if collector.networks.is_empty() {
            return;
        }
        let collected_at = chrono::Utc::now();
        let warm_up = CollectionContext {
            collected_at,
            elapsed: None,
        };
        let first = collector.collect(&warm_up).await.expect("network metrics");
        assert!(first.is_empty());

        let sampled = CollectionContext {
            collected_at,
            elapsed: Some(Duration::from_secs(1)),
        };
        let second = collector.collect(&sampled).await.expect("network metrics");
        assert!(
            second
                .iter()
                .any(|metric| metric.name == "network.receive.rate")
        );
        assert!(second.iter().all(|metric| metric.resource.is_some()));
        assert!(
            second
                .iter()
                .all(|metric| metric.collected_at == collected_at)
        );
    }
}
