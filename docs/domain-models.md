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
| `collector` | Adapter that produced the value: `system`, `disk`, `network`, or `gpu` |
| `resource` | Optional mount point, network interface, or GPU identity |
| `name` | Hierarchical metric name such as `cpu.total.usage` |
| `value` | Numeric measurement represented as `f64` |
| `unit` | Explicit unit such as `percent` or `bytes` |

### MetricBatch

**Status: implemented, transient.** A `Vec<Metric>` containing all successful collector results
from one cycle.

### ProcessSample and CollectionBatch

**Status: implemented.** A process sample records one bounded ranking entry: collection time, PID,
process start time, optional parent PID, process name, optional executable path, CPU percentage,
resident bytes, disk I/O deltas/rates, optional network deltas/rates, optional GPU time/usage, and
per-dimension ranks. PID plus start time is the stable identity used to
distinguish PID reuse. A `CollectionBatch` combines a `MetricBatch`, one process snapshot, and
collector capability updates and is the message sent through the bounded Tokio channel. The batch
is one transaction boundary but has no database identifier or table.

Process privacy is deliberate: command lines, environment variables, and working directories are
never collected. Executable path collection is opt-in and disabled by default.

### CollectorCapability

**Status: implemented in SQLite as `collector_capabilities`.** One row records the current state
of a named collector field for a resource. Its identity is `(collector, resource, capability)`;
the mutable fields are `available`, `degraded`, or `unavailable` state, provider name, optional
detail, and the last check time. Capability updates commit in the same transaction as the metric
batch that produced them. This is current state rather than time-series history.

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
time for metric and process raw, one-minute, and 15-minute data; database/WAL/reusable-page sizes; both rollup
watermarks; last maintenance time/result; open warning/critical counts; latest event time; and the
current collector capabilities. It is not persisted as one object.

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

### ProcessMetricRollup

**Status: implemented in schema v6.** `process_metric_rollups` normalizes CPU, memory, disk read,
disk write, network receive, network transmit, and optional GPU usage into one metric-name/value
shape keyed by resolution, bucket, PID, process start time, and metric. It stores count, minimum,
maximum, sum, weighted average, last value, and best observed rank. Raw process samples are kept 24
hours, one-minute rollups 7 days, and 15-minute rollups 90 days by default.

### MaintenanceState

**Status: implemented in SQLite.** `maintenance_state` stores integer or text values by key. It
currently carries the next one-minute and 15-minute bucket watermarks plus the last maintenance
time and result.

### Device

**Status: planned — no schema yet.** Will identify CPUs, GPUs, disks, and network interfaces so
metrics remain attributable across hardware changes. Identity fields and replacement rules are
TBD — not yet designed.

### Event

**Status: implemented in SQLite as `anomaly_events`.** An event is a sustained abnormal period for
one rule and normalized resource. It stores `open` or `closed` status; `warning` or `critical`
severity; collector, metric, resource, and unit identity; start, detection, peak, last-sample, and
optional end timestamps; configured thresholds; peak/last values; sample count; and data-gap
count. A warning event can escalate to critical but does not downgrade before it closes.

### AnomalyRuleState

**Status: implemented in SQLite as `anomaly_states`.** The primary key is `(rule_id, resource)`.
The stored phase is `normal`, `pending`, `open`, or `recovering`; optional warning/critical
severity, candidate timestamps/sample counts, active event ID, latest value/time, peak, evidence
time, and data-gap count allow evaluation to continue across normal process restarts.

### AnomalyEventEvidence

**Status: implemented in SQLite as `anomaly_event_evidence`.** Each row identifies an event,
timestamp, value, and evidence kind: `prelude`, `trigger`, `escalation`, `peak`, `periodic`, or
`recovery`. Prelude capture is limited to the newest 120 matching raw rows within the configured
window. Only the latest `peak` evidence row is retained, while periodic evidence is rate-limited.
Evidence remains available after the corresponding raw samples expire.

### AnomalyEventProcessEvidence

**Status: implemented in SQLite as `anomaly_event_process_evidence`.** CPU events copy top CPU
rows, memory events copy top memory rows, and disk-I/O/network/GPU events select their matching
rank for prelude, trigger, escalation, periodic, and recovery checkpoints. Peak-only metric
evidence does not add a process checkpoint. Disk-space capacity events are not claimed as direct
ownership; the dashboard instead shows host-wide process writer context. The default copies at
most five rows per checkpoint and 500 rows per event.
Copied evidence remains available after the 24-hour raw process snapshot retention expires.

### Report

**Status: implemented as a transient read model and local HTML artifact; no schema.** A report
contains its requested range, selected metric resolution, actual metric/process coverage, bounded
resource series, overlapping anomaly events with preserved evidence, and bounded process
summaries plus current collector capabilities. The generated file embeds its CSS and SVG,
performs no network requests, and states the process-data privacy boundary. Reports are not
registered in SQLite and have no status lifecycle.

### DashboardSnapshot

**Status: implemented as a transient JSON read model; no schema.** A snapshot shares report range,
resolution, bounded resource series, coverage, event summaries, and process summaries, but loads
full metric/process evidence only when a user selects one event. `StorageStatus` is fetched
separately so live storage health does not enlarge every chart response.

## 2. Entity Relationships

Persistent relationships are shown below. Report content is assembled by bounded read-only queries
and does not add foreign keys.

```text
CollectionCycle (transient) 1──* MetricSample
CollectionCycle (transient) 1──* ProcessSample
CollectionCycle (transient) 1──* CollectorCapability (current-state upsert)
MetricSample *──* MetricRollup   (derived by matching series and time bucket; no foreign key)
ProcessSample *──* ProcessMetricRollup (derived by process identity, dimension, and bucket)
MaintenanceState ── stores ──► one-minute and 15-minute watermark keys
AnomalyRuleState 0..1──0..1 Event
Event 1──* AnomalyEventEvidence
Event 1──* AnomalyEventProcessEvidence (matching attributable dimensions)
Report (transient) ── reads ──► metric/process raw or rollup rows and Event
DashboardSnapshot (transient) ── reads ──► metric/process raw or rollup rows and Event

Device 1··* MetricSample     (planned — no schema yet)
```

The schema does not persist a collection-cycle ID, device ID, report ID, or SQL foreign-key
constraints. `anomaly_states.event_id` and `anomaly_event_evidence.event_id` are application-owned
references to `anomaly_events.id`. `resource` is descriptive text and is not a foreign key to the
planned Device entity.

## 3. Collection Flow

1. [`../src/runtime.rs`](../src/runtime.rs) validates configuration and acquires the advisory lock
   beside the selected database; a second collector for that database is rejected.
2. The runtime waits for the Tokio interval and creates one `CollectionContext` containing a shared
   UTC timestamp and optional elapsed time.
3. `SystemCollector`, `DiskCollector`, and `NetworkCollector` run each metric cycle;
   `ProcessCollector` and `GpuCollector` run on independent 15-second default cadences.
4. Collector failures are logged while successful metric, process, and capability results are
   combined into one `CollectionBatch`.
5. A non-empty batch is sent through the bounded channel and committed with anomaly evaluation as
   one transaction.
6. A sample limit, Ctrl-C, or SIGTERM stops collection, closes the channel, drains queued batches,
   and releases the process lock after the writer joins.

The interval uses Tokio's `Skip` missed-tick behavior, so delayed collection does not create a
burst of catch-up cycles. Disk and network delta/rate metrics are omitted during the first cycle,
when no reliable elapsed duration exists. Disk capacity is emitted on the first cycle and then at
its configured interval (60 seconds by default). With idle suppression enabled, a disk with zero
read/write bytes or a network interface whose counters are all zero emits no I/O metrics; rollup
consumers interpret those missing delta intervals as zero activity.

Process CPU, network cumulative counters, and optional GPU cumulative time require a previous
refresh. Later snapshots rank all visible processes by CPU, memory, disk read/write, available
network receive/transmit, and available GPU usage, then retain their union with a default hard cap
of 40 rows. Disk deltas come from `sysinfo`; macOS process network totals come from one bounded
`nettop` snapshot joined by PID and protected against PID reuse/counter reset.
On macOS, the GPU collector parses the `AGXAccelerator` property list from `/usr/sbin/ioreg` with a
two-second timeout. Repeated command failures back off exponentially up to five minutes. Missing
or invalid fields change only their capability state and never emit a synthetic zero metric.
Optional GPU attribution reads a fresh, root-owned, non-writable JSON snapshot produced by the
separate one-shot helper. The user collector never invokes `sudo` or `powermetrics` itself.

## 4. Storage Flow

1. `spawn_writer` starts one blocking task that owns a `rusqlite::Connection`.
2. Opening storage creates parent directories, enables WAL, sets a busy timeout, and runs the
   transactional schema upgrade.
3. Startup restores configured rule states from `anomaly_states`.
4. For each received `CollectionBatch`, the writer clones and evaluates the engine, then commits
   raw metric/process samples, collector capability upserts, event transitions, metric/process
   evidence, and all next states inside one SQLite transaction. The live engine advances only
   after commit succeeds.
5. At the configured cadence, the same writer transaction recomputes up to the configured number
   of complete one-minute buckets, then derives complete 15-minute buckets from them.
6. That transaction advances watermarks and deletes expired metrics and closed events in bounded
   chunks, never deleting metric data that its downstream tier has not processed. Event evidence
   is deleted before its closed event; open events are not retention candidates.
7. After commit, storage requests a passive WAL checkpoint. Automatic `VACUUM` is not performed.
8. Shutdown drops the sender, drains queued batches, commits them, and joins the writer task.

Schema version 2 adds `collected_at_ms`, backfills v1 timestamps, and creates `metric_rollups` and
`maintenance_state`. Schema version 3 creates the three anomaly tables and their query indexes.
Schema version 4 creates `process_samples` and `anomaly_event_process_evidence`. Schema version 5
creates `collector_capabilities`. Schema version 6 adds multi-resource process columns and
`process_metric_rollups`. A
database newer than the supported version is rejected rather than opened with an incompatible
writer. Rollup buckets wait for the configured late-arrival grace period.
One-minute aggregation reads raw rows in timestamp/ID order; 15-minute aggregation combines counts,
sums, extremes, and the latest child value so averages remain weighted.

The runtime treats an unexpectedly stopped writer as an application error. Maintenance errors are
logged and do not stop collection. Retry and quarantine behavior for failed inserts is TBD — not
yet designed.

## 5. Anomaly Detection Flow

[`../src/anomaly.rs`](../src/anomaly.rs) evaluates only finite, newer raw metric values whose exact
name matches an enabled rule. State is independent per rule and resource:

```text
normal ── first breach ──► pending ── duration + sample count ──► open
  ▲                           │                                      │
  │                           └── clears/gap ──► normal              ├── sustained critical
  │                                                                  │      └──► critical
  │                                                                  ▼
  └──────── recovery duration + sample count ◄── recovering ◄── recovery threshold
```

A candidate must satisfy both its elapsed duration and minimum sample count. A pending candidate
that changes severity restarts its candidate window. Values between the recovery and warning
thresholds neither recover nor reopen an event. A data gap larger than the rule maximum resets a
pending candidate; for an open/recovering event it increments the gap count, clears escalation and
recovery candidates, and leaves the event open. Duplicate or older timestamps are ignored.

The tracked default rules are:

| Rule / metric | Warning | Critical | Recovery | Maximum gap |
|---|---|---|---|---|
| `cpu-sustained-high` / `cpu.total.usage` | ≥90% for 120 s and 12 samples | ≥97% for 60 s and 12 samples | ≤75% for 60 s and 12 samples | 15 s |
| `memory-pressure` / `memory.usage` | ≥90% for 300 s and 60 samples | ≥95% for 120 s and 24 samples | ≤85% for 120 s and 24 samples | 15 s |
| `disk-space-low` / `disk.space.usage` | ≥90% for 60 s and 2 samples | ≥95% for 60 s and 2 samples | ≤88% for 60 s and 2 samples | 90 s |

When an event opens, storage records up to five configured minutes of prelude samples plus the
trigger. A fresh process snapshot must be no older than two configured process intervals before it
can be copied as attribution evidence. Later evaluations update the last value, peak, sample count,
and gap count; escalation,
newest peak, periodic, and final recovery evidence are preserved. The default periodic interval is
60 seconds. Closed events and their evidence are retained for 365 days by default and removed in
1,000-event maintenance chunks.

CPU events select `cpu_rank`; memory events select `memory_rank`. Trigger evidence is copied first,
then the newest eligible prelude rows until the per-event cap is reached. Escalation, periodic, and
recovery checkpoints use the latest fresh snapshot. Disk-space events intentionally receive no
process attribution because the collected process dimensions do not identify filesystem usage.

## 6. Report Generation Flow

1. `report generate` resolves either `--last` or paired RFC 3339 `--from`/`--to` values. It
   defaults to the last hour and rejects empty, reversed, conflicting, or over-365-day ranges.
2. The reader prefers raw rows for ranges up to two hours, one-minute rollups for ranges up to 24
   hours, and 15-minute rollups for longer ranges, falling back to another retained tier when the
   preferred tier has no rows.
3. A fixed metric whitelist selects total CPU usage, memory usage, Apple GPU usage/memory fields,
   per-mount disk-space usage, disk read/write rates, and network receive/transmit rates. SQL time
   buckets cap each series at approximately 1,200 points while preserving weighted averages and
   observed minima/maxima.
4. The reader adds at most 200 overlapping anomaly events with their bounded stored evidence, plus
   bounded multi-resource process summaries grouped by PID and start time, selecting process raw
   or rollup tiers from retained coverage.
5. The renderer includes current collector capabilities, escapes all persisted labels and names,
   embeds CSS and SVG without JavaScript or external assets, and writes the completed document
   using a temporary sibling plus atomic rename.
6. Output defaults to `~/Documents/SimpleProfiler Reports/`; `--output` selects another file and
   the opt-in `--open` flag invokes the local macOS viewer after the write succeeds.

Report generation is read-only and does not change schema version 6, retention watermarks, anomaly
state, or the running background collector.

## 7. Dashboard Query Flow

1. `dashboard` verifies that the selected database already uses schema version 6, generates a
   random 128-bit session token, and binds an available `127.0.0.1` port by default.
2. Requests must carry the generated token in their path and the exact loopback Host value. Only
   versioned `GET` APIs exist; responses disable caching, framing, referrer forwarding, and remote
   scripts/styles/connections through security headers.
3. Each API request reserves one of four query slots, opens a short-lived SQLite connection with
   read-only flags inside `spawn_blocking`, performs a bounded query, and closes that connection.
4. `/api/v1/snapshot` uses the report range resolver, resolution fallback, fixed metric whitelist,
   approximately 1,200 points per series, 200 event-summary limit, and bounded multi-resource
   process union. It also returns at most three ranked process identities per matching dimension,
   each downsampled to approximately 360 raw or rollup buckets, plus a retained system-memory
   total for percentage conversion. `/api/v1/events/<ID>` loads preserved evidence only for the
   selected event.
5. The embedded browser client renders light/dark resource charts with min/max bands and explicit
   gaps, a GPU summary, capability health, anomaly drill-down, storage health, and sortable process
   summaries. A retained-history slider, Earlier/Later/Live controls, chart pointer dragging, and
   chart keyboard navigation convert the selected window into existing bounded `from`/`to`
   requests. Pointer hover or chart focus shows the nearest timestamp and system average/min/max;
   CPU, memory, disk-I/O, network, and GPU charts add ranked process lines with color, pattern,
   label, and tooltip values. Memory displays percentage and bytes. Disk-space capacity uses a
   separate host-wide writer-activity lane because percent and bytes-per-second cannot share a scale. Historical
   navigation disables auto-refresh until Live is selected.
6. Slider input is debounced before querying, concurrent refresh requests collapse to the newest
   queued range, and all navigation plus process chart series remain subject to the same 365-day,
   point, event, process, and four-query admission limits.
7. Ctrl-C or SIGTERM gracefully stops only the dashboard listener. The separately installed
   background collector and its single writer continue uninterrupted.

The server rejects invalid/over-365-day ranges with HTTP 400, excess concurrent queries with 503,
unknown sessions/events with 404, and a mismatched Host value with 403.

## 8. Metric Naming

The implemented names are:

| Pattern | Unit | Cardinality |
|---|---|---|
| `cpu.total.usage` | `percent` | One per cycle |
| `cpu.core.<index>.usage` | `percent` | One per logical CPU per cycle |
| `memory.total` | `bytes` | One per cycle |
| `memory.used` | `bytes` | One per cycle |
| `memory.available` | `bytes` | One per cycle |
| `memory.usage` | `percent` | One per cycle |
| `gpu.device.usage` | `percent` | One per Apple GPU collection when exposed |
| `gpu.renderer.usage` | `percent` | One per Apple GPU collection when exposed |
| `gpu.tiler.usage` | `percent` | One per Apple GPU collection when exposed |
| `gpu.memory.used` | `bytes` | One per Apple GPU collection when exposed |
| `gpu.memory.allocated` | `bytes` | One per Apple GPU collection when exposed |
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

## 9. Failure Behavior

- Invalid sampling/retention values, interval, channel capacity, retention-tier ordering, process
  ranking limits, provider timeouts/freshness, event-evidence caps, or GPU interval values are rejected before the
  runtime starts.
- A collector error is logged, while successful collectors in the same cycle continue to storage.
- An all-failed cycle does not write an empty batch but still counts toward `--samples`.
- A closed storage channel stops the run with an error.
- A second process targeting the same database exits before it creates a channel or writer.
- A writer panic or database error is returned when the writer task is joined.
- SQLite transaction failure does not partially commit the affected batch.
- SQLite transaction failure also leaves the live anomaly engine unchanged, so persisted and
  in-memory state cannot diverge across a failed batch.
- Invalid anomaly rule IDs, duplicate IDs, non-finite or unordered thresholds, zero sample limits,
  and zero maximum gaps are rejected during configuration validation.
- Rollup rows, cleanup, and watermarks commit in one transaction; a failed maintenance pass does
  not expose a partially advanced watermark.
- Report ranges larger than 365 days or invalid/conflicting time options are rejected before query;
  a valid range with no retained data still produces an explicit empty-state report.
- Report output uses a temporary file and rename so a render/write failure does not expose a
  partially written destination.
- Dashboard startup rejects a missing or non-current database without creating/migrating it, never
  binds outside IPv4 loopback, and creates a new session URL on every launch.
- Dashboard queries run off Tokio worker threads with a four-query admission limit; an API error
  does not affect the collector or SQLite writer.
- GPU command failures and timeouts emit no GPU metrics, persist degraded field capabilities, and
  back off repeated retries. Missing property-list fields are unavailable rather than zero.
- Process network/helper failures preserve CPU, memory, and disk samples and update only the
  affected capability. GPU snapshots are rejected when stale, oversized, non-regular, non-root
  owned, or group/world writable.
- `service stop` sends SIGTERM and fails if the process does not report stopped within 20 seconds.
- `launchctl` failures include stderr context instead of being reported as successful lifecycle
  changes.

Historical collector-health timelines, explicit missing-sample markers, and collector-failure
event rules are planned — no schema yet. Current field-level capability state is implemented.

## 10. Deprecated Components

N/A — the initial version has no deprecated domain components.

## 11. Developer Tooling / Maintenance Scripts

No separate domain maintenance scripts exist. The writer performs rollup and retention maintenance
internally. The `status` command shows raw/rollup ranges, storage sizes, watermarks, maintenance,
open-event counts, and current collector capabilities. `events list` lists recent or open events
and `events show` renders one
event's thresholds, measurements, counts, metric evidence, and related-process evidence.
`processes top` renders the latest CPU, memory, disk, network, or GPU ranking. `report generate` performs an
on-demand read-only query and writes the selected range as local HTML; `dashboard` serves bounded
read-only JSON queries and compiled-in assets until interrupted. Neither is scheduled. On
macOS, the `service` command group implements this lifecycle:

```text
uninstalled ── install ──► loaded/running
      ▲                         │
      └────── uninstall ◄───────┤
                                ├── stop ──► loaded/stopped ── start ──► loaded/running
                                └── restart ───────────────────────────► loaded/running
```

Install and upgrade write the executable, plist, and managed `~/.local/bin/simple-profiler`
launcher atomically and preserve an existing private configuration. The launcher selects that
configuration automatically; install refuses to overwrite an unmanaged file at the same path.
Status combines parsed `launchctl` state with the latest sample, maintenance state, and open
anomaly counts. Normal uninstall removes the service binary/plist and managed launcher; `--purge`
also removes configuration, metrics, and logs. Startup applies built-in schema upgrades; manual
compaction and repair commands are TBD — not yet designed.
