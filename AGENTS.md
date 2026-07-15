# Simple Profiler — Agent Guide

Simple Profiler owns local collection and diagnostic storage of host resource metrics.

## Hard constraints

- MUST run `cargo test` successfully before declaring work done (source: README verification gate).
- MUST run `cargo fmt --check` and Clippy before declaring Rust changes done (source: `rustfmt.toml` and `clippy.toml`).
- MUST keep SQLite writes behind the single writer task (source: `docs/project-overview.md` §3).
- MUST commit raw samples, anomaly transitions, evidence, and restored state in one writer
  transaction (source: `docs/project-overview.md` §3).
- MUST keep collector-to-storage channels bounded (source: `docs/project-overview.md` §3).
- MUST update `PROGRESS.md` at clock-out (source: selected agent-harness workflow).
- MUST NOT install, unload, uninstall, or purge the macOS LaunchAgent without explicit user
  approval (source: `docs/project-overview.md` §10).
- MUST keep the dashboard loopback-only, token-scoped, read-only, and query-bounded (source:
  `docs/project-overview.md` §3).

## Read before you work

Read the matching doc before non-trivial work. Small fixes and running checks can skip this.

| Task | Read first |
|---|---|
| Architecture, runtime flow, directory layout, integrations | [docs/project-overview.md](docs/project-overview.md) |
| Metrics, storage entities, anomaly or report behavior | [docs/domain-models.md](docs/domain-models.md) |
| Style, lint rules, errors, and layering conventions | [docs/coding-style.md](docs/coding-style.md) |
| Building or restyling the dashboard UI | [DESIGN.md](DESIGN.md) |

## Commands

```bash
cargo build
cargo build --release
cargo run -- run
cargo run -- events list
cargo run -- events show 1
cargo run -- processes top --sort cpu
cargo run -- report generate --last 1h
cargo run -- dashboard
target/release/simple-profiler service status
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

## Session routine

- **Clock-in:** read [PROGRESS.md](PROGRESS.md), then inspect `git log -3` and `git status`,
  run `cargo test`, and pick up the single active item (WIP = 1).
- **Clock-out:** run the verification commands, update `PROGRESS.md` with state, commit,
  and test status, remove stale artifacts, then commit. A session is complete only after
  verification passes and the repository is clean.

## Conventions

- Collector implementations live under `src/collector/` and return normalized metric or process
  snapshots to the runtime; they do not write SQLite directly.
- Blocking SQLite work stays in the storage writer's blocking task.
- Anomaly rules and state transitions live in `src/anomaly.rs`; their SQLite representation and
  evidence queries live in `src/anomaly_storage.rs`.
- Process snapshot persistence, ranking queries, retention, and anomaly attribution live in
  `src/process_storage.rs` and remain owned by the single writer transaction.
- Report time-range parsing and HTML rendering live in `src/report.rs`; bounded, read-only report
  queries and retention-tier selection live in `src/report_storage.rs`.
- Dashboard loopback serving, session authorization, security headers, and API handlers live in
  `src/dashboard.rs`; embedded HTML/CSS/JavaScript live under `src/dashboard/` and follow
  `DESIGN.md`.
- Implemented and planned behavior MUST be labeled separately in documentation.

## Docs maintenance

When modifying a file under `docs/`, update its `> **Last updated:** YYYY-MM-DD` field to
today's date. Requirement keywords (MUST, SHOULD, MAY) follow RFC 2119.
