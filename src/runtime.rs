use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::{sync::mpsc, time::MissedTickBehavior};
use tracing::{info, warn};

use crate::{
    collector::{CollectionContext, Collector, DiskCollector, NetworkCollector, SystemCollector},
    config::AppConfig,
    storage::spawn_writer,
};

pub async fn run_profiler(config: AppConfig, sample_limit: Option<u64>) -> Result<()> {
    let collectors: Vec<Box<dyn Collector>> = vec![
        Box::new(SystemCollector::new()),
        Box::new(DiskCollector::new()),
        Box::new(NetworkCollector::new()),
    ];
    run_with_collectors(config, sample_limit, collectors).await
}

async fn run_with_collectors(
    config: AppConfig,
    sample_limit: Option<u64>,
    mut collectors: Vec<Box<dyn Collector>>,
) -> Result<()> {
    config.validate()?;
    let (sender, receiver) = mpsc::channel(config.channel_capacity);
    let writer = spawn_writer(&config.database_path, receiver);
    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut collected_cycles = 0_u64;
    let mut previous_cycle = None;

    info!(
        database = %config.database_path.display(),
        interval_seconds = config.interval_seconds,
        "profiler started"
    );

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

                let metric_count = cycle_batch.len();
                if !cycle_batch.is_empty() {
                    sender.send(cycle_batch).await.context("storage writer stopped")?;
                }
                collected_cycles += 1;
                info!(collected_cycles, metric_count, "collection cycle completed");

                if sample_limit.is_some_and(|limit| collected_cycles >= limit) {
                    break;
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for shutdown signal")?;
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
        };
        let collectors: Vec<Box<dyn Collector>> = vec![
            Box::new(UnavailableCollector),
            Box::new(SuccessfulCollector),
        ];

        run_with_collectors(config, Some(1), collectors)
            .await
            .expect("profiler run");

        let storage = Storage::open(&database_path).expect("storage");
        assert_eq!(storage.status().expect("status").sample_count, 1);
    }

    #[tokio::test]
    async fn does_not_write_an_empty_batch_when_all_collectors_fail() {
        let directory = tempdir().expect("temp dir");
        let database_path = directory.path().join("metrics.sqlite3");
        let config = AppConfig {
            database_path: database_path.clone(),
            interval_seconds: 1,
            channel_capacity: 4,
        };
        let collectors: Vec<Box<dyn Collector>> = vec![Box::new(UnavailableCollector)];

        run_with_collectors(config, Some(1), collectors)
            .await
            .expect("profiler run");

        let storage = Storage::open(&database_path).expect("storage");
        assert_eq!(storage.status().expect("status").sample_count, 0);
    }
}
