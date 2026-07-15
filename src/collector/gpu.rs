use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::process::Command;

use crate::{
    config::{GpuConfig, GpuProvider},
    model::{CapabilityBatch, CapabilityState, CollectorCapability, Metric, MetricBatch},
};

use super::CollectionContext;

const COLLECTOR: &str = "gpu";
const RESOURCE: &str = "Apple GPU 0";
const PROVIDER: &str = "apple_ioreg";
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Default)]
pub struct GpuCollection {
    pub metrics: MetricBatch,
    pub capabilities: CapabilityBatch,
    pub warning: Option<String>,
}

pub struct GpuCollector {
    config: GpuConfig,
    next_due: Option<Instant>,
    failure_count: u32,
    last_warning: Option<String>,
    disabled_reported: bool,
}

impl GpuCollector {
    pub fn new(config: GpuConfig) -> Self {
        Self {
            config,
            next_due: None,
            failure_count: 0,
            last_warning: None,
            disabled_reported: false,
        }
    }

    pub async fn collect(&mut self, context: &CollectionContext) -> Option<GpuCollection> {
        if !self.config.enabled {
            if self.disabled_reported {
                return None;
            }
            self.disabled_reported = true;
            return Some(GpuCollection {
                capabilities: unavailable_capabilities(
                    context.collected_at.timestamp_millis(),
                    "disabled",
                    "disabled by configuration",
                ),
                ..GpuCollection::default()
            });
        }

        let now = Instant::now();
        if self.next_due.is_some_and(|due| now < due) {
            return None;
        }

        let result = match self.config.provider {
            GpuProvider::Auto | GpuProvider::AppleIoreg if cfg!(target_os = "macos") => {
                collect_apple_ioreg(context, self.config.command_timeout_seconds).await
            }
            GpuProvider::Auto => {
                Err("no supported non-privileged GPU provider was found".to_owned())
            }
            GpuProvider::AppleIoreg => Err("apple_ioreg is available only on macOS".to_owned()),
        };

        match result {
            Ok(collection) => {
                self.failure_count = 0;
                self.next_due = Some(now + Duration::from_secs(self.config.interval_seconds));
                self.last_warning = None;
                Some(collection)
            }
            Err(error) => {
                self.failure_count = self.failure_count.saturating_add(1);
                let multiplier = 1_u64 << self.failure_count.min(5);
                let delay = Duration::from_secs(
                    self.config
                        .interval_seconds
                        .saturating_mul(multiplier)
                        .min(MAX_BACKOFF.as_secs()),
                );
                self.next_due = Some(now + delay);
                let warning =
                    (self.last_warning.as_deref() != Some(error.as_str())).then(|| error.clone());
                self.last_warning = Some(error.clone());
                Some(GpuCollection {
                    capabilities: degraded_capabilities(
                        context.collected_at.timestamp_millis(),
                        &error,
                    ),
                    warning,
                    ..GpuCollection::default()
                })
            }
        }
    }
}

async fn collect_apple_ioreg(
    context: &CollectionContext,
    timeout_seconds: u64,
) -> Result<GpuCollection, String> {
    let mut command = Command::new("/usr/sbin/ioreg");
    command
        .args(["-r", "-c", "AGXAccelerator", "-a", "-d", "1", "-w", "0"])
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(timeout_seconds), command.output())
        .await
        .map_err(|_| format!("ioreg timed out after {timeout_seconds} seconds"))?
        .map_err(|error| format!("could not run ioreg: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            format!("ioreg exited with {}", output.status)
        } else {
            format!("ioreg exited with {}: {detail}", output.status)
        });
    }
    parse_ioreg_plist(&output.stdout, context)
}

#[derive(Debug, Deserialize)]
struct IoregNode {
    #[serde(rename = "gpu-core-count")]
    gpu_core_count: Option<u64>,
    #[serde(rename = "PerformanceStatistics")]
    performance_statistics: Option<PerformanceStatistics>,
}

#[derive(Debug, Deserialize)]
struct PerformanceStatistics {
    #[serde(rename = "Device Utilization %")]
    device_utilization: Option<u64>,
    #[serde(rename = "Renderer Utilization %")]
    renderer_utilization: Option<u64>,
    #[serde(rename = "Tiler Utilization %")]
    tiler_utilization: Option<u64>,
    #[serde(rename = "In use system memory")]
    memory_used: Option<u64>,
    #[serde(rename = "Alloc system memory")]
    memory_allocated: Option<u64>,
}

fn parse_ioreg_plist(source: &[u8], context: &CollectionContext) -> Result<GpuCollection, String> {
    let nodes: Vec<IoregNode> = plist::from_bytes(source)
        .map_err(|error| format!("could not parse ioreg property list: {error}"))?;
    let node = nodes
        .into_iter()
        .find(|node| node.performance_statistics.is_some())
        .ok_or_else(|| "ioreg did not expose AGX PerformanceStatistics".to_owned())?;
    let statistics = node
        .performance_statistics
        .ok_or_else(|| "ioreg did not expose AGX PerformanceStatistics".to_owned())?;
    let checked_at_ms = context.collected_at.timestamp_millis();
    let mut metrics = Vec::with_capacity(5);
    let mut capabilities = Vec::with_capacity(9);

    push_percent(
        &mut metrics,
        &mut capabilities,
        context,
        "gpu.device.usage",
        statistics.device_utilization,
    );
    push_percent(
        &mut metrics,
        &mut capabilities,
        context,
        "gpu.renderer.usage",
        statistics.renderer_utilization,
    );
    push_percent(
        &mut metrics,
        &mut capabilities,
        context,
        "gpu.tiler.usage",
        statistics.tiler_utilization,
    );
    push_bytes(
        &mut metrics,
        &mut capabilities,
        context,
        "gpu.memory.used",
        statistics.memory_used,
    );
    push_bytes(
        &mut metrics,
        &mut capabilities,
        context,
        "gpu.memory.allocated",
        statistics.memory_allocated,
    );
    capabilities.push(capability(
        "gpu.device.identity",
        CapabilityState::Available,
        node.gpu_core_count
            .map(|count| format!("Apple GPU with {count} cores"))
            .or_else(|| Some("Apple AGX GPU".to_owned())),
        checked_at_ms,
    ));
    for (name, detail) in [
        (
            "gpu.memory.total",
            "unified-memory total is not exposed by this adapter",
        ),
        (
            "gpu.power",
            "powermetrics requires superuser privileges and is intentionally not used",
        ),
        (
            "gpu.temperature",
            "no stable non-privileged source is available",
        ),
    ] {
        capabilities.push(capability(
            name,
            CapabilityState::Unavailable,
            Some(detail.to_owned()),
            checked_at_ms,
        ));
    }
    Ok(GpuCollection {
        metrics,
        capabilities,
        warning: None,
    })
}

fn push_percent(
    metrics: &mut MetricBatch,
    capabilities: &mut CapabilityBatch,
    context: &CollectionContext,
    name: &str,
    value: Option<u64>,
) {
    match value {
        Some(value) if value <= 100 => {
            metrics.push(Metric::for_resource(
                context.collected_at,
                COLLECTOR,
                RESOURCE,
                name,
                value as f64,
                "percent",
            ));
            capabilities.push(capability(
                name,
                CapabilityState::Available,
                None,
                context.collected_at.timestamp_millis(),
            ));
        }
        Some(value) => capabilities.push(capability(
            name,
            CapabilityState::Degraded,
            Some(format!("ignored out-of-range value {value}")),
            context.collected_at.timestamp_millis(),
        )),
        None => capabilities.push(capability(
            name,
            CapabilityState::Unavailable,
            Some("ioreg field is missing".to_owned()),
            context.collected_at.timestamp_millis(),
        )),
    }
}

fn push_bytes(
    metrics: &mut MetricBatch,
    capabilities: &mut CapabilityBatch,
    context: &CollectionContext,
    name: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        metrics.push(Metric::for_resource(
            context.collected_at,
            COLLECTOR,
            RESOURCE,
            name,
            value as f64,
            "bytes",
        ));
        capabilities.push(capability(
            name,
            CapabilityState::Available,
            None,
            context.collected_at.timestamp_millis(),
        ));
    } else {
        capabilities.push(capability(
            name,
            CapabilityState::Unavailable,
            Some("ioreg field is missing".to_owned()),
            context.collected_at.timestamp_millis(),
        ));
    }
}

fn capability(
    name: &str,
    state: CapabilityState,
    detail: Option<String>,
    checked_at_ms: i64,
) -> CollectorCapability {
    CollectorCapability {
        collector: COLLECTOR.to_owned(),
        resource: RESOURCE.to_owned(),
        capability: name.to_owned(),
        state,
        provider: PROVIDER.to_owned(),
        detail,
        checked_at_ms,
    }
}

fn degraded_capabilities(checked_at_ms: i64, detail: &str) -> CapabilityBatch {
    let mut capabilities = Vec::new();
    for name in [
        "gpu.device.usage",
        "gpu.renderer.usage",
        "gpu.tiler.usage",
        "gpu.memory.used",
        "gpu.memory.allocated",
        "gpu.device.identity",
    ] {
        capabilities.push(capability(
            name,
            CapabilityState::Degraded,
            Some(detail.to_owned()),
            checked_at_ms,
        ));
    }
    for name in ["gpu.memory.total", "gpu.power", "gpu.temperature"] {
        capabilities.push(capability(
            name,
            CapabilityState::Unavailable,
            Some(detail.to_owned()),
            checked_at_ms,
        ));
    }
    capabilities
}

fn unavailable_capabilities(checked_at_ms: i64, provider: &str, detail: &str) -> CapabilityBatch {
    [
        "gpu.device.usage",
        "gpu.renderer.usage",
        "gpu.tiler.usage",
        "gpu.memory.used",
        "gpu.memory.allocated",
        "gpu.memory.total",
        "gpu.power",
        "gpu.temperature",
        "gpu.device.identity",
    ]
    .into_iter()
    .map(|name| CollectorCapability {
        collector: COLLECTOR.to_owned(),
        resource: RESOURCE.to_owned(),
        capability: name.to_owned(),
        state: CapabilityState::Unavailable,
        provider: provider.to_owned(),
        detail: Some(detail.to_owned()),
        checked_at_ms,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    const FIXTURE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><array><dict>
<key>gpu-core-count</key><integer>10</integer>
<key>PerformanceStatistics</key><dict>
<key>Device Utilization %</key><integer>42</integer>
<key>Renderer Utilization %</key><integer>31</integer>
<key>Tiler Utilization %</key><integer>9</integer>
<key>In use system memory</key><integer>1048576</integer>
<key>Alloc system memory</key><integer>2097152</integer>
</dict></dict></array></plist>"#;

    fn context() -> CollectionContext {
        CollectionContext {
            collected_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            elapsed: None,
        }
    }

    #[test]
    fn parses_structured_apple_gpu_metrics_and_capabilities() {
        let collection = parse_ioreg_plist(FIXTURE, &context()).expect("GPU collection");

        assert_eq!(collection.metrics.len(), 5);
        assert!(collection.metrics.iter().any(|metric| {
            metric.name == "gpu.device.usage" && metric.value == 42.0 && metric.unit == "percent"
        }));
        assert!(
            collection
                .metrics
                .iter()
                .any(|metric| { metric.name == "gpu.memory.used" && metric.value == 1_048_576.0 })
        );
        assert_eq!(collection.capabilities.len(), 9);
        assert!(collection.capabilities.iter().any(|capability| {
            capability.capability == "gpu.power" && capability.state == CapabilityState::Unavailable
        }));
    }

    #[test]
    fn missing_fields_are_unavailable_instead_of_zero() {
        let source = br#"<?xml version="1.0"?><plist version="1.0"><array><dict><key>PerformanceStatistics</key><dict><key>Device Utilization %</key><integer>5</integer></dict></dict></array></plist>"#;
        let collection = parse_ioreg_plist(source, &context()).expect("GPU collection");

        assert_eq!(collection.metrics.len(), 1);
        assert!(collection.capabilities.iter().any(|capability| {
            capability.capability == "gpu.memory.used"
                && capability.state == CapabilityState::Unavailable
        }));
    }

    #[test]
    fn rejects_out_of_range_percent_without_emitting_a_metric() {
        let source = br#"<?xml version="1.0"?><plist version="1.0"><array><dict><key>PerformanceStatistics</key><dict><key>Device Utilization %</key><integer>101</integer></dict></dict></array></plist>"#;
        let collection = parse_ioreg_plist(source, &context()).expect("GPU collection");

        assert!(collection.metrics.is_empty());
        assert!(collection.capabilities.iter().any(|capability| {
            capability.capability == "gpu.device.usage"
                && capability.state == CapabilityState::Degraded
        }));
    }
}
