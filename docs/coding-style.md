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
`#[serde(default)]`, and validates runtime, sampling, and retention settings before work begins.
Durations and batch limits that must be positive are rejected at zero; retention tiers are checked
from shortest to longest. New runtime settings SHOULD follow that typed model instead of reading
environment values inside collectors.

### Typed library errors and contextual application errors

[`../src/collector/mod.rs`](../src/collector/mod.rs) uses `thiserror` for the collector boundary.
[`../src/main.rs`](../src/main.rs), [`../src/runtime.rs`](../src/runtime.rs), and
[`../src/storage.rs`](../src/storage.rs) use `anyhow::Context` when propagating application-level
failures. New collector failure categories SHOULD be added to `CollectorError`; operational context
SHOULD be attached at the caller boundary.

### Blocking storage isolation

[`../src/storage.rs`](../src/storage.rs) owns SQLite inside `tokio::task::spawn_blocking`. Async
runtime code MUST NOT execute rusqlite statements directly on a Tokio worker thread. Inserts,
rollups, retention cleanup, maintenance watermarks, and WAL checkpoints MUST preserve this single
writer boundary.

## 4. Team Conventions (Not Enforced by the Linter)

### 4.1 Separate collection from persistence

Collectors MUST return normalized `MetricBatch` values and MUST NOT open SQLite connections.

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
- Storage access belongs in `src/storage.rs` and MUST preserve the single-writer boundary.
- Retention cleanup MUST NOT pass the watermark proving that the downstream rollup tier completed.
- Maintenance work SHOULD use bounded bucket and row batches; automatic `VACUUM` MUST NOT run in
  the collection path.
- The channel between collection and storage MUST remain bounded.
- CLI parsing and override precedence belong in `src/main.rs`; reusable behavior belongs in the
  library modules.
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
