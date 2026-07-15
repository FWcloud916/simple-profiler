use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use simple_profiler::{
    AppConfig, logging, run_profiler,
    service::{ServiceManager, ServiceStatus},
    storage::{ProcessSort, Storage},
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

    /// List or inspect detected system anomalies.
    Events {
        #[command(subcommand)]
        action: EventsCommand,
    },

    /// Inspect the latest resource-heavy processes.
    Processes {
        #[command(subcommand)]
        action: ProcessesCommand,
    },

    /// Install and manage the macOS background service.
    Service {
        #[command(subcommand)]
        action: ServiceCommand,
    },
}

#[derive(Debug, Subcommand)]
enum EventsCommand {
    /// List recent anomaly events.
    List {
        /// Show only events that are still open.
        #[arg(long)]
        open: bool,
        /// Maximum number of events to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Override the SQLite database path.
        #[arg(long)]
        database: Option<PathBuf>,
    },
    /// Show one event and its preserved evidence samples.
    Show {
        /// Event identifier shown by `events list`.
        id: i64,
        /// Override the SQLite database path.
        #[arg(long)]
        database: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum ProcessesCommand {
    /// Show the latest top processes by CPU or resident memory.
    Top {
        /// Resource used to rank the processes.
        #[arg(long, value_enum, default_value_t = ProcessSortArg::Cpu)]
        sort: ProcessSortArg,
        /// Maximum number of processes to show.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Override the SQLite database path.
        #[arg(long)]
        database: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProcessSortArg {
    Cpu,
    Memory,
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
            print_dataset("process samples", &status.processes);
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
            println!(
                "anomalies: warning={}, critical={}, latest={}",
                status.open_warning_count,
                status.open_critical_count,
                format_time(status.latest_event_ms)
            );
            Ok(())
        }
        Command::Events { action } => handle_events(action, &mut config),
        Command::Processes { action } => handle_processes(action, &mut config),
        Command::Service { action } => handle_service(action),
    }
}

fn handle_events(action: EventsCommand, config: &mut AppConfig) -> Result<()> {
    match action {
        EventsCommand::List {
            open,
            limit,
            database,
        } => {
            if !(1..=1_000).contains(&limit) {
                anyhow::bail!("--limit must be between 1 and 1000");
            }
            if let Some(database) = database {
                config.database_path = database;
            }
            let storage = Storage::open(&config.database_path)?;
            let events = storage.list_events(open, limit)?;
            if events.is_empty() {
                println!("no anomaly events");
                return Ok(());
            }
            for event in events {
                let resource = if event.resource.is_empty() {
                    "system"
                } else {
                    &event.resource
                };
                println!(
                    "#{} {} {} {} {} peak={:.2} started={} ended={}",
                    event.id,
                    event.status,
                    event.severity,
                    event.metric_name,
                    resource,
                    event.peak_value,
                    format_time(Some(event.started_at_ms)),
                    format_time(event.ended_at_ms)
                );
            }
            Ok(())
        }
        EventsCommand::Show { id, database } => {
            if let Some(database) = database {
                config.database_path = database;
            }
            let storage = Storage::open(&config.database_path)?;
            let Some(event) = storage.event(id)? else {
                anyhow::bail!("anomaly event #{id} was not found");
            };
            let resource = if event.summary.resource.is_empty() {
                "system"
            } else {
                &event.summary.resource
            };
            println!("event: #{}", event.summary.id);
            println!("rule: {}", event.summary.rule_id);
            println!(
                "metric: {} ({resource}, {})",
                event.summary.metric_name, event.unit
            );
            println!("state: {} {}", event.summary.status, event.summary.severity);
            println!(
                "time: {} -> {} (detected {})",
                format_time(Some(event.summary.started_at_ms)),
                format_time(event.summary.ended_at_ms),
                format_time(Some(event.detected_at_ms))
            );
            println!(
                "thresholds: warning={:.2}, critical={:.2}, recovery={:.2}",
                event.warning_threshold, event.critical_threshold, event.recovery_threshold
            );
            println!(
                "values: peak={:.2} at {}, last={:.2} at {}, samples={}, gaps={}",
                event.summary.peak_value,
                format_time(Some(event.summary.peak_at_ms)),
                event.last_value,
                format_time(Some(event.last_sample_ms)),
                event.sample_count,
                event.data_gap_count
            );
            println!("evidence:");
            for evidence in event.evidence {
                println!(
                    "  {} {:>10.2} {}",
                    format_time(Some(evidence.collected_at_ms)),
                    evidence.value,
                    evidence.kind
                );
            }
            if !event.process_evidence.is_empty() {
                println!("related processes:");
                for evidence in event.process_evidence {
                    print_process_sample(&evidence.sample, Some(&evidence.kind), None);
                }
            }
            Ok(())
        }
    }
}

fn handle_processes(action: ProcessesCommand, config: &mut AppConfig) -> Result<()> {
    match action {
        ProcessesCommand::Top {
            sort,
            limit,
            database,
        } => {
            if !(1..=100).contains(&limit) {
                anyhow::bail!("--limit must be between 1 and 100");
            }
            if let Some(database) = database {
                config.database_path = database;
            }
            let sort = match sort {
                ProcessSortArg::Cpu => ProcessSort::Cpu,
                ProcessSortArg::Memory => ProcessSort::Memory,
            };
            let storage = Storage::open(&config.database_path)?;
            let processes = storage.latest_processes(sort, limit)?;
            if processes.is_empty() {
                println!("no process snapshots");
                return Ok(());
            }
            println!(
                "process snapshot: {}",
                format_time(Some(processes[0].collected_at_ms))
            );
            for process in processes {
                let rank = match sort {
                    ProcessSort::Cpu => process.cpu_rank,
                    ProcessSort::Memory => process.memory_rank,
                };
                print_process_sample(&process, None, rank);
            }
            Ok(())
        }
    }
}

fn print_process_sample(
    process: &simple_profiler::storage::StoredProcessSample,
    kind: Option<&str>,
    rank: Option<u32>,
) {
    let prefix = kind.map_or_else(
        || rank.map_or_else(|| "".to_owned(), |rank| format!("#{rank} ")),
        |kind| format!("[{kind}] {} ", format_time(Some(process.collected_at_ms))),
    );
    println!(
        "  {prefix}pid={} {} cpu={:.2}% memory={}{}",
        process.pid,
        process.name,
        process.cpu_usage_percent,
        format_bytes(process.memory_bytes),
        process
            .executable_path
            .as_deref()
            .map_or_else(String::new, |path| format!(" path={path}"))
    );
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1_024.0;
    const MIB: f64 = KIB * 1_024.0;
    const GIB: f64 = MIB * 1_024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
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
            println!("command: {}", manager.paths().cli_launcher.display());
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
        "latest process snapshot: {}",
        format_time(status.processes.newest_ms)
    );
    println!(
        "last maintenance: {} ({})",
        format_time(status.last_maintenance_ms),
        status
            .last_maintenance_result
            .as_deref()
            .unwrap_or("not run")
    );
    println!(
        "open anomalies: warning={}, critical={}",
        status.open_warning_count, status.open_critical_count
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
