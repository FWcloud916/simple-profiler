use std::time::Duration;

use anyhow::{Context, Result};
use tokio::{sync::mpsc, time::MissedTickBehavior};
use tracing::{info, warn};

use crate::{
    collector::{Collector, SystemCollector},
    config::AppConfig,
    storage::spawn_writer,
};

pub async fn run_profiler(config: AppConfig, sample_limit: Option<u64>) -> Result<()> {
    config.validate()?;
    let (sender, receiver) = mpsc::channel(config.channel_capacity);
    let writer = spawn_writer(&config.database_path, receiver);
    let mut collector = SystemCollector::new();
    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut collected_cycles = 0_u64;

    info!(
        database = %config.database_path.display(),
        interval_seconds = config.interval_seconds,
        "profiler started"
    );

    loop {
        tokio::select! {
            _ = interval.tick() => {
                match collector.collect().await {
                    Ok(batch) => {
                        let metric_count = batch.len();
                        sender.send(batch).await.context("storage writer stopped")?;
                        collected_cycles += 1;
                        info!(collected_cycles, metric_count, "metrics collected");

                        if sample_limit.is_some_and(|limit| collected_cycles >= limit) {
                            break;
                        }
                    }
                    Err(error) => warn!(collector = collector.name(), %error, "collection failed"),
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
