# Simple Profiler — Progress

> **Last session:** 2026-07-15 · commit `cc03d5f` · tests: passing

## Now (WIP = 1)

Design and implement raw-data retention plus time-bucket rollups without blocking collection.

## Feature list

| # | Behavior | Verify with | State |
|---|---|---|---|
| 1 | Record CPU and memory measurements in SQLite and inspect stored range | `cargo test` | passing |
| 2 | Record disk capacity/I/O and network transfer measurements | `cargo test` | passing |
| 3 | Enforce raw-data retention and create time rollups | `cargo test` | active |
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

## Blockers

None.

## Next steps

1. Decide raw, one-minute, and long-term retention windows.
2. Design rollup schema, aggregate fields, and idempotent bucket processing.
3. Add cleanup scheduling that does not block the collector-to-storage pipeline.
4. Define report queries against raw and rolled-up time ranges.

## Decision log

- 2026-07-15 — Identify disk and network samples with an optional `resource` field and migrate
  existing SQLite rows without data loss.
- 2026-07-15 — Share one timestamp/elapsed context across collectors, combine successful results,
  and suppress rate metrics during the first warm-up cycle.
- 2026-07-15 — Use Rust after reviewing Rust and Go; rationale is recorded in
  [docs/project-overview.md](docs/project-overview.md) §2.
- 2026-07-15 — Use a modular monolith with bounded batches and one SQLite writer; rationale is
  recorded in [docs/project-overview.md](docs/project-overview.md) §3.
- 2026-07-15 — Develop macOS first while keeping collectors adapter-oriented for later Linux and
  Windows support.
- 2026-07-15 — Track the agent harness and dashboard design modules in the repository.
