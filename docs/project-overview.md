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
eventually turn selected time ranges into diagnostic reports. The implemented MVP collects CPU,
memory, disk, and network measurements, writes raw data to SQLite, and maintains one-minute and
15-minute retention tiers. GPU, anomaly detection, reports, and a local dashboard are planned.

### 1.2 Relationship with Other Systems

The current program is standalone. It reads host information through `sysinfo`, writes a local
SQLite file, and makes no network calls at runtime. Future GPU collectors will use platform- or
vendor-specific adapters, but their exact APIs are not yet designed.

### 1.3 Deprecated / Retired or Not-Yet-Enabled Features

- **Not yet enabled:** GPU, process, temperature, and power collectors.
- **Not yet enabled:** anomaly events, reports, and the dashboard.
- No deprecated features exist in the initial version.

## 2. Tech Stack

| Component | Choice | Evidence / rationale |
|---|---|---|
| Language | Rust 1.92.0, edition 2024 | Selected by the user for predictable resource use and low-level system access; pinned as the MSRV in [`../clippy.toml`](../clippy.toml) |
| Async runtime | Tokio 1.52.3 | Timed collection, bounded channels, shutdown signals, and the blocking storage task |
| Host metrics | sysinfo 0.38.4 | Cross-platform CPU, memory, disk, and network access; this is the newest locked release compatible with Rust 1.92.0 |
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
Tokio interval ──► shared CollectionContext
                         │
                         ├──► SystemCollector
                         ├──► DiskCollector
                         └──► NetworkCollector
                                   │
                                   ▼
                      combined successful MetricBatch
                                   │
                                   ▼
                         bounded mpsc channel
                                   │
                                   ▼
                blocking SQLite WAL writer/maintainer
                         │              │
                         ├── raw rows   └── 1m / 15m rollups
```

The CLI assembles configuration and chooses an operation. `run` starts all collectors and the
storage writer; `status` opens the same SQLite database and summarizes each retention tier,
database/WAL size, rollup watermarks, and the last maintenance result. Each cycle supplies one UTC
timestamp and elapsed monotonic duration to every collector. Successful results are combined into
one transaction; one unavailable collector does not discard the others. Disk and network rate
metrics warm up for one cycle. By default, fully idle I/O series are omitted and missing intervals
therefore mean zero activity; disk capacity is emitted every 60 seconds.

### Key Principles

- Collectors MUST only collect and normalize measurements; they do not write storage directly.
- The collector-to-storage channel MUST remain bounded so slow storage creates backpressure.
- One writer task owns the SQLite connection to avoid write-lock contention.
- Rollup, retention deletion, and WAL checkpoint work MUST stay on that same writer task.
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
│   │   ├── disk.rs        # Disk capacity and I/O collector
│   │   ├── mod.rs         # Collector contract, context, and shared error type
│   │   ├── network.rs     # Interface transfer and error collector
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

The persistent models are timestamped raw metric samples, time-bucket rollups, and maintenance
watermarks. Each collection cycle produces one transient batch containing the results from every
successful collector. Disk and network samples use the optional `resource` field for a mount point
or interface name.

```text
CollectionCycle 1──* MetricSample
MetricSample    *──* MetricRollup (derived by series and time bucket; no foreign key)
```

The `CollectionCycle` boundary currently exists only as an in-memory `MetricBatch`; it is not a
database table. Device, Event, and Report remain planned diagnostic entities without schemas. See
[domain-models.md](domain-models.md) for status and field details.

## 6. API / Interface Structure

Simple Profiler has a CLI interface and no HTTP interface yet.

| Command | Purpose | Important options |
|---|---|---|
| `simple-profiler run` | Collect CPU, memory, disk, and network metrics until interrupted or a sample limit is reached | `--database`, `--interval-seconds`, `--samples` |
| `simple-profiler status` | Print schema, per-tier row counts/ranges, file sizes, watermarks, and maintenance result | `--database` |

The global `--config <PATH>` option loads TOML settings. The local dashboard and report commands
are TBD — not yet designed.

## 7. Background Jobs & Scheduled Tasks

| Task | Trigger | Current behavior |
|---|---|---|
| Collection cycle | Tokio interval; five seconds by default | Create one shared timestamp/elapsed context, run all collectors, and combine successful results |
| System metric collection | Each cycle | Refresh total/per-core CPU and memory metrics |
| Disk metric collection | Each cycle | Emit capacity every 60 seconds by default; emit non-idle I/O delta/rate after warm-up |
| Network metric collection | Each cycle | Emit non-idle per-interface transfer, packet, error, and rate metrics after warm-up |
| SQLite writer | Each received batch | Insert the complete batch inside one transaction |
| Storage maintenance | Checked by the writer after inserts; every 60 seconds by default | Roll up at most 60 complete buckets per tier, apply retention in 10,000-row chunks, then request a passive WAL checkpoint |
| Graceful shutdown | Ctrl-C or `--samples` limit | Close the channel, drain queued batches, then stop |

Maintenance waits 30 seconds before considering a bucket complete. Raw deletion cannot pass the
one-minute watermark, and one-minute deletion cannot pass the 15-minute watermark. Maintenance
errors are logged without stopping later collection. Event evaluation and scheduled report
generation are TBD — not yet designed.

## 8. External Service Integrations

The implemented runtime has no external service or network integration. `sysinfo` is the in-process
platform abstraction for CPU, memory, disk, and network data, and bundled SQLite is compiled with
the application. Future NVIDIA, AMD, and Apple GPU adapter choices are TBD — not yet designed.

## 9. Database / Data Stores

The application owns one client-embedded SQLite database. [`../src/storage.rs`](../src/storage.rs)
creates and upgrades the schema transactionally using SQLite `user_version`. Schema version 2
adds an integer millisecond timestamp to raw samples, backfills v1 rows, and creates rollup and
maintenance-state tables.

| Table | Purpose | Indexes |
|---|---|---|
| `metric_samples` | One normalized raw measurement with RFC 3339 and millisecond timestamps | `collected_at_ms`; `(metric_name, collected_at_ms)`; `(metric_name, resource, collected_at_ms)` |
| `metric_rollups` | One aggregate per resolution, bucket, collector, normalized resource, and metric name | Composite primary key plus `(resolution_seconds, bucket_start_ms)` |
| `maintenance_state` | Integer/text watermarks and the last maintenance result | Primary key on `key` |

SQLite runs in WAL mode with a five-second busy timeout. Metric batches are written inside a
transaction. Rollups store sample count, minimum, maximum, sum, weighted average, and last value.
One-minute rollups are derived from raw samples; 15-minute rollups combine one-minute statistics
without averaging averages. Upserts make completed buckets safe to recompute. Defaults retain raw
rows for 24 hours, one-minute rows for 30 days, and 15-minute rows for 365 days. Cleanup leaves
reusable SQLite pages and does not run automatic `VACUUM`. Because this is an embedded local store,
the server-database observation module does not apply.

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

Secrets are not currently required. The default database path is `data/simple-profiler.sqlite3`,
the collection interval is five seconds, and the bounded channel capacity is 128 batches.
`[sampling]` controls the 60-second disk-capacity interval and idle-I/O suppression. `[retention]`
controls the 24-hour/30-day/365-day tiers, 60-second maintenance cadence, 30-second late-arrival
grace, 10,000-row delete chunks, and 60-bucket processing limit.
