# Simple Profiler

A local-first Rust service that continuously records system metrics for later diagnosis.

## What it does

- Collects total and per-core CPU usage plus memory usage at a configurable interval.
- Sends metric batches through a bounded channel to one SQLite writer.
- Stores timestamped samples in a local SQLite database using WAL mode.
- Reports the number and time range of stored samples from the command line.

Disk, network, GPU, anomaly detection, retention, reports, and the dashboard are planned but
are not implemented yet.

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

