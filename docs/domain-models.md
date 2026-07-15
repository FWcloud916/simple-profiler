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
| `collected_at_ms` | `INTEGER` | UTC Unix timestamp in milliseconds for range queries and buckets |
| `collector` | `TEXT` | Producing collector name |
| `resource` | `TEXT`, nullable | Mount point or interface name; existing CPU/memory rows remain `NULL` |
| `metric_name` | `TEXT` | Normalized hierarchical name |
| `value` | `REAL` | Numeric sample |
| `unit` | `TEXT` | Unit required for interpretation |

### StorageStatus

**Status: implemented, read model.** Contains schema version; row count and optional oldest/newest
time for raw, one-minute, and 15-minute data; database/WAL/reusable-page sizes; both rollup
watermarks; and the last maintenance time/result. It is not persisted as one object.

### MetricRollup

**Status: implemented in SQLite.** One `metric_rollups` row describes a metric series within a
one-minute or 15-minute bucket.

| Column | Meaning |
|---|---|
| `bucket_start_ms` | UTC bucket boundary in Unix milliseconds |
| `resolution_seconds` | `60` or `900` |
| `collector`, `resource`, `metric_name`, `unit` | Series identity; missing raw resources normalize to an empty string |
| `sample_count` | Number of raw observations represented |
| `min_value`, `max_value` | Observed extremes |
| `sum_value` | Sum of represented observations; the primary interpretation for delta metrics |
| `average_value` | `sum_value / sample_count`; 15-minute values use combined counts and sums |
| `last_value` | Latest value represented by the bucket |

The primary key is resolution, bucket, collector, normalized resource, and metric name. Upserts
replace a recomputed bucket, so processing the same completed range again is idempotent.

### MaintenanceState

**Status: implemented in SQLite.** `maintenance_state` stores integer or text values by key. It
currently carries the next one-minute and 15-minute bucket watermarks plus the last maintenance
time and result.

### Device

**Status: planned — no schema yet.** Will identify CPUs, GPUs, disks, and network interfaces so
metrics remain attributable across hardware changes. Identity fields and replacement rules are
TBD — not yet designed.

### Event

**Status: planned — no schema yet.** Will represent a bounded abnormal period with severity,
trigger evidence, and related metrics. Thresholds and lifecycle states are TBD — not yet designed.

### Report

**Status: planned — no schema yet.** Will record requested diagnostic time ranges and generated
artifacts. Format, status lifecycle, and privacy metadata are TBD — not yet designed.

## 2. Entity Relationships

Solid relationships are implemented; dotted descriptions are planned only.

```text
CollectionCycle (transient) 1──* MetricSample
MetricSample *──* MetricRollup   (derived by matching series and time bucket; no foreign key)
MaintenanceState ── stores ──► one-minute and 15-minute watermark keys

Device 1··* MetricSample     (planned — no schema yet)
Event  *··* MetricSample     (planned — no schema yet)
Report 1··* Event            (planned — no schema yet)
```

The schema does not persist a collection-cycle ID, device ID, event ID, report ID, or foreign keys
between samples and rollups. `resource` is descriptive text and is not a foreign key to the
planned Device entity.

## 3. Collection Flow

1. [`../src/runtime.rs`](../src/runtime.rs) validates configuration and acquires the advisory lock
   beside the selected database; a second collector for that database is rejected.
2. The runtime waits for the Tokio interval and creates one `CollectionContext` containing a shared
   UTC timestamp and optional elapsed time.
3. `SystemCollector`, `DiskCollector`, and `NetworkCollector` run sequentially with that context.
4. Collector failures are logged while successful results are combined into one batch.
5. A non-empty batch is sent through the bounded channel and committed as one transaction.
6. A sample limit, Ctrl-C, or SIGTERM stops collection, closes the channel, drains queued batches,
   and releases the process lock after the writer joins.

The interval uses Tokio's `Skip` missed-tick behavior, so delayed collection does not create a
burst of catch-up cycles. Disk and network delta/rate metrics are omitted during the first cycle,
when no reliable elapsed duration exists. Disk capacity is emitted on the first cycle and then at
its configured interval (60 seconds by default). With idle suppression enabled, a disk with zero
read/write bytes or a network interface whose counters are all zero emits no I/O metrics; rollup
consumers interpret those missing delta intervals as zero activity.

## 4. Storage Flow

1. `spawn_writer` starts one blocking task that owns a `rusqlite::Connection`.
2. Opening storage creates parent directories, enables WAL, sets a busy timeout, and runs the
   transactional schema upgrade.
3. Each received `MetricBatch` is inserted inside one SQLite transaction.
4. At the configured cadence, the same writer transaction recomputes up to the configured number
   of complete one-minute buckets, then derives complete 15-minute buckets from them.
5. That transaction advances watermarks and deletes expired rows in bounded chunks, never deleting
   data that its downstream tier has not processed.
6. After commit, storage requests a passive WAL checkpoint. Automatic `VACUUM` is not performed.
7. Shutdown drops the sender, drains queued batches, commits them, and joins the writer task.

Schema version 2 adds `collected_at_ms`, backfills v1 timestamps, and creates `metric_rollups` and
`maintenance_state`. A database newer than the supported version is rejected rather than opened
with an incompatible writer. Rollup buckets wait for the configured late-arrival grace period.
One-minute aggregation reads raw rows in timestamp/ID order; 15-minute aggregation combines counts,
sums, extremes, and the latest child value so averages remain weighted.

The runtime treats an unexpectedly stopped writer as an application error. Maintenance errors are
logged and do not stop collection. Retry and quarantine behavior for failed inserts is TBD — not
yet designed.

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
| `disk.space.total` | `bytes` | One per mount point at the capacity interval |
| `disk.space.available` | `bytes` | One per mount point at the capacity interval |
| `disk.space.used` | `bytes` | One per mount point at the capacity interval |
| `disk.space.usage` | `percent` | One per mount point at the capacity interval |
| `disk.io.<read\|write>.delta` | `bytes` | One per active mount point after warm-up |
| `disk.io.<read\|write>.rate` | `bytes_per_second` | One per active mount point after warm-up |
| `network.<receive\|transmit>.delta` | `bytes` | One per active interface after warm-up |
| `network.<receive\|transmit>.rate` | `bytes_per_second` | One per active interface after warm-up |
| `network.<receive\|transmit>.packets.delta` | `packets` | One per active interface after warm-up |
| `network.<receive\|transmit>.errors.delta` | `errors` | One per active interface after warm-up |

New collectors SHOULD follow dot-separated, stable names and MUST attach an explicit unit. A
registry for validating names and units is TBD — not yet designed.

## 6. Failure Behavior

- Invalid sampling/retention values, interval, channel capacity, or retention-tier ordering are
  rejected before the runtime starts.
- A collector error is logged, while successful collectors in the same cycle continue to storage.
- An all-failed cycle does not write an empty batch but still counts toward `--samples`.
- A closed storage channel stops the run with an error.
- A second process targeting the same database exits before it creates a channel or writer.
- A writer panic or database error is returned when the writer task is joined.
- SQLite transaction failure does not partially commit the affected batch.
- Rollup rows, cleanup, and watermarks commit in one transaction; a failed maintenance pass does
  not expose a partially advanced watermark.
- `service stop` sends SIGTERM and fails if the process does not report stopped within 20 seconds.
- `launchctl` failures include stderr context instead of being reported as successful lifecycle
  changes.

Per-collector health state, missing-sample markers, and failure events are planned — no schema yet.

## 7. Deprecated Components

N/A — the initial version has no deprecated domain components.

## 8. Developer Tooling / Maintenance Scripts

No separate domain maintenance scripts exist. The writer performs rollup and retention maintenance
internally. The `status` command shows raw/rollup ranges, storage sizes, watermarks, and the last
maintenance result. On macOS, the `service` command group implements this lifecycle:

```text
uninstalled ── install ──► loaded/running
      ▲                         │
      └────── uninstall ◄───────┤
                                ├── stop ──► loaded/stopped ── start ──► loaded/running
                                └── restart ───────────────────────────► loaded/running
```

Install and upgrade write the executable and plist atomically and preserve an existing private
configuration. Status combines parsed `launchctl` state with the latest sample and maintenance
state. Normal uninstall removes only the service binary/plist; `--purge` also removes
configuration, metrics, and logs. Startup applies built-in schema upgrades; manual compaction and
repair commands are TBD — not yet designed.
