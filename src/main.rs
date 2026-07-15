use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use simple_profiler::{AppConfig, run_profiler, storage::Storage};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "simple-profiler",
    version,
    about = "Low-overhead local system profiler"
)]
struct Cli {
    /// Optional TOML configuration file.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Collect CPU, memory, disk, and network metrics until interrupted.
    Run {
        /// Override the SQLite database path.
        #[arg(long)]
        database: Option<PathBuf>,

        /// Override the collection interval.
        #[arg(long)]
        interval_seconds: Option<u64>,

        /// Stop after this many collection cycles; useful for diagnostics and scripts.
        #[arg(long)]
        samples: Option<u64>,
    },

    /// Show the amount and time range of data currently stored.
    Status {
        /// Override the SQLite database path.
        #[arg(long)]
        database: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let mut config = match cli.config {
        Some(path) => AppConfig::from_file(&path)?,
        None => AppConfig::default(),
    };

    match cli.command {
        Command::Run {
            database,
            interval_seconds,
            samples,
        } => {
            if let Some(database) = database {
                config.database_path = database;
            }
            if let Some(interval_seconds) = interval_seconds {
                config.interval_seconds = interval_seconds;
            }
            config.validate()?;
            run_profiler(config, samples).await
        }
        Command::Status { database } => {
            if let Some(database) = database {
                config.database_path = database;
            }
            let storage = Storage::open(&config.database_path)?;
            let status = storage.status()?;
            println!("database: {}", config.database_path.display());
            println!("schema: v{}", status.schema_version);
            print_dataset("raw", &status.raw);
            print_dataset("1 minute", &status.minute);
            print_dataset("15 minute", &status.quarter_hour);
            println!(
                "storage: database={} bytes, wal={} bytes, reusable={} bytes",
                status.database_bytes, status.wal_bytes, status.free_page_bytes
            );
            println!(
                "watermarks: 1 minute={}, 15 minute={}",
                format_time(status.minute_watermark_ms),
                format_time(status.quarter_hour_watermark_ms)
            );
            println!(
                "maintenance: {} ({})",
                format_time(status.last_maintenance_ms),
                status
                    .last_maintenance_result
                    .as_deref()
                    .unwrap_or("not run")
            );
            Ok(())
        }
    }
}

fn print_dataset(label: &str, dataset: &simple_profiler::storage::DatasetStatus) {
    println!(
        "{label}: {} rows, {} -> {}",
        dataset.row_count,
        format_time(dataset.oldest_ms),
        format_time(dataset.newest_ms)
    );
}

fn format_time(timestamp_ms: Option<i64>) -> String {
    timestamp_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(|| "no data".to_owned(), |time| time.to_rfc3339())
}
