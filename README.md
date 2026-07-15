# Simple Profiler

A local-first Rust service that continuously records system metrics for later diagnosis.

## What it does

- Collects CPU, memory, disk capacity/I/O, and network transfer metrics at a configurable interval.
- Suppresses idle disk/network I/O by default and samples disk capacity every 60 seconds to limit
  storage growth.
- Combines successful collectors into one cycle batch and sends it through a bounded channel.
- Stores timestamped, resource-aware samples in a local SQLite database using one WAL writer.
- Retains raw samples for 24 hours, one-minute rollups for 30 days, and 15-minute rollups for 365
  days by default.
- Reports schema version, row counts and time ranges by resolution, database/WAL size, rollup
  watermarks, and maintenance status from the command line.
- Installs and supervises itself as a per-user macOS LaunchAgent, with graceful shutdown,
  single-instance protection, service health output, and bounded log rotation.

GPU collection, anomaly detection, diagnostic reports, and the dashboard are planned but are not
implemented yet.

## Quickstart

### Prerequisites

- Rust 1.92.0 or a compatible newer toolchain

### Setup

```bash
cargo build
```

### Run

Run continuously with the default settings:

```bash
cargo run -- run
```

Collect two cycles into a temporary database, then inspect it:

```bash
cargo run -- run --database /tmp/simple-profiler.sqlite3 --interval-seconds 1 --samples 2
cargo run -- status --database /tmp/simple-profiler.sqlite3
```

Load settings from the tracked example configuration:

```bash
cargo run -- --config config/default.toml run
```

### Run in the background on macOS

Build an optimized binary, then explicitly install and start the per-user LaunchAgent:

```bash
cargo build --release
target/release/simple-profiler service install
```

The install command copies the executable, creates a private default configuration when none
exists, writes `~/Library/LaunchAgents/com.simple-profiler.agent.plist`, and starts the service.
It changes the current user's macOS service state and SHOULD only be run intentionally.

Inspect or manage it with:

```bash
target/release/simple-profiler service status
target/release/simple-profiler service stop
target/release/simple-profiler service start
target/release/simple-profiler service restart
target/release/simple-profiler service uninstall
```

Normal uninstall preserves configuration, metrics, and logs. The destructive
`service uninstall --purge` variant removes them too.

### Test

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

## Project structure

```text
config/        Example runtime configuration
docs/          Architecture and development references
src/           CLI, collectors, runtime coordination, models, and SQLite storage
Cargo.toml     Rust package and dependency manifest
PROGRESS.md    Cross-session implementation state
DESIGN.md      Planned dashboard design contract
```

## Documentation

| Doc | What it covers |
|---|---|
| [docs/project-overview.md](docs/project-overview.md) | Architecture, directory map, interfaces, storage, and deployment |
| [docs/domain-models.md](docs/domain-models.md) | Metric data and planned diagnostic entities |
| [docs/coding-style.md](docs/coding-style.md) | Rust formatting, linting, and project conventions |
| [DESIGN.md](DESIGN.md) | Planned dashboard design tokens and visual rules |
