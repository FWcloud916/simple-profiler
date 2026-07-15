# Simple Profiler — Progress

> **Last session:** 2026-07-15 · commit `3b50d19` · tests: passing (47)

## Now (WIP = 1)

No feature is active. Top-process snapshots and CPU/memory event attribution are implemented;
local HTML diagnostic reports are the next product feature.

## Feature list

| # | Behavior | Verify with | State |
|---|---|---|---|
| 1 | Record CPU and memory measurements in SQLite and inspect stored range | `cargo test` | passing |
| 2 | Record disk capacity/I/O and network transfer measurements | `cargo test` | passing |
| 3 | Enforce raw-data retention and create time rollups | `cargo test` | passing |
| 4 | Detect sustained resource anomalies and preserve event evidence | `cargo test` | passing |
| 5 | Record bounded top-process snapshots and attribute CPU/memory events | `cargo test` | passing |
| 6 | Generate a local HTML diagnostic report for a selected time range | `cargo test` | not_started |
| 7 | Explore metrics and events in a local dashboard | `cargo test` | not_started |
| 8 | Collect GPU measurements through capability-aware platform adapters | `cargo test` | not_started |
| 9 | Install and supervise the profiler as an operating-system background service | `cargo test` | passing |

## Done

- Rust package, configuration model, CLI, and module boundaries created.
- CPU and memory Collector implemented with total and per-core CPU measurements.
- Bounded channel and single blocking SQLite writer implemented.
- `run` and `status` commands exercised against a temporary database.
- Unit tests, rustfmt, and Clippy verification pass.
- Core project documentation and selected documentation modules created.
- Shared collection context and partial-failure isolation implemented for multiple collectors.
- Disk capacity/I/O and per-interface network transfer, packet, and error metrics implemented.
- Resource-aware metric storage and unversioned-to-v1 SQLite migration implemented.
- Idle disk/network I/O suppression and configurable disk-capacity sampling implemented.
- SQLite schema v2 migration preserves v1 rows and adds millisecond timestamps, rollups, and
  maintenance state.
- Idempotent one-minute and weighted 15-minute rollups implemented with safe watermarks, bounded
  retention deletion, passive WAL checkpointing, and no automatic `VACUUM`.
- `status` now reports every retention tier, storage/WAL sizes, reusable pages, watermarks, and the
  last maintenance result.
- The retention phase passes 19 tests plus rustfmt and strict Clippy verification; a three-cycle
  live collection/status smoke test also passed.
- Per-user macOS LaunchAgent install/start/stop/restart/status/uninstall commands implemented.
- Installed binaries, private configuration, database, logs, and plist use standard user-library
  paths; reinstall preserves configuration and normal uninstall preserves all user data.
- SIGTERM drains queued SQLite batches before exit, while an advisory lock rejects a second
  collector targeting the same database.
- File logs rotate at a configurable size with a bounded number of retained files; routine
  collection-cycle entries use debug level.
- The service phase passes 26 tests, `plutil`, rustfmt, strict Clippy, release build, SIGTERM, and
  competing-process integration checks. After explicit approval, the user installed the
  LaunchAgent and live status confirmed that background samples continued advancing.
- Configurable CPU, memory, and per-mount disk-space anomaly rules implemented with warning,
  critical escalation, hysteresis recovery, duration/sample gates, and data-gap handling.
- SQLite schema v3 persists anomaly events, restart-safe rule state, and bounded prelude/trigger/
  escalation/peak/periodic/recovery evidence in the same transaction as raw samples.
- `events list`, `events show`, `status`, and `service status` expose event history, evidence, and
  open warning/critical counts.
- Closed events default to 365-day retention with bounded cleanup; evidence remains after raw
  samples expire, and open/pending state resumes across process restarts.
- The anomaly phase passes 35 tests, rustfmt, strict Clippy, v2-to-v3 migration, restart/recovery,
  evidence-retention, and command-line smoke checks.
- A privacy-bounded process collector samples the union of top CPU and resident-memory processes
  every 15 seconds by default, identifying PID reuse with PID plus process start time.
- SQLite schema v4 stores 24 hours of raw process snapshots and copies bounded top-process evidence
  into CPU/memory anomaly events; disk-space events intentionally receive no process attribution.
- `processes top --sort cpu|memory`, `events show`, `status`, and `service status` expose process
  rankings, event attribution, and process-snapshot health without collecting command lines,
  environments, or working directories; executable paths remain opt-in.
- The process-attribution phase passes 44 tests, rustfmt, strict Clippy, release build, v3-to-v4
  migration, PID-reuse/retention/cardinality/privacy boundaries, and live CPU/memory CLI smoke checks.
- The installed macOS LaunchAgent now runs the schema-v4 process-attribution release; live status
  confirms system and process timestamps continue advancing after restart.
- LaunchAgent upgrade now waits for asynchronous `bootout` removal before `bootstrap`, preventing
  the observed launchd `Operation already in progress` race on consecutive upgrades.
- `service install` now creates a managed `~/.local/bin/simple-profiler` launcher that automatically
  uses the private service configuration; upgrades and uninstalls refuse to modify user-owned
  files at the same path. Live zsh, process-query, and service-status checks pass.

## Blockers

None.

## Next steps

1. Define the local HTML report request, time-range selection, and output contract.
2. Define report queries that choose raw, one-minute, or 15-minute data by requested range.
3. Design report sections that combine resource summaries, anomaly timelines, and evidence.
4. Inspect the first naturally occurring CPU or memory anomaly to validate its preserved process
   evidence against the report requirements.

## Decision log

- 2026-07-15 — Manage a shell-quoted launcher under `~/.local/bin` during service install so the
  short CLI command targets the background database; never replace or remove an unmanaged path.
- 2026-07-15 — Wait up to five seconds for launchd to report an unloaded agent before bootstrapping
  its replacement; `bootout` completion is asynchronous even after launchctl exits successfully.
- 2026-07-15 — Sample the union of top 10 CPU and top 10 resident-memory processes every 15
  seconds; keep raw snapshots for 24 hours and identify process instances with PID plus start time.
- 2026-07-15 — Copy top five fresh process rows into CPU/memory event checkpoints with a 500-row
  per-event cap; do not attribute disk-space events from unrelated CPU/memory dimensions.
- 2026-07-15 — Do not collect process command lines, environments, or working directories;
  executable path collection is opt-in and disabled by default.
- 2026-07-15 — Evaluate anomaly rules from incoming raw batches inside the single SQLite writer;
  raw samples, event changes, evidence, and restart state commit atomically.
- 2026-07-15 — Use `normal → pending → open → recovering → normal` with warning-to-critical
  escalation, hysteresis recovery, duration plus sample-count gates, and explicit data-gap rules.
- 2026-07-15 — Start with sustained total CPU, memory usage, and per-mount disk-space rules; defer
  per-core CPU, disk I/O, and network anomaly rules until their cardinality/sparse semantics are
  designed.
- 2026-07-15 — Preserve bounded prelude, trigger, escalation, latest peak, periodic, and recovery
  evidence independently of raw retention; retain closed events for 365 days by default.
- 2026-07-15 — Identify disk and network samples with an optional `resource` field and migrate
  existing SQLite rows without data loss.
- 2026-07-15 — Retain raw samples for 24 hours, one-minute rollups for 30 days, and 15-minute
  rollups for 365 days by default.
- 2026-07-15 — Store count/min/max/sum/average/last for every rollup; combine child counts and sums
  so 15-minute averages remain weighted, while delta consumers use `sum_value`.
- 2026-07-15 — Run rollup and cleanup transactions on the single writer with a 30-second grace,
  60-bucket limit, 10,000-row delete chunks, downstream watermarks, and no automatic `VACUUM`.
- 2026-07-15 — Suppress fully idle disk/network I/O and sample disk capacity every 60 seconds by
  default; missing sparse delta intervals represent zero activity.
- 2026-07-15 — Implement macOS background execution as the per-user
  `com.simple-profiler.agent` LaunchAgent; system-wide LaunchDaemon, Linux, and Windows service
  management remain future work.
- 2026-07-15 — Install under `~/Library/Application Support/SimpleProfiler`, keep logs under
  `~/Library/Logs/SimpleProfiler`, preserve data on normal uninstall, and require `--purge` for
  destructive removal.
- 2026-07-15 — Handle SIGTERM through the normal drain path and use a per-database advisory lock so
  a manual process cannot race the LaunchAgent's writer/maintenance state.
- 2026-07-15 — Share one timestamp/elapsed context across collectors, combine successful results,
  and suppress rate metrics during the first warm-up cycle.
- 2026-07-15 — Use Rust after reviewing Rust and Go; rationale is recorded in
  [docs/project-overview.md](docs/project-overview.md) §2.
- 2026-07-15 — Use a modular monolith with bounded batches and one SQLite writer; rationale is
  recorded in [docs/project-overview.md](docs/project-overview.md) §3.
- 2026-07-15 — Develop macOS first while keeping collectors adapter-oriented for later Linux and
  Windows support.
- 2026-07-15 — Track the agent harness and dashboard design modules in the repository.
