# Simple Profiler — Project Overview

> **Type:** Explanation
> **Audience:** Developers, AI assistants, and tooling that needs project context
> **Last updated:** 2026-07-15
>
> A local-first system profiler for collecting evidence needed to diagnose host problems.
> Related docs: [domain-models.md](domain-models.md), [coding-style.md](coding-style.md),
> and [../DESIGN.md](../DESIGN.md).

---

## 1. Purpose

### 1.1 Core Responsibilities

Simple Profiler continuously samples host resource metrics, persists them locally, and will
eventually turn selected time ranges into diagnostic reports. The implemented MVP collects CPU
and memory measurements and writes them to SQLite. Disk, network, GPU, anomaly detection,
retention, reports, and a local dashboard are planned.

### 1.2 Relationship with Other Systems

The current program is standalone. It reads host information through `sysinfo`, writes a local
SQLite file, and makes no network calls at runtime. Future GPU collectors will use platform- or
vendor-specific adapters, but their exact APIs are not yet designed.

### 1.3 Deprecated / Retired or Not-Yet-Enabled Features

- **Not yet enabled:** disk, network, GPU, process, temperature, and power collectors.
- **Not yet enabled:** rollups, retention, anomaly events, reports, and the dashboard.
- No deprecated features exist in the initial version.

## 2. Tech Stack

| Component | Choice | Evidence / rationale |
|---|---|---|
| Language | Rust 1.92.0, edition 2024 | Selected by the user for predictable resource use and low-level system access; pinned as the MSRV in [`../clippy.toml`](../clippy.toml) |
| Async runtime | Tokio 1.52.3 | Timed collection, bounded channels, shutdown signals, and the blocking storage task |
| Host metrics | sysinfo 0.38.4 | Cross-platform CPU and memory access; this is the newest locked release compatible with Rust 1.92.0 |
| Local datastore | SQLite through rusqlite 0.39.0 | A local embedded store fits a single-host profiler; rusqlite supports an explicit single-writer design |
| CLI | Clap 4.6.1 | Defines the implemented `run` and `status` commands |
| Configuration | TOML through toml 1.1.3 and Serde | Human-readable local configuration with typed validation |
| Logs | tracing and tracing-subscriber | Structured runtime lifecycle and collection messages |
| Error handling | anyhow and thiserror | Application context plus typed collector errors |

Rust was chosen over Go because the user explicitly selected Rust after reviewing the trade-off.
Go remains a reasonable alternative for faster initial development but is not part of this
project. SQLx was considered for SQLite; rusqlite was used because the storage design owns one
synchronous connection inside one blocking writer task and does not need an async connection pool.

## 3. Architecture Overview

Simple Profiler is a modular monolith compiled as one executable. The current runtime flow is:

```text
tokio interval
    │
    ▼
SystemCollector ── MetricBatch ──► bounded mpsc channel
                                           │
                                           ▼
                                  blocking Storage writer
                                           │
                                           ▼
                                    SQLite (WAL mode)
```

The CLI assembles configuration and chooses an operation. `run` starts collection and the storage
writer; `status` opens the same SQLite database and reads its sample count and time range.

### Key Principles

- Collectors MUST only collect and normalize measurements; they do not write storage directly.
- The collector-to-storage channel MUST remain bounded so slow storage creates backpressure.
- One writer task owns the SQLite connection to avoid write-lock contention.
- Blocking database work runs outside Tokio's asynchronous worker threads.
- A collector failure SHOULD be isolated and logged without corrupting already stored data.
- Implemented and planned capabilities MUST be labeled separately.

## 4. Directory Structure

```text
.
├── config/
│   └── default.toml       # Tracked example settings
├── docs/                  # Architecture, domain, and coding references
├── src/
│   ├── collector/
│   │   ├── mod.rs         # Collector contract and shared error type
│   │   └── system.rs      # CPU and memory collector
│   ├── config.rs          # TOML model, defaults, and validation
│   ├── lib.rs             # Library module exports
│   ├── main.rs            # Clap CLI and command dispatch
│   ├── model.rs           # Normalized Metric and MetricBatch types
│   ├── runtime.rs         # Timed collection and graceful shutdown
│   └── storage.rs         # SQLite schema, queries, and writer task
├── AGENTS.md              # AI-agent routing and hard constraints
├── CLAUDE.md              # Symlink to AGENTS.md
├── DESIGN.md              # Planned dashboard design contract
├── PROGRESS.md            # Cross-session implementation state
├── Cargo.toml             # Package manifest
├── clippy.toml            # Clippy MSRV configuration
└── rustfmt.toml           # Rustfmt edition and line width
```

Future collector modules SHOULD be added below `src/collector/`. Report and dashboard directories
are TBD — not yet designed.

## 5. Domain Models (High-Level)

The current persistent model is a timestamped metric sample. Each collection cycle produces one
transient batch containing total CPU, per-core CPU, and memory metrics.

```text
CollectionCycle 1──* MetricSample
```

The `CollectionCycle` boundary currently exists only as an in-memory `MetricBatch`; it is not a
database table. Planned diagnostic entities include Device, Event, Rollup, and Report, but no
schema exists for them. See [domain-models.md](domain-models.md) for status and field details.

## 6. API / Interface Structure

Simple Profiler has a CLI interface and no HTTP interface yet.

| Command | Purpose | Important options |
|---|---|---|
| `simple-profiler run` | Collect until interrupted or a sample limit is reached | `--database`, `--interval-seconds`, `--samples` |
| `simple-profiler status` | Print database path, metric row count, and stored time range | `--database` |

The global `--config <PATH>` option loads TOML settings. The local dashboard and report commands
are TBD — not yet designed.

## 7. Background Jobs & Scheduled Tasks

| Task | Trigger | Current behavior |
|---|---|---|
| System metric collection | Tokio interval; five seconds by default | Refresh CPU and memory, normalize the results, and send one batch to storage |
| SQLite writer | Each received batch | Insert the complete batch inside one transaction |
| Graceful shutdown | Ctrl-C or `--samples` limit | Close the channel, drain queued batches, then stop |

Data rollup, retention cleanup, event evaluation, and scheduled report generation are TBD — not
yet designed.

## 8. External Service Integrations

The implemented runtime has no external service or network integration. `sysinfo` is an in-process
platform abstraction, and bundled SQLite is compiled with the application. Future NVIDIA, AMD, and
Apple GPU adapter choices are TBD — not yet designed.

## 9. Database / Data Stores

The application owns one client-embedded SQLite database. The schema is created idempotently from
[`../src/storage.rs`](../src/storage.rs); there is no separate migration framework yet.

| Table | Purpose | Indexes |
|---|---|---|
| `metric_samples` | One normalized measurement per timestamp, collector, metric name, value, and unit | `collected_at`; `(metric_name, collected_at)` |

SQLite runs in WAL mode with a five-second busy timeout. Metric batches are written inside a
transaction. Retention duration, rollup tables, schema versioning, and database size limits are
TBD — not yet designed. Because this is an embedded local store, the server-database observation
module does not apply.

## 10. Environments & Deployment

### Environments

Only local development and direct local execution exist. macOS is the first development platform;
Linux and Windows support are architectural goals but are not yet verified.

### Deployment Pipeline

TBD — not yet designed. There is no CI, packaged release, macOS LaunchAgent, Linux systemd unit,
or Windows Service definition yet.

### Configuration Hierarchy

Configuration precedence is:

1. Command-line overrides for database path and interval
2. A TOML file passed with `--config`
3. Built-in defaults matching [`../config/default.toml`](../config/default.toml)

Secrets are not currently required. The default database path is
`data/simple-profiler.sqlite3`, the interval is five seconds, and the bounded channel capacity is
128 batches.

