# Simple Profiler — Progress

> **Last session:** 2026-07-15 · commit `186700f` · tests: passing (69)

## Now (WIP = 1)

No feature is active. Chart value inspection and ranked top-process trend lines are implemented;
upgrading the installed binary to expose them through the managed launcher requires explicit user
approval.

## Feature list

| # | Behavior | Verify with | State |
|---|---|---|---|
| 1 | Record CPU and memory measurements in SQLite and inspect stored range | `cargo test` | passing |
| 2 | Record disk capacity/I/O and network transfer measurements | `cargo test` | passing |
| 3 | Enforce raw-data retention and create time rollups | `cargo test` | passing |
| 4 | Detect sustained resource anomalies and preserve event evidence | `cargo test` | passing |
| 5 | Record bounded top-process snapshots and attribute CPU/memory events | `cargo test` | passing |
| 6 | Generate a local HTML diagnostic report for a selected time range | `cargo test` | passing |
| 7 | Explore metrics and events in a local dashboard | `cargo test` | passing |
| 8 | Collect GPU measurements through capability-aware platform adapters | `cargo test` | passing |
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
- `report generate` now accepts relative durations or paired RFC 3339 timestamps, defaults to the
  last hour, enforces a 365-day maximum, and atomically writes a self-contained local HTML report.
- Report queries automatically prefer raw, one-minute, or 15-minute retained metrics by requested
  range, fall back to available tiers, cap chart series at approximately 1,200 points, and combine
  resource trends with anomaly/process evidence without changing schema version 4.
- The report phase passes 54 tests, rustfmt, strict Clippy, release build, invalid/empty-range
  checks, and live 1-hour/24-hour/7-day database smoke tests. The live 7-day report selected
  15-minute data, contained 24 series and 28 process summaries, and was about 17 KiB with no
  external URL or script dependency.
- `dashboard` now serves an on-demand local interface from an ephemeral `127.0.0.1` port using a
  random 128-bit path token, exact Host validation, four-query admission control, strict security
  headers, and compiled-in HTML/CSS/JavaScript assets with no external dependency.
- Dashboard requests open short-lived schema-v4 SQLite read-only connections on blocking tasks,
  reuse the report range/resolution/point limits, load event detail on demand, and cannot migrate
  or modify the collector database. Ctrl-C/SIGTERM stops only the dashboard.
- The dashboard implements system-theme light/dark layouts, 15m–30d presets plus custom ranges,
  min/average/max charts with explicit gaps, live storage health, anomaly evidence drill-down,
  sortable process summaries, empty/error states, and optional 15-second refresh.
- The dashboard phase passes 59 tests, rustfmt, strict Clippy, release build, JavaScript syntax,
  invalid range and hostile Host checks, security-header checks, and live 1h/24h/7d API smoke
  tests. Live responses selected raw/1m/15m tiers in 5–26 ms, stayed below 1 MiB, and background
  sample timestamps continued advancing during dashboard queries.
- SQLite schema v5 stores current collector capability state and commits it in the same writer
  transaction as the metric/process batch, anomaly transitions, evidence, and restart state.
- A non-privileged Apple GPU adapter now parses structured `ioreg` property-list output every 15
  seconds by default, recording device/renderer/tiler utilization plus in-use/allocated memory.
- GPU fields independently report available, degraded, or unavailable state. Root-only
  `powermetrics` is deliberately not used; GPU power, temperature, unified-memory total, and
  per-process GPU attribution remain unavailable instead of being synthesized as zero.
- Status, service status, reports, and the dashboard now expose GPU metrics/capabilities. The GPU
  phase passes 65 tests, rustfmt, strict Clippy, release build, schema-v4-to-v5 migration, M4 live
  collection, report rendering, and loopback dashboard API smoke checks.
- After explicit user approval, the installed LaunchAgent was upgraded to schema v5 and restarted
  as PID 77600. Live status confirmed five Apple GPU metrics share fresh timestamps, capability
  state is available for supported fields, ordinary/process samples continue advancing, and the
  service stderr log has no new errors.
- The dashboard now moves a fixed-duration window through retained history with a global slider,
  Earlier/Later/Live controls, direct mouse/touch chart dragging, and Left/Right/Home/End keyboard
  navigation. Historical movement disables auto-refresh until Live is selected; slider queries are
  debounced and concurrent refreshes collapse to the newest queued range.
- Timeline navigation reuses the existing bounded read-only `from`/`to` snapshot API without schema
  changes or dependencies. The phase passes 66 tests, JavaScriptCore syntax, rustfmt, strict Clippy,
  release build, embedded-interaction regression checks, and a live explicit-range API smoke test.
- After explicit user approval, the installed LaunchAgent was upgraded to the timeline release and
  restarted as PID 38158. The managed dashboard exposed the slider, Earlier/Later/Live controls,
  and retained-history label; system/process sample timestamps continued advancing after the
  temporary dashboard verification process stopped.
- Dashboard chart hover/focus now shows the nearest timestamp with system average/min/max. CPU and
  memory charts overlay the top three retained process series using dedicated colors, line patterns,
  rank labels, and tooltips; memory values include both system percentage and bytes.
- Process chart queries reuse schema-v5 raw snapshots, preserve PID-plus-start-time identity, select
  the top-three union per dimension, and cap each line near 360 points. The phase passes 69 tests,
  JavaScriptCore syntax, rustfmt, strict Clippy, release build, and live one-hour API verification;
  five unique series returned with at most 185 points in the observed window.

## Blockers

None.

## Next steps

1. After explicit approval, upgrade the installed binary to the chart-inspection release and verify
   the managed dashboard serves tooltip and process-series assets.
2. Design NVIDIA and AMD adapters without weakening field-level capability semantics.
3. Decide whether GPU anomaly rules are useful after observing real retained workloads.
4. Inspect the first naturally occurring CPU or memory anomaly to validate its preserved process
   evidence in reports and the dashboard.

## Decision log

- 2026-07-15 — Overlay the union of the top three CPU and top three memory process identities on
  system charts using bounded raw-process buckets. Convert memory bytes with a retained system-total
  sample, and distinguish ranks with color, line pattern, text label, and tooltip instead of color
  alone; keep schema version 5 unchanged.

- 2026-07-15 — Navigate dashboard history as a fixed-duration time window over retained coverage;
  support slider/buttons, direct pointer dragging, and keyboard controls while reusing bounded
  explicit-range queries and disabling auto-refresh during historical exploration.

- 2026-07-15 — Collect Apple GPU device/renderer/tiler utilization and in-use/allocated memory from
  structured non-privileged `ioreg` output every 15 seconds, with a two-second timeout and
  exponential retry backoff capped at five minutes.
- 2026-07-15 — Persist current field-level collector capabilities in schema v5; missing/invalid
  fields are unavailable or degraded, never synthetic zeroes, and capability upserts share the
  metric batch transaction.
- 2026-07-15 — Do not invoke root-only `powermetrics`; leave GPU power, temperature, unified-memory
  total, and per-process attribution explicitly unavailable until a stable non-privileged source
  exists.

- 2026-07-15 — Serve the dashboard only on `127.0.0.1` with an ephemeral port, random 128-bit
  session path, exact Host validation, no mutation routes/CORS, strict response headers, and at
  most four concurrent blocking queries.
- 2026-07-15 — Keep dashboard assets in the Rust executable with system light/dark themes and no
  Node runtime, CDN, remote fonts, telemetry, or persistent HTTP listener in the LaunchAgent.
- 2026-07-15 — Open one short-lived SQLite read-only connection per API request and reject
  non-current schemas rather than migrating; reuse report ranges/tiers/bounds and fetch full event
  evidence only on selection.
- 2026-07-15 — Generate reports as transient, self-contained HTML artifacts with embedded CSS/SVG,
  no JavaScript or network assets, full escaping of stored labels, and atomic file replacement.
- 2026-07-15 — Prefer raw metrics through two hours, one-minute rollups through 24 hours, and
  15-minute rollups for longer ranges; fall back to retained tiers and cap each chart series near
  1,200 points.
- 2026-07-15 — Keep reports read-only and schema-free; include at most 200 overlapping events and
  the union of top 20 CPU/memory process summaries for ranges no longer than 365 days.
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
