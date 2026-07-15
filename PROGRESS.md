# Simple Profiler — Progress

> **Last session:** 2026-07-15 · commit `4ade288` · tests: passing

## Now (WIP = 1)

No feature is active. Tiered retention is complete; sustained anomaly detection is the next
planned feature.

## Feature list

| # | Behavior | Verify with | State |
|---|---|---|---|
| 1 | Record CPU and memory measurements in SQLite and inspect stored range | `cargo test` | passing |
| 2 | Record disk capacity/I/O and network transfer measurements | `cargo test` | passing |
| 3 | Enforce raw-data retention and create time rollups | `cargo test` | passing |
| 4 | Detect sustained resource anomalies and preserve event evidence | `cargo test` | not_started |
| 5 | Generate a local HTML diagnostic report for a selected time range | `cargo test` | not_started |
| 6 | Explore metrics and events in a local dashboard | `cargo test` | not_started |
| 7 | Collect GPU measurements through capability-aware platform adapters | `cargo test` | not_started |
| 8 | Install and supervise the profiler as an operating-system background service | `cargo test` | not_started |

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

## Blockers

None.

## Next steps

1. Define anomaly rules, duration thresholds, and the event evidence model.
2. Decide whether anomaly evaluation reads raw samples, rollups, or both.
3. Define report queries that select the appropriate raw or rolled-up resolution.
4. Design background-service installation and supervision for supported operating systems.

## Decision log

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
- 2026-07-15 — Share one timestamp/elapsed context across collectors, combine successful results,
  and suppress rate metrics during the first warm-up cycle.
- 2026-07-15 — Use Rust after reviewing Rust and Go; rationale is recorded in
  [docs/project-overview.md](docs/project-overview.md) §2.
- 2026-07-15 — Use a modular monolith with bounded batches and one SQLite writer; rationale is
  recorded in [docs/project-overview.md](docs/project-overview.md) §3.
- 2026-07-15 — Develop macOS first while keeping collectors adapter-oriented for later Linux and
  Windows support.
- 2026-07-15 — Track the agent harness and dashboard design modules in the repository.
