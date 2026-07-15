# Simple Profiler

A local-first Rust service that continuously records system metrics for later diagnosis.

## What it does

- Collects CPU, memory, Apple GPU, disk capacity/I/O, and network transfer metrics at configurable
  intervals.
- On Apple silicon, records GPU device/renderer/tiler utilization plus in-use and allocated GPU
  memory through non-privileged structured `ioreg` output. Capability status explicitly reports
  unavailable power, temperature, or unified-memory-total fields instead of storing zeroes.
- Suppresses idle disk/network I/O by default and samples disk capacity every 60 seconds to limit
  storage growth.
- Combines successful collectors into one cycle batch and sends it through a bounded channel.
- Stores timestamped, resource-aware samples in a local SQLite database using one WAL writer.
- Detects sustained CPU, memory, and per-mount disk-space anomalies with configurable warning,
  critical, recovery, duration, sample-count, and data-gap rules.
- Persists anomaly state across restarts and preserves bounded prelude, trigger, escalation, peak,
  periodic, and recovery evidence independently from raw-sample retention.
- Samples a bounded union of the top CPU, resident-memory, disk-read/write, network-receive/
  transmit, and optional GPU processes every 15 seconds, without collecting command lines,
  environments, or working directories.
- Uses non-privileged macOS `nettop` process counters for network attribution and `sysinfo` deltas
  for process disk I/O. Provider failures degrade only the affected dimension.
- Attaches matching bounded top-process evidence to CPU, memory, disk-I/O, network, and GPU anomaly
  events; disk-space capacity events show host-wide writer context instead of false filesystem
  ownership.
- Retains process raw samples for 24 hours, process one-minute rollups for 7 days, and process
  15-minute rollups for 90 days. System metric tiers remain 24 hours, 30 days, and 365 days;
  closed anomaly events are retained for 365 days.
- Reports schema version, row counts and time ranges by resolution, database/WAL size, rollup
  watermarks, maintenance status, open anomaly counts, and collector capabilities from the command
  line.
- Generates a self-contained local HTML diagnostic report for relative or explicit time ranges,
  automatically selecting raw, one-minute, or 15-minute data and including anomaly/process
  evidence without external scripts, fonts, or network requests.
- Serves an on-demand, read-only dashboard on a random loopback port with a per-launch session
  token, embedded assets, bounded live queries, time-range controls, charts, anomaly evidence, and
  sortable top-process summaries.
- Installs and supervises itself as a per-user macOS LaunchAgent, with graceful shutdown,
  single-instance protection, service health output, and bounded log rotation.

NVIDIA/AMD adapters remain planned. Per-process Apple GPU attribution is implemented as an
optional, separately installed root helper that writes a bounded root-owned snapshot; it is never
enabled or installed by the normal per-user service command.

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
cargo run -- events list --database /tmp/simple-profiler.sqlite3
cargo run -- processes top --database /tmp/simple-profiler.sqlite3 --sort cpu
```

Load settings from the tracked example configuration:

```bash
cargo run -- --config config/default.toml run
```

### Inspect anomaly events

List recent events, restrict the list to events that are still open, or inspect one event and its
preserved evidence:

```bash
cargo run -- events list
cargo run -- events list --open --limit 50
cargo run -- events show 1
```

`status` and `service status` also report the current warning/critical event counts. The tracked
[`config/default.toml`](config/default.toml) contains the default sustained CPU, memory-pressure,
and per-mount disk-space rules. Rule state is stored in SQLite, so a normal process restart does
not reset an in-progress detection or open event.

### Inspect GPU capabilities

GPU collection is enabled by default every 15 seconds on macOS. The same status commands show each
field as `available`, `degraded`, or `unavailable` together with its provider and reason. Configure
the adapter under `[gpu]` in [`config/default.toml`](config/default.toml); `provider = "auto"`
selects the non-privileged Apple `ioreg` adapter on macOS.

### Inspect resource-heavy processes

Show the latest ranking for any collected process dimension:

```bash
cargo run -- processes top --sort cpu --limit 10
cargo run -- processes top --sort memory --limit 10
cargo run -- processes top --sort disk-write --limit 10
cargo run -- processes top --sort network-receive --limit 10
```

Process identity uses PID plus process start time, so PID reuse does not merge unrelated
processes. Raw process snapshots default to 24-hour retention and rollups keep bounded trends for
90 days. Matching event evidence is copied into the event record and remains available after raw
snapshots expire. Executable paths are disabled by default; command lines, environment variables,
and working directories are never collected.

### Optional per-process Apple GPU helper

The normal profiler remains a user LaunchAgent. The separately built
`simple-profiler-gpu-helper` is a one-shot root utility intended for a root LaunchDaemon: it reads
structured `powermetrics` output and atomically replaces
`/var/run/simple-profiler/process-gpu.json`. The user process accepts that file only when it is
root-owned, not group/world writable, at most 1 MiB, and fresh. Build it with:

```bash
cargo build --release --bin simple-profiler-gpu-helper
```

[`config/com.simple-profiler.gpu-helper.plist.example`](config/com.simple-profiler.gpu-helper.plist.example)
is the LaunchDaemon template. Installing the binary/plist and loading the LaunchDaemon changes
system-wide privileged state and is intentionally not automated; do it only after explicit
approval. After installation, set
`gpu_snapshot_path = "/var/run/simple-profiler/process-gpu.json"` under `[process]`.

### Generate a diagnostic report

Generate a local report for the last hour, or choose an explicit RFC 3339 time range:

```bash
cargo run -- report generate --last 1h
cargo run -- report generate \
  --from 2026-07-15T08:00:00+08:00 \
  --to 2026-07-15T12:00:00+08:00 \
  --output /tmp/simple-profiler-report.html
```

`--last` accepts minutes, hours, or days such as `30m`, `6h`, and `7d`. The maximum range is 365
days. Reports default to `~/Documents/SimpleProfiler Reports/`; add `--open` to open the completed
file on macOS. The generated HTML is self-contained and can be viewed offline.

### Explore the local dashboard

Start the read-only dashboard on an available loopback port and open it in the default browser:

```bash
cargo run -- dashboard --open
```

Without `--open`, the command prints its session URL. The dashboard process remains in the
foreground until Ctrl-C or SIGTERM, while the installed background collector continues normally.
It never listens beyond `127.0.0.1`, and each launch uses a new unguessable URL token. Presets cover
15 minutes through 30 days; custom ranges support up to 365 days. The time navigator moves the
selected window across retained history with a slider, Earlier/Later controls, or direct horizontal
dragging on any chart. Focused charts also accept Left/Right/Home/End keys, and Live returns to the
latest preset with auto-refresh enabled. Hovering a chart shows the selected timestamp plus the
system average, minimum, maximum, and top matching processes. CPU, memory, disk-I/O, network, and
GPU charts overlay the top three retained process series with ranked colors and line patterns.
Memory tooltips show percentage and bytes; disk-space capacity uses a separate host-wide writer
activity lane because percent-used and bytes-per-second are different units.

### Run in the background on macOS

Build an optimized binary, then explicitly install and start the per-user LaunchAgent:

```bash
cargo build --release
target/release/simple-profiler service install
```

The install command copies the executable, creates a private default configuration when none
exists, writes `~/Library/LaunchAgents/com.simple-profiler.agent.plist`, installs a managed
`~/.local/bin/simple-profiler` launcher that automatically selects the service configuration, and
starts the service. It refuses to overwrite an unmanaged file at that command path. `~/.local/bin`
must be present in the shell's `PATH`. Installation changes the current user's macOS service state
and SHOULD only be run intentionally.

Inspect or manage it with:

```bash
target/release/simple-profiler service status
target/release/simple-profiler service stop
target/release/simple-profiler service start
target/release/simple-profiler service restart
target/release/simple-profiler service uninstall
```

Normal uninstall removes the managed command launcher while preserving configuration, metrics,
and logs. A user-owned launcher is never removed. The destructive
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
src/           CLI, collectors, dashboard assets/server, runtime coordination, models, and storage
Cargo.toml     Rust package and dependency manifest
PROGRESS.md    Cross-session implementation state
DESIGN.md      Dashboard design contract
```

## Documentation

| Doc | What it covers |
|---|---|
| [docs/project-overview.md](docs/project-overview.md) | Architecture, directory map, interfaces, storage, and deployment |
| [docs/domain-models.md](docs/domain-models.md) | Metric, anomaly-event, evidence, and storage mechanisms |
| [docs/coding-style.md](docs/coding-style.md) | Rust formatting, linting, and project conventions |
| [DESIGN.md](DESIGN.md) | Dashboard design tokens, responsive behavior, and visual rules |
