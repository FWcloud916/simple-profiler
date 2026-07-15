use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::{sync::mpsc, time::MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::model::CollectionBatch;
use crate::{
    collector::{
        CollectionContext, Collector, DiskCollector, GpuCollector, NetworkCollector,
        ProcessCollector, SystemCollector,
    },
    config::AppConfig,
    instance::InstanceLock,
    storage::spawn_writer,
};

pub async fn run_profiler(config: AppConfig, sample_limit: Option<u64>) -> Result<()> {
    let disk_capacity_interval =
        Duration::from_secs(config.sampling.disk_capacity_interval_seconds);
    let suppress_idle_io = config.sampling.suppress_idle_io;
    let collectors: Vec<Box<dyn Collector>> = vec![
        Box::new(SystemCollector::new()),
        Box::new(DiskCollector::with_options(
            disk_capacity_interval,
            suppress_idle_io,
        )),
        Box::new(NetworkCollector::with_suppress_idle_io(suppress_idle_io)),
    ];
    run_with_collectors(config, sample_limit, collectors).await
}

async fn run_with_collectors(
    config: AppConfig,
    sample_limit: Option<u64>,
    mut collectors: Vec<Box<dyn Collector>>,
) -> Result<()> {
    config.validate()?;
    let _instance_lock = InstanceLock::acquire(&config.database_path)?;
    let (sender, receiver) = mpsc::channel(config.channel_capacity);
    let mut process_collector = ProcessCollector::new(config.process.clone());
    let mut gpu_collector = GpuCollector::new(config.gpu.clone());
    let writer = spawn_writer(
        &config.database_path,
        config.retention.clone(),
        config.anomaly.clone(),
        config.process.clone(),
        receiver,
    );
    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut collected_cycles = 0_u64;
    let mut previous_cycle = None;

    info!(
        database = %config.database_path.display(),
        interval_seconds = config.interval_seconds,
        "profiler started"
    );

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let cycle_started = Instant::now();
                let context = CollectionContext {
                    collected_at: Utc::now(),
                    elapsed: previous_cycle.map(|previous| cycle_started.duration_since(previous)),
                };
                previous_cycle = Some(cycle_started);
                let mut cycle_batch = Vec::new();

                for collector in &mut collectors {
                    match collector.collect(&context).await {
                        Ok(mut batch) => cycle_batch.append(&mut batch),
                        Err(error) => {
                            warn!(collector = collector.name(), %error, "collection failed");
                        }
                    }
                }

                let process = match process_collector.collect(&context).await {
                    Ok(Some(collection)) => collection,
                    Ok(None) => Default::default(),
                    Err(error) => {
                        warn!(collector = "process", %error, "collection failed");
                        Default::default()
                    }
                };
                for warning in &process.warnings {
                    warn!(collector = "process", error = %warning, "collection degraded");
                }
                let gpu = gpu_collector.collect(&context).await.unwrap_or_default();
                if let Some(warning) = &gpu.warning {
                    warn!(collector = "gpu", error = %warning, "collection degraded");
                }
                cycle_batch.extend(gpu.metrics);
                let metric_count = cycle_batch.len();
                let process_count = process.samples.len();
                let capability_count = gpu.capabilities.len() + process.capabilities.len();
                let mut capabilities = gpu.capabilities;
                capabilities.extend(process.capabilities);
                let storage_batch = CollectionBatch {
                    metrics: cycle_batch,
                    processes: process.samples,
                    capabilities,
                };
                if !storage_batch.is_empty() {
                    sender.send(storage_batch).await.context("storage writer stopped")?;
                }
                collected_cycles += 1;
                debug!(
                    collected_cycles,
                    metric_count,
                    process_count,
                    capability_count,
                    "collection cycle completed"
                );

                if sample_limit.is_some_and(|limit| collected_cycles >= limit) {
                    break;
                }
            }
            signal = &mut shutdown => {
                signal?;
                info!("shutdown signal received");
                break;
            }
        }
    }

    drop(sender);
    writer.await.context("storage writer task panicked")??;
    info!("profiler stopped");
    Ok(())
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate =
            signal(SignalKind::terminate()).context("failed to listen for SIGTERM")?;
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for shutdown signal")?;
            }
            _ = terminate.recv() => {}
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("failed to listen for shutdown signal")
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        collector::CollectorError,
        model::{Metric, MetricBatch},
        storage::Storage,
    };

    struct SuccessfulCollector;

    #[async_trait]
    impl Collector for SuccessfulCollector {
        fn name(&self) -> &'static str {
            "success"
        }

        async fn collect(
            &mut self,
            context: &CollectionContext,
        ) -> Result<MetricBatch, CollectorError> {
            Ok(vec![Metric::new(
                context.collected_at,
                self.name(),
                "test.value",
                1.0,
                "count",
            )])
        }
    }

    struct UnavailableCollector;

    #[async_trait]
    impl Collector for UnavailableCollector {
        fn name(&self) -> &'static str {
            "unavailable"
        }

        async fn collect(
            &mut self,
            _context: &CollectionContext,
        ) -> Result<MetricBatch, CollectorError> {
            Err(CollectorError::Unavailable("not supported".to_owned()))
        }
    }

    #[tokio::test]
    async fn stores_successful_metrics_when_another_collector_fails() {
        let directory = tempdir().expect("temp dir");
        let database_path = directory.path().join("metrics.sqlite3");
        let config = AppConfig {
            database_path: database_path.clone(),
            interval_seconds: 1,
            channel_capacity: 4,
            gpu: crate::config::GpuConfig {
                enabled: false,
                ..crate::config::GpuConfig::default()
            },
            ..AppConfig::default()
        };
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(UnavailableCollector),
            Box::new(SuccessfulCollector),
        ];

        run_with_collectors(config, Some(1), collectors)
            .await
            .expect("profiler run");

        let storage = Storage::open(&database_path).expect("storage");
        assert_eq!(storage.status().expect("status").raw.row_count, 1);
    }

    #[tokio::test]
    async fn does_not_write_an_empty_batch_when_all_collectors_fail() {
        let directory = tempdir().expect("temp dir");
        let database_path = directory.path().join("metrics.sqlite3");
        let config = AppConfig {
            database_path: database_path.clone(),
            interval_seconds: 1,
            channel_capacity: 4,
            gpu: crate::config::GpuConfig {
                enabled: false,
                ..crate::config::GpuConfig::default()
            },
            ..AppConfig::default()
        };
        let collectors: Vec<Box<dyn Collector>> = vec![Box::new(UnavailableCollector)];

        run_with_collectors(config, Some(1), collectors)
            .await
            .expect("profiler run");

        let storage = Storage::open(&database_path).expect("storage");
        assert_eq!(storage.status().expect("status").raw.row_count, 0);
    }
}
