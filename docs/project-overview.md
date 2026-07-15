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
memory, disk, network, and bounded top-process measurements, writes raw data to SQLite, and
maintains one-minute and 15-minute metric retention tiers. It evaluates sustained CPU, memory, and
per-mount disk-space rules, persists metric and related-process evidence across restarts, and
exposes event and process inspection commands. On macOS
it can install itself as a per-user LaunchAgent and expose service lifecycle/health commands. GPU,
reports, and a local dashboard are planned.

### 1.2 Relationship with Other Systems

The current program is standalone. It reads host information through `sysinfo`, writes a local
SQLite file, and makes no network calls at runtime. Future GPU collectors will use platform- or
vendor-specific adapters, but their exact APIs are not yet designed.

### 1.3 Deprecated / Retired or Not-Yet-Enabled Features

- **Not yet enabled:** GPU, temperature, and power collectors.
- **Not yet enabled:** diagnostic report generation and the dashboard.
- **Not yet enabled:** Linux systemd, Windows Service, system-wide macOS LaunchDaemon, signed
  installer, and automatic updates.
- No deprecated features exist in the initial version.

## 2. Tech Stack

| Component | Choice | Evidence / rationale |
|---|---|---|
| Language | Rust 1.92.0, edition 2024 | Selected by the user for predictable resource use and low-level system access; pinned as the MSRV in [`../clippy.toml`](../clippy.toml) |
| Async runtime | Tokio 1.52.3 | Timed collection, bounded channels, shutdown signals, and the blocking storage task |
| Host metrics | sysinfo 0.38.4 | Cross-platform CPU, memory, disk, and network access; this is the newest locked release compatible with Rust 1.92.0 |
| Local datastore | SQLite through rusqlite 0.39.0 | A local embedded store fits a single-host profiler; rusqlite supports an explicit single-writer design |
| CLI | Clap 4.6.1 | Defines the implemented `run`, `status`, `events`, `processes`, and `service` commands |
| Configuration | TOML through toml 1.1.3 and Serde | Human-readable local configuration with typed validation |
| Logs | tracing and tracing-subscriber | Structured runtime lifecycle and collection messages |
| macOS process control | launchd/launchctl plus libc 0.2.186 | Per-user supervision, effective-user identity, and advisory process locking |
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
                         ├──► NetworkCollector
                         └──► ProcessCollector (15-second cadence)
                                   │
                                   ▼
                    combined successful CollectionBatch
                                   │
                                   ▼
                         bounded mpsc channel
                                   │
                                   ▼
                blocking SQLite WAL writer/maintainer
                         │              │
                         ├── raw rows   ├── anomaly engine/state/evidence
                         ├── process snapshots and event attribution
                         │              └── 1m / 15m rollups
```

On macOS, `launchd` starts the installed executable with its private configuration. Before opening
the collector channel, `run` acquires an advisory lock beside the selected database so a terminal
process and LaunchAgent cannot maintain the same database concurrently. `SIGINT` and `SIGTERM`
share the same shutdown path: stop collection, close the channel, drain queued batches, and join
the writer. Runtime logs can use a size-limited file writer with numbered retained files.

The CLI assembles configuration and chooses an operation. `run` starts all collectors and the
storage writer; `status` opens the same SQLite database and summarizes retention plus open anomaly
counts; `events list/show` query recent event summaries and preserved metric/process evidence;
`processes top` queries the latest CPU or memory ranking. Each cycle supplies
one UTC timestamp and elapsed monotonic duration to every collector. The writer restores anomaly
state at startup and evaluates incoming raw batches before commit. Raw rows, event transitions,
evidence, and the next state commit in one transaction; the in-memory engine advances only after
that commit succeeds. One unavailable collector does not discard the others. Disk and network rate
metrics and process CPU usage warm up before rate-based output. By default, fully idle I/O series
are omitted and missing intervals
therefore mean zero activity; disk capacity is emitted every 60 seconds.

### Key Principles

- Collectors MUST only collect and normalize measurements; they do not write storage directly.
- The collector-to-storage channel MUST remain bounded so slow storage creates backpressure.
- One writer task owns the SQLite connection to avoid write-lock contention.
- Raw metrics, anomaly transitions/evidence/state, rollups, retention deletion, and WAL checkpoint
  work MUST stay on that same writer task.
- Only one profiler process may own a database's instance lock at a time.
- Blocking database work runs outside Tokio's asynchronous worker threads.
- A collector failure SHOULD be isolated and logged without corrupting already stored data.
- Implemented and planned capabilities MUST be labeled separately.
- Installing or removing a LaunchAgent changes user service state and MUST require explicit user
  intent; normal uninstall MUST preserve configuration, metrics, and logs.

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
│   │   ├── process.rs     # Bounded top CPU and memory process collector
│   │   └── system.rs      # CPU and memory collector
│   ├── anomaly.rs         # Sustained-threshold state machine
│   ├── anomaly_storage.rs # Event/state/evidence persistence and queries
│   ├── config.rs          # TOML model, defaults, and validation
│   ├── instance.rs        # Per-database process lock
│   ├── lib.rs             # Library module exports
│   ├── logging.rs         # Console or size-rotated file logging
│   ├── main.rs            # Clap CLI and command dispatch
│   ├── model.rs           # Normalized metric/process batch types
│   ├── process_storage.rs # Process snapshots, retention, queries, and event attribution
│   ├── runtime.rs         # Timed collection and graceful shutdown
│   ├── service.rs         # macOS LaunchAgent files and lifecycle management
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

The persistent models are timestamped raw metric/process samples, time-bucket metric rollups,
maintenance watermarks, anomaly rule states, anomaly events, and bounded event evidence. Each
collection cycle produces one transient batch containing the results from every successful
collector. Disk and
network samples use the optional `resource` field for a mount point or interface name; anomaly
state uses `(rule_id, resource)` as its identity.

```text
CollectionCycle 1──* MetricSample
CollectionCycle 1──* ProcessSample
MetricSample    *──* MetricRollup (derived by series and time bucket; no foreign key)
AnomalyRuleState 0..1──0..1 AnomalyEvent
AnomalyEvent     1──* AnomalyEventEvidence
AnomalyEvent     1──* AnomalyEventProcessEvidence (CPU/memory events only)
```

The `CollectionCycle` boundary currently exists only as an in-memory `CollectionBatch`; it is not
a database table. Device and Report remain planned diagnostic entities without schemas. See
[domain-models.md](domain-models.md) for event lifecycle and field details.

## 6. API / Interface Structure

Simple Profiler has a CLI interface and no HTTP interface yet.

| Command | Purpose | Important options |
|---|---|---|
| `simple-profiler run` | Collect CPU, memory, disk, and network metrics until interrupted or a sample limit is reached | `--database`, `--interval-seconds`, `--samples` |
| `simple-profiler status` | Print schema, per-tier row counts/ranges, file sizes, watermarks, maintenance result, and anomaly counts | `--database` |
| `simple-profiler events list` | List recent events newest-first, optionally only those still open | `--open`, `--limit` (1–1,000), `--database` |
| `simple-profiler events show <ID>` | Show thresholds, time range, peak/last values, sample/gap counts, and ordered metric/process evidence | `--database` |
| `simple-profiler processes top` | Show the latest bounded process ranking by CPU or resident memory | `--sort cpu\|memory`, `--limit` (1–100), `--database` |
| `simple-profiler service install` | Copy the current executable, preserve/create service configuration, write the plist, load it, and start collection | none |
| `simple-profiler service start\|stop\|restart` | Manage the installed per-user LaunchAgent; stop waits for graceful SIGTERM shutdown | none |
| `simple-profiler service status` | Show installed/loaded/running state, PID, paths, latest sample, maintenance result, and open anomaly counts | none |
| `simple-profiler service uninstall` | Unload the agent and remove its plist/binary while preserving user data | `--purge` also removes configuration, metrics, and logs |

The global `--config <PATH>` option loads TOML settings. The local dashboard and report commands
are TBD — not yet designed.

## 7. Background Jobs & Scheduled Tasks

| Task | Trigger | Current behavior |
|---|---|---|
| Collection cycle | Tokio interval; five seconds by default | Create one shared timestamp/elapsed context, run all collectors, and combine successful results |
| System metric collection | Each cycle | Refresh total/per-core CPU and memory metrics |
| Disk metric collection | Each cycle | Emit capacity every 60 seconds by default; emit non-idle I/O delta/rate after warm-up |
| Network metric collection | Each cycle | Emit non-idle per-interface transfer, packet, error, and rate metrics after warm-up |
| Process collection | Every 15 seconds by default | After CPU warm-up, retain the union of top 10 CPU and top 10 resident-memory processes |
| SQLite writer/anomaly evaluation | Each received batch | Evaluate matching raw metrics and atomically commit metric/process samples, event transitions, evidence, and restart state |
| Storage maintenance | Checked by the writer after inserts; every 60 seconds by default | Roll up at most 60 complete buckets per tier, apply metric and closed-event retention in bounded chunks, then request a passive WAL checkpoint |
| macOS LaunchAgent supervision | Login load and abnormal exit | Start the installed `run` command and restart after unsuccessful exit, throttled to at most one launch per 10 seconds |
| Log rotation | Before a write would exceed the configured size | Rename numbered files and retain five rotated 10 MiB files plus the current file by default |
| Graceful shutdown | Ctrl-C, SIGTERM, or `--samples` limit | Close the channel, drain queued batches, then stop |

Maintenance waits 30 seconds before considering a bucket complete. Raw deletion cannot pass the
one-minute watermark, and one-minute deletion cannot pass the 15-minute watermark. Maintenance
errors are logged without stopping later collection. Pending detection is reset by a configured
data gap; an open event records the gap and stays open rather than treating missing data as
recovery. Scheduled report generation is TBD — not yet designed.

## 8. External Service Integrations

The implemented runtime has no external network integration. `sysinfo` is the in-process platform
abstraction for CPU, memory, disk, and network data, and bundled SQLite is compiled with the
application. On macOS, service commands invoke the local `/bin/launchctl` process and read its
status output. Anomaly detection reads only local metric batches and configuration; it makes no
notification or network call. Future NVIDIA, AMD, and Apple GPU adapter choices are TBD — not yet
designed.

## 9. Database / Data Stores

The application owns one client-embedded SQLite database. [`../src/storage.rs`](../src/storage.rs)
creates and upgrades the schema transactionally using SQLite `user_version`. Schema version 2
adds an integer millisecond timestamp to raw samples, backfills v1 rows, and creates rollup and
maintenance-state tables. Schema version 3 adds anomaly event, restart-state, and evidence tables.
Schema version 4 adds process snapshots and per-event process evidence without removing older
metric or anomaly rows.

| Table | Purpose | Indexes |
|---|---|---|
| `metric_samples` | One normalized raw measurement with RFC 3339 and millisecond timestamps | `collected_at_ms`; `(metric_name, collected_at_ms)`; `(metric_name, resource, collected_at_ms)` |
| `metric_rollups` | One aggregate per resolution, bucket, collector, normalized resource, and metric name | Composite primary key plus `(resolution_seconds, bucket_start_ms)` |
| `maintenance_state` | Integer/text watermarks and the last maintenance result | Primary key on `key` |
| `anomaly_events` | Open/closed warning or critical periods, thresholds, peak/last values, counts, and timestamps | `(status, started_at_ms)`; `(severity, started_at_ms)` |
| `anomaly_states` | Restart-safe normal/pending/open/recovering state per rule and resource | Primary key on `(rule_id, resource)` |
| `anomaly_event_evidence` | Bounded prelude, trigger, escalation, peak, periodic, and recovery samples | `(event_id, collected_at_ms)` |
| `process_samples` | Bounded snapshots keyed by timestamp and PID/start-time identity, with CPU/memory ranks | `collected_at_ms`; `(pid, process_start_time_seconds, collected_at_ms)` |
| `anomaly_event_process_evidence` | Copied top-process snapshots for CPU/memory event checkpoints | `(event_id, collected_at_ms)` plus a uniqueness key per process snapshot |

SQLite runs in WAL mode with a five-second busy timeout. Collection batches are written inside a
transaction together with anomaly state and evidence. Rollups store sample count, minimum,
maximum, sum, weighted average, and last value. Process snapshots use PID plus process start time
as identity and store name, optional parent PID, CPU percentage, resident bytes, and CPU/memory
ranks. Command line, environment, and working directory are not collected; executable path is
opt-in and disabled by default.
One-minute rollups are derived from raw samples; 15-minute rollups combine one-minute statistics
without averaging averages. Upserts make completed buckets safe to recompute. Defaults retain raw
rows for 24 hours, process snapshots for 24 hours, one-minute rows for 30 days, 15-minute rows for
365 days, and closed events for 365 days. Event evidence remains queryable after its source raw
samples expire. Closed-event cleanup deletes metric/process evidence and events in 1,000-event
chunks by default; open events are retained. Cleanup
leaves reusable SQLite pages and does not run automatic `VACUUM`. Because this is an embedded local
store, the server-database observation module does not apply.

## 10. Environments & Deployment

### Environments

Local development/direct execution and per-user macOS LaunchAgent execution exist. macOS 26 on
Apple silicon is the verified development platform. Linux and Windows collection remain
architectural goals but their service managers are not implemented.

### Deployment Pipeline

There is no CI, signed/notarized package, automatic updater, Linux systemd unit, or Windows Service
definition yet. A local optimized binary can install itself as
`~/Library/LaunchAgents/com.simple-profiler.agent.plist`. Installation copies the executable and
creates these per-user locations:

```text
~/Library/Application Support/SimpleProfiler/bin/simple-profiler
~/Library/Application Support/SimpleProfiler/config.toml
~/Library/Application Support/SimpleProfiler/data/simple-profiler.sqlite3
~/Library/Logs/SimpleProfiler/
```

Reinstall replaces the executable and plist atomically but preserves an existing configuration.
Normal uninstall preserves configuration, metrics, and logs; `--purge` is the explicit destructive
variant. The agent starts on login, restarts only after unsuccessful exit, allows 20 seconds for
shutdown, and runs as the current user rather than as a system LaunchDaemon.

### Configuration Hierarchy

Configuration precedence is:

1. Command-line overrides for database path and interval
2. A TOML file passed with `--config`
3. Built-in defaults matching [`../config/default.toml`](../config/default.toml)

Secrets are not currently required. The default database path is `data/simple-profiler.sqlite3`,
the collection interval is five seconds, and the bounded channel capacity is 128 batches.
`[sampling]` controls the 60-second disk-capacity interval and idle-I/O suppression. `[retention]`
controls the 24-hour/30-day/365-day tiers, 60-second maintenance cadence, 30-second late-arrival
grace, 10,000-row delete chunks, and 60-bucket processing limit. `[logging]` optionally selects a
file and defaults to 10 MiB per file with five retained files. `[anomaly]` enables detection,
controls 365-day closed-event retention, five-minute prelude capture, 60-second periodic evidence,
and 1,000-event cleanup chunks. Its rule list supplies metric names, warning/critical/recovery
thresholds, duration/sample requirements, and maximum data gaps. `[process]` defaults to a
15-second cadence, top 10 CPU plus top 10 memory union, 24-hour raw retention, top five rows per
event checkpoint, and a 500-row per-event cap; executable paths are opt-in. The generated
LaunchAgent config
uses an absolute database path and log path under the per-user directories above.
