# Simple Profiler — Domain Models & Business Mechanisms

> **Type:** Reference
> **Audience:** Developers, AI assistants, and code reviewers
> **Last updated:** 2026-07-15
>
> Implemented and planned metric, storage, and diagnostic concepts.

---

## 1. Model Details

### Metric

**Status: implemented.** [`../src/model.rs`](../src/model.rs) defines the normalized unit of
measurement passed between collectors and storage.

| Field | Meaning |
|---|---|
| `collected_at` | UTC timestamp assigned once per collector cycle |
| `collector` | Adapter that produced the value: `system`, `disk`, or `network` |
| `resource` | Optional mount point or network interface identity |
| `name` | Hierarchical metric name such as `cpu.total.usage` |
| `value` | Numeric measurement represented as `f64` |
| `unit` | Explicit unit such as `percent` or `bytes` |

### MetricBatch

**Status: implemented, transient.** A `Vec<Metric>` containing all successful collector results
from one cycle. It is the message sent through the bounded Tokio channel. The batch is a
transaction boundary but has no database identifier or table.

### MetricSample

**Status: implemented in SQLite.** One row of `metric_samples`, persisted by
[`../src/storage.rs`](../src/storage.rs).

| Column | SQLite type | Notes |
|---|---|---|
| `id` | `INTEGER` | Auto-incremented primary key |
| `collected_at` | `TEXT` | UTC RFC 3339 timestamp |
| `collector` | `TEXT` | Producing collector name |
| `resource` | `TEXT`, nullable | Mount point or interface name; existing CPU/memory rows remain `NULL` |
| `metric_name` | `TEXT` | Normalized hierarchical name |
| `value` | `REAL` | Numeric sample |
| `unit` | `TEXT` | Unit required for interpretation |

### StorageStatus

**Status: implemented, read model.** Contains total metric row count and optional oldest/newest
timestamps returned by the `status` command. It is not persisted separately.

### Device

**Status: planned — no schema yet.** Will identify CPUs, GPUs, disks, and network interfaces so
metrics remain attributable across hardware changes. Identity fields and replacement rules are
TBD — not yet designed.

### Event

**Status: planned — no schema yet.** Will represent a bounded abnormal period with severity,
trigger evidence, and related metrics. Thresholds and lifecycle states are TBD — not yet designed.

### Rollup

**Status: planned — no schema yet.** Will retain aggregate values for longer periods after raw
samples expire. Windows and aggregate fields are TBD — not yet designed.

### Report

**Status: planned — no schema yet.** Will record requested diagnostic time ranges and generated
artifacts. Format, status lifecycle, and privacy metadata are TBD — not yet designed.

## 2. Entity Relationships

Solid relationships are implemented; dotted descriptions are planned only.

```text
CollectionCycle (transient) 1──* MetricSample

Device 1··* MetricSample     (planned — no schema yet)
Event  *··* MetricSample     (planned — no schema yet)
Rollup *··1 MetricName       (planned — no schema yet)
Report 1··* Event            (planned — no schema yet)
```

The current schema does not persist a collection-cycle ID, device ID, event ID, or report ID.
`resource` is descriptive text and is not a foreign key to the planned Device entity.

## 3. Collection Flow

1. [`../src/runtime.rs`](../src/runtime.rs) waits for the Tokio interval.
2. It creates one `CollectionContext` containing a shared UTC timestamp and optional elapsed time.
3. `SystemCollector`, `DiskCollector`, and `NetworkCollector` run sequentially with that context.
4. Collector failures are logged while successful results are combined into one batch.
5. A non-empty batch is sent through the bounded channel and committed as one transaction.
6. If a sample limit was supplied and reached, collection stops after completing that cycle.

The interval uses Tokio's `Skip` missed-tick behavior, so delayed collection does not create a
burst of catch-up cycles. Disk and network delta/rate metrics are omitted during the first cycle,
when no reliable elapsed duration exists; disk capacity metrics are still emitted.

## 4. Storage Flow

1. `spawn_writer` starts one blocking task that owns a `rusqlite::Connection`.
2. Opening storage creates parent directories, enables WAL, sets a busy timeout, and runs the
   transactional schema upgrade.
3. Each received `MetricBatch` is inserted inside one SQLite transaction.
4. Shutdown drops the sender, drains queued batches, commits them, and joins the writer task.

Schema version 1 adds nullable `resource` to the original unversioned table, creates its composite
index, and preserves existing rows. A database newer than the supported version is rejected rather
than opened with an incompatible writer.

The runtime treats an unexpectedly stopped writer as an application error. Retry and quarantine
behavior for failed batches is TBD — not yet designed.

## 5. Metric Naming

The implemented names are:

| Pattern | Unit | Cardinality |
|---|---|---|
| `cpu.total.usage` | `percent` | One per cycle |
| `cpu.core.<index>.usage` | `percent` | One per logical CPU per cycle |
| `memory.total` | `bytes` | One per cycle |
| `memory.used` | `bytes` | One per cycle |
| `memory.available` | `bytes` | One per cycle |
| `memory.usage` | `percent` | One per cycle |
| `disk.space.total` | `bytes` | One per mount point per cycle |
| `disk.space.available` | `bytes` | One per mount point per cycle |
| `disk.space.used` | `bytes` | One per mount point per cycle |
| `disk.space.usage` | `percent` | One per mount point per cycle |
| `disk.io.<read\|write>.delta` | `bytes` | One per mount point after warm-up |
| `disk.io.<read\|write>.rate` | `bytes_per_second` | One per mount point after warm-up |
| `network.<receive\|transmit>.delta` | `bytes` | One per interface after warm-up |
| `network.<receive\|transmit>.rate` | `bytes_per_second` | One per interface after warm-up |
| `network.<receive\|transmit>.packets.delta` | `packets` | One per interface after warm-up |
| `network.<receive\|transmit>.errors.delta` | `errors` | One per interface after warm-up |

New collectors SHOULD follow dot-separated, stable names and MUST attach an explicit unit. A
registry for validating names and units is TBD — not yet designed.

## 6. Failure Behavior

- Invalid interval or channel capacity is rejected before the runtime starts.
- A collector error is logged, while successful collectors in the same cycle continue to storage.
- An all-failed cycle does not write an empty batch but still counts toward `--samples`.
- A closed storage channel stops the run with an error.
- A writer panic or database error is returned when the writer task is joined.
- SQLite transaction failure does not partially commit the affected batch.

Per-collector health state, missing-sample markers, and failure events are planned — no schema yet.

## 7. Deprecated Components

N/A — the initial version has no deprecated domain components.

## 8. Developer Tooling / Maintenance Scripts

No domain maintenance scripts exist. Schema inspection currently uses the `status` command for a
row count and time range. Startup applies built-in schema upgrades; compaction and repair commands
are TBD — not yet designed.
