# Simple Profiler — Coding Style Guide

> **Type:** Reference / How-to
> **Audience:** Developers, AI assistants, and code reviewers
> **Last updated:** 2026-07-15
>
> This document records only conventions backed by the current Rust configuration, source, or
> explicitly selected architecture.
>
> Configuration sources: [`../rustfmt.toml`](../rustfmt.toml),
> [`../clippy.toml`](../clippy.toml), and [`../Cargo.toml`](../Cargo.toml).
>
> **Terminology:** MUST, SHOULD, and MAY follow RFC 2119.

---

## 1. Linter Overview

| Tool | Role | Tracked configuration |
|---|---|---|
| rustfmt | Deterministic Rust formatting | Edition 2024 and maximum width 100 in `rustfmt.toml` |
| Clippy | Rust lint diagnostics | Minimum supported Rust version 1.92.0 in `clippy.toml` |
| rustc | Compilation and type checking | Edition 2024 in `Cargo.toml` |

There is no CI configuration yet. The repository's pre-merge gate is the local command set in §6.

## 2. Linter Rules Summary

- Source MUST be formatted using Rust edition 2024 rules.
- Formatted lines target the configured maximum width of 100.
- Clippy analysis MUST use MSRV 1.92.0.
- The project gate promotes every Clippy warning to an error with `-D warnings`.
- No additional allow/deny groups or per-lint thresholds are configured.

## 3. Project-Specific Code Examples

### Typed configuration validation

[`../src/config.rs`](../src/config.rs) deserializes into `AppConfig`, applies defaults with
`#[serde(default)]`, and validates runtime, sampling, logging, anomaly, process, and retention
settings before work begins. Durations, log limits, and batch limits that must be positive are
rejected at zero; retention tiers are checked from shortest to longest. Anomaly rule validation
also enforces unique non-empty IDs, finite ordered high-water thresholds, positive sample limits,
and a positive maximum gap. Process validation bounds ranking cardinality and requires event
top-N/cap consistency. New runtime settings SHOULD follow that typed model instead of reading
environment values inside collectors.

### Typed library errors and contextual application errors

[`../src/collector/mod.rs`](../src/collector/mod.rs) uses `thiserror` for the collector boundary.
[`../src/main.rs`](../src/main.rs), [`../src/runtime.rs`](../src/runtime.rs), and
[`../src/storage.rs`](../src/storage.rs) use `anyhow::Context` when propagating application-level
failures. [`../src/service.rs`](../src/service.rs) also includes failed path or `launchctl` stderr
context at the operating-system boundary. New collector failure categories SHOULD be added to
`CollectorError`; operational context SHOULD be attached at the caller boundary.

### Blocking storage isolation

[`../src/storage.rs`](../src/storage.rs) owns SQLite inside `tokio::task::spawn_blocking`, while
[`../src/anomaly_storage.rs`](../src/anomaly_storage.rs) contains event/state/evidence statements
and [`../src/process_storage.rs`](../src/process_storage.rs) contains process persistence,
attribution, and ranking queries called by that owner. Async runtime code MUST NOT execute
rusqlite statements directly on a Tokio worker thread. Raw inserts, anomaly transitions, evidence,
restored state, rollups, retention
cleanup, maintenance watermarks, and WAL checkpoints MUST preserve this single-writer boundary.

## 4. Team Conventions (Not Enforced by the Linter)

### 4.1 Separate collection from persistence

Collectors MUST return normalized measurements or process snapshots and MUST NOT open SQLite
connections.

```rust
// Good: collector returns data to the runtime.
let batch = collector.collect().await?;
sender.send(batch).await?;

// Bad: collector writes directly to storage.
collector.collect_and_insert(&connection).await?;
```

### 4.2 Make units explicit

Every `Metric` MUST include a unit. Metric names describe the measurement, while `unit` determines
how the numeric value is interpreted.

```rust
// Good
Metric::new(now, "system", "memory.used", bytes, "bytes");

// Bad: consumers cannot safely interpret this value.
Metric::new(now, "system", "memory.used", bytes, "");
```

### 4.3 Label planned behavior

Documentation MUST distinguish implemented behavior from `planned — no schema yet` or
`TBD — not yet designed`. Planned types MUST NOT be described as available runtime behavior.

## 5. Architecture Conventions

- Collector implementations belong under `src/collector/` and implement `Collector`.
- Runtime coordination belongs in `src/runtime.rs`; collectors SHOULD NOT own schedules.
- General storage ownership belongs in `src/storage.rs`; anomaly persistence and query details
  belong in `src/anomaly_storage.rs`; process persistence, attribution, retention, and ranking
  queries belong in `src/process_storage.rs`. All MUST preserve the single-writer boundary.
- Anomaly state transitions belong in `src/anomaly.rs` and SHOULD remain testable without SQLite.
- A batch's raw metric/process rows, anomaly event changes, metric/process evidence, and next rule
  states MUST commit in one transaction; the live engine MUST advance only after that commit.
- Retention cleanup MUST NOT pass the watermark proving that the downstream rollup tier completed.
- Maintenance work SHOULD use bounded bucket and row batches; automatic `VACUUM` MUST NOT run in
  the collection path.
- The channel between collection and storage MUST remain bounded.
- CLI parsing and override precedence belong in `src/main.rs`; reusable behavior belongs in the
  library modules.
- Report range/output behavior and HTML/SVG rendering belong in `src/report.rs`; SQLite tier
  selection, aggregation, and evidence queries belong in `src/report_storage.rs`. Report queries
  MUST be read-only and bounded, and persisted names/resources MUST be HTML-escaped before output.
- Generated reports MUST remain self-contained without CDN, remote fonts, or network requests.
  Output SHOULD use a sibling temporary file plus atomic rename, and chart series SHOULD remain
  bounded to approximately 1,200 points.
- macOS installation paths, plist rendering, and `launchctl` calls belong in `src/service.rs`.
- Per-database locking belongs in `src/instance.rs`; every `unsafe` libc call MUST carry a local
  safety explanation.
- Log-file creation and bounded rotation belong in `src/logging.rs`; normal collection-cycle logs
  SHOULD remain at debug level to avoid needless file growth.
- Installer writes SHOULD use temporary files plus rename. Normal uninstall MUST preserve user
  configuration, metrics, and logs; destructive purge MUST require an explicit flag and user
  approval before execution. Managed CLI launchers MUST be identified before replacement or
  removal; an unrelated user-owned path MUST be preserved.
- See [project-overview.md](project-overview.md) §3 for the full runtime flow.

## 6. Running the Linter (Pre-merge)

Run all verification commands from the repository root:

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

There is no changed-files-only command or CI gate yet.

## 7. References

- [The Rust Style Guide](https://doc.rust-lang.org/style-guide/)
- [rustfmt configuration](../rustfmt.toml)
- [Clippy configuration](../clippy.toml)
- [Architecture overview](project-overview.md)
