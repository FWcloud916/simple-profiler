# Simple Profiler — Progress

> **Last session:** 2026-07-15 · commit `4add7b7` · tests: passing

## Now (WIP = 1)

Add disk and network collectors using the existing bounded collection-to-storage pipeline.

## Feature list

| # | Behavior | Verify with | State |
|---|---|---|---|
| 1 | Record CPU and memory measurements in SQLite and inspect stored range | `cargo test` | passing |
| 2 | Record disk capacity/I/O and network transfer measurements | `cargo test` | active |
| 3 | Enforce raw-data retention and create time rollups | `cargo test` | not_started |
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

## Blockers

None.

## Next steps

1. Design stable disk and network metric names and units.
2. Add platform-capability handling so unavailable metrics do not stop other collectors.
3. Add collector tests and run the full verification gate.
4. Update domain and overview docs with the implemented metrics.

## Decision log

- 2026-07-15 — Use Rust after reviewing Rust and Go; rationale is recorded in
  [docs/project-overview.md](docs/project-overview.md) §2.
- 2026-07-15 — Use a modular monolith with bounded batches and one SQLite writer; rationale is
  recorded in [docs/project-overview.md](docs/project-overview.md) §3.
- 2026-07-15 — Develop macOS first while keeping collectors adapter-oriented for later Linux and
  Windows support.
- 2026-07-15 — Track the agent harness and dashboard design modules in the repository.

