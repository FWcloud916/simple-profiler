use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use simple_profiler::{
    AppConfig, logging, run_profiler,
    service::{ServiceManager, ServiceStatus},
    storage::Storage,
};

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

    /// Install and manage the macOS background service.
    Service {
        #[command(subcommand)]
        action: ServiceCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Install or upgrade the user LaunchAgent and start it.
    Install,
    /// Start the installed service.
    Start,
    /// Stop the service gracefully.
    Stop,
    /// Stop and start the service gracefully.
    Restart,
    /// Show installation and process state.
    Status,
    /// Remove the service; configuration and data are preserved unless --purge is used.
    Uninstall {
        /// Also remove configuration, metrics, and logs.
        #[arg(long)]
        purge: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = match cli.config {
        Some(path) => AppConfig::from_file(&path)?,
        None => AppConfig::default(),
    };
    logging::init(&config.logging)?;

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
        Command::Service { action } => handle_service(action),
    }
}

fn handle_service(action: ServiceCommand) -> Result<()> {
    let manager = ServiceManager::from_environment()?;
    match action {
        ServiceCommand::Install => {
            manager.install(&std::env::current_exe()?)?;
            println!("service installed and started");
            println!("config: {}", manager.paths().config.display());
            println!("database: {}", manager.paths().database.display());
            println!("logs: {}", manager.paths().logs.display());
        }
        ServiceCommand::Start => {
            manager.start()?;
            println!("service started");
        }
        ServiceCommand::Stop => {
            manager.stop()?;
            println!("service stopped");
        }
        ServiceCommand::Restart => {
            manager.restart()?;
            println!("service restarted");
        }
        ServiceCommand::Status => {
            print_service_status(&manager.status()?, &manager);
            print_service_data_status(&manager)?;
        }
        ServiceCommand::Uninstall { purge } => {
            manager.uninstall(purge)?;
            if purge {
                println!("service, configuration, metrics, and logs removed");
            } else {
                println!("service removed; configuration, metrics, and logs preserved");
            }
        }
    }
    Ok(())
}

fn print_service_status(status: &ServiceStatus, manager: &ServiceManager) {
    println!("installed: {}", status.installed);
    println!("loaded: {}", status.loaded);
    println!("running: {}", status.running());
    println!("state: {}", status.state.as_deref().unwrap_or("unknown"));
    println!(
        "pid: {}",
        status
            .pid
            .map_or_else(|| "none".to_owned(), |pid| pid.to_string())
    );
    println!(
        "last exit code: {}",
        status
            .last_exit_code
            .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
    );
    println!("config: {}", manager.paths().config.display());
    println!("database: {}", manager.paths().database.display());
}

fn print_service_data_status(manager: &ServiceManager) -> Result<()> {
    if !manager.paths().config.is_file() || !manager.paths().database.is_file() {
        println!("data: no database yet");
        return Ok(());
    }

    let config = AppConfig::from_file(&manager.paths().config)?;
    let storage = Storage::open(&config.database_path)?;
    let status = storage.status()?;
    println!("latest sample: {}", format_time(status.raw.newest_ms));
    println!(
        "last maintenance: {} ({})",
        format_time(status.last_maintenance_ms),
        status
            .last_maintenance_result
            .as_deref()
            .unwrap_or("not run")
    );
    Ok(())
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
