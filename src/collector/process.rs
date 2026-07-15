use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde::Deserialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tokio::{process::Command, time::timeout};

use super::{CollectionContext, CollectorError};
use crate::{
    config::ProcessConfig,
    model::{CapabilityState, CollectorCapability, ProcessSample, ProcessSnapshot},
};

#[derive(Debug, Default)]
pub struct ProcessCollection {
    pub samples: ProcessSnapshot,
    pub capabilities: Vec<CollectorCapability>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct NetworkTotals {
    received: u64,
    transmitted: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct GpuTotal {
    pid: u32,
    gpu_time_ns: u64,
}

#[derive(Debug, Deserialize)]
struct GpuSnapshotFile {
    collected_at_ms: i64,
    processes: Vec<GpuTotal>,
}

pub struct ProcessCollector {
    config: ProcessConfig,
    system: Option<System>,
    last_collection: Option<Instant>,
    previous_network: HashMap<(u32, u64), NetworkTotals>,
    previous_gpu: HashMap<(u32, u64), u64>,
    warmed_up: bool,
}

impl ProcessCollector {
    pub fn new(config: ProcessConfig) -> Self {
        Self {
            config,
            system: Some(System::new()),
            last_collection: None,
            previous_network: HashMap::new(),
            previous_gpu: HashMap::new(),
            warmed_up: false,
        }
    }

    pub async fn collect(
        &mut self,
        context: &CollectionContext,
    ) -> Result<Option<ProcessCollection>, CollectorError> {
        if !self.config.enabled
            || self.last_collection.is_some_and(|last| {
                last.elapsed() < Duration::from_secs(self.config.interval_seconds)
            })
        {
            return Ok(None);
        }

        let elapsed = self
            .last_collection
            .map_or(Duration::from_secs(self.config.interval_seconds), |last| {
                last.elapsed()
            });
        let mut system = self.system.take().unwrap_or_default();
        let config = self.config.clone();
        let collected_at = context.collected_at;
        let result = tokio::task::spawn_blocking(move || {
            let mut refresh_kind = ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage()
                .without_tasks();
            if config.include_executable_path {
                refresh_kind = refresh_kind.with_exe(UpdateKind::OnlyIfNotSet);
            }
            system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
            let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
            let candidates = system
                .processes()
                .values()
                .map(|process| {
                    let disk = process.disk_usage();
                    ProcessCandidate {
                        pid: process.pid().as_u32(),
                        process_start_time_seconds: process.start_time(),
                        parent_pid: process.parent().map(|pid| pid.as_u32()),
                        name: process.name().to_string_lossy().into_owned(),
                        executable_path: config
                            .include_executable_path
                            .then(|| process.exe().map(PathBuf::from))
                            .flatten(),
                        cpu_usage_percent: f64::from(process.cpu_usage()),
                        memory_bytes: process.memory(),
                        disk_read_bytes: disk.read_bytes,
                        disk_write_bytes: disk.written_bytes,
                        disk_read_bytes_per_second: disk.read_bytes as f64 / seconds,
                        disk_write_bytes_per_second: disk.written_bytes as f64 / seconds,
                        network_receive_bytes: None,
                        network_transmit_bytes: None,
                        network_receive_bytes_per_second: None,
                        network_transmit_bytes_per_second: None,
                        gpu_time_ns: None,
                        gpu_usage_percent: None,
                    }
                })
                .collect::<Vec<_>>();
            (system, candidates)
        })
        .await;

        let (system, mut candidates) = match result {
            Ok(result) => result,
            Err(error) => {
                self.system = Some(System::new());
                return Err(CollectorError::Unavailable(format!(
                    "process refresh task failed: {error}"
                )));
            }
        };
        self.system = Some(system);

        let mut collection = ProcessCollection::default();
        collection
            .capabilities
            .extend(base_capabilities(collected_at.timestamp_millis()));

        if self.config.network_enabled {
            match collect_network_totals(&self.config).await {
                Ok(totals) => {
                    apply_network_deltas(
                        &mut candidates,
                        &totals,
                        &mut self.previous_network,
                        elapsed,
                    );
                    collection.capabilities.push(capability(
                        "process.network_io",
                        CapabilityState::Available,
                        "macos-nettop",
                        None,
                        collected_at.timestamp_millis(),
                    ));
                }
                Err(error) => {
                    collection.warnings.push(error.clone());
                    collection.capabilities.push(capability(
                        "process.network_io",
                        CapabilityState::Degraded,
                        "macos-nettop",
                        Some(error),
                        collected_at.timestamp_millis(),
                    ));
                }
            }
        } else {
            collection.capabilities.push(capability(
                "process.network_io",
                CapabilityState::Unavailable,
                "disabled",
                Some("process network collection is disabled".to_owned()),
                collected_at.timestamp_millis(),
            ));
        }

        if let Some(path) = self.config.gpu_snapshot_path.as_deref() {
            match collect_gpu_totals(
                path,
                self.config.gpu_snapshot_max_age_seconds,
                collected_at.timestamp_millis(),
            )
            .await
            {
                Ok(totals) => {
                    apply_gpu_deltas(&mut candidates, &totals, &mut self.previous_gpu, elapsed);
                    collection.capabilities.push(capability(
                        "process.gpu_time",
                        CapabilityState::Available,
                        "privileged-helper",
                        None,
                        collected_at.timestamp_millis(),
                    ));
                }
                Err(error) => {
                    collection.warnings.push(error.clone());
                    collection.capabilities.push(capability(
                        "process.gpu_time",
                        CapabilityState::Degraded,
                        "privileged-helper",
                        Some(error),
                        collected_at.timestamp_millis(),
                    ));
                }
            }
        } else {
            collection.capabilities.push(capability(
                "process.gpu_time",
                CapabilityState::Unavailable,
                "not-configured",
                Some("optional privileged GPU helper is not configured".to_owned()),
                collected_at.timestamp_millis(),
            ));
        }

        self.last_collection = Some(Instant::now());
        if !self.warmed_up {
            self.warmed_up = true;
            return Ok(Some(collection));
        }
        collection.samples = rank_candidates(candidates, &self.config, collected_at);
        Ok(Some(collection))
    }
}

fn base_capabilities(checked_at_ms: i64) -> Vec<CollectorCapability> {
    ["process.cpu", "process.memory", "process.disk_io"]
        .into_iter()
        .map(|name| {
            capability(
                name,
                CapabilityState::Available,
                "sysinfo",
                None,
                checked_at_ms,
            )
        })
        .collect()
}

fn capability(
    name: &str,
    state: CapabilityState,
    provider: &str,
    detail: Option<String>,
    checked_at_ms: i64,
) -> CollectorCapability {
    CollectorCapability {
        collector: "process".to_owned(),
        resource: String::new(),
        capability: name.to_owned(),
        state,
        provider: provider.to_owned(),
        detail,
        checked_at_ms,
    }
}

async fn command_output(
    command: &PathBuf,
    args: &[&str],
    timeout_seconds: u64,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(command);
    child.args(args).kill_on_drop(true);
    let output = timeout(Duration::from_secs(timeout_seconds), child.output())
        .await
        .map_err(|_| format!("{} timed out", command.display()))?
        .map_err(|error| format!("failed to run {}: {error}", command.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} exited with {}: {}",
            command.display(),
            output.status,
            detail.trim().chars().take(240).collect::<String>()
        ));
    }
    if output.stdout.len() > 4 * 1024 * 1024 {
        return Err(format!("{} returned more than 4 MiB", command.display()));
    }
    Ok(output.stdout)
}

async fn collect_network_totals(
    config: &ProcessConfig,
) -> Result<HashMap<u32, NetworkTotals>, String> {
    let output = command_output(
        &config.network_command,
        &["-P", "-L", "1", "-n", "-x", "-J", "bytes_in,bytes_out"],
        config.network_command_timeout_seconds,
    )
    .await?;
    parse_nettop(&String::from_utf8_lossy(&output))
}

fn parse_nettop(source: &str) -> Result<HashMap<u32, NetworkTotals>, String> {
    let mut totals = HashMap::new();
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let line = line.trim_end_matches(',');
        if line.starts_with("time,") || line.starts_with(",bytes_in,") {
            continue;
        }
        let mut fields = line.rsplitn(3, ',');
        let transmitted = fields.next().and_then(|value| value.parse::<u64>().ok());
        let received = fields.next().and_then(|value| value.parse::<u64>().ok());
        let identity = fields.next();
        let (Some(transmitted), Some(received), Some(identity)) = (transmitted, received, identity)
        else {
            continue;
        };
        let Some(pid) = identity
            .rsplit_once('.')
            .and_then(|(_, pid)| pid.parse::<u32>().ok())
        else {
            continue;
        };
        totals
            .entry(pid)
            .and_modify(|total: &mut NetworkTotals| {
                total.received = total.received.saturating_add(received);
                total.transmitted = total.transmitted.saturating_add(transmitted);
            })
            .or_insert(NetworkTotals {
                received,
                transmitted,
            });
    }
    if totals.is_empty() {
        return Err("nettop returned no process counters".to_owned());
    }
    Ok(totals)
}

fn apply_network_deltas(
    candidates: &mut [ProcessCandidate],
    totals: &HashMap<u32, NetworkTotals>,
    previous: &mut HashMap<(u32, u64), NetworkTotals>,
    elapsed: Duration,
) {
    let seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let mut next = HashMap::new();
    for candidate in candidates {
        let key = candidate_identity(candidate);
        let Some(total) = totals.get(&candidate.pid).copied() else {
            continue;
        };
        next.insert(key, total);
        let Some(old) = previous.get(&key) else {
            continue;
        };
        let received = total.received.saturating_sub(old.received);
        let transmitted = total.transmitted.saturating_sub(old.transmitted);
        candidate.network_receive_bytes = Some(received);
        candidate.network_transmit_bytes = Some(transmitted);
        candidate.network_receive_bytes_per_second = Some(received as f64 / seconds);
        candidate.network_transmit_bytes_per_second = Some(transmitted as f64 / seconds);
    }
    *previous = next;
}

async fn collect_gpu_totals(
    path: &std::path::Path,
    max_age_seconds: u64,
    now_ms: i64,
) -> Result<HashMap<u32, u64>, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("could not read GPU snapshot metadata: {error}"))?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err("GPU snapshot must be a regular file no larger than 1 MiB".to_owned());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                return Err(
                    "GPU snapshot must be root-owned and not group/world writable".to_owned(),
                );
            }
        }
        let source = std::fs::read(&path)
            .map_err(|error| format!("could not read GPU snapshot: {error}"))?;
        let snapshot: GpuSnapshotFile = serde_json::from_slice(&source)
            .map_err(|error| format!("GPU snapshot contained invalid JSON: {error}"))?;
        let maximum_age_ms = i64::try_from(max_age_seconds)
            .unwrap_or(i64::MAX / 1_000)
            .saturating_mul(1_000);
        if snapshot.collected_at_ms > now_ms.saturating_add(5_000)
            || now_ms.saturating_sub(snapshot.collected_at_ms) > maximum_age_ms
        {
            return Err("GPU snapshot is stale or has a future timestamp".to_owned());
        }
        Ok(snapshot
            .processes
            .into_iter()
            .map(|row| (row.pid, row.gpu_time_ns))
            .collect())
    })
    .await
    .map_err(|error| format!("GPU snapshot task failed: {error}"))?
}

fn apply_gpu_deltas(
    candidates: &mut [ProcessCandidate],
    totals: &HashMap<u32, u64>,
    previous: &mut HashMap<(u32, u64), u64>,
    elapsed: Duration,
) {
    let elapsed_ns = elapsed.as_nanos().max(1) as f64;
    let mut next = HashMap::new();
    for candidate in candidates {
        let key = candidate_identity(candidate);
        let Some(total) = totals.get(&candidate.pid).copied() else {
            continue;
        };
        next.insert(key, total);
        let Some(old) = previous.get(&key) else {
            continue;
        };
        let delta = total.saturating_sub(*old);
        candidate.gpu_time_ns = Some(delta);
        candidate.gpu_usage_percent = Some((delta as f64 / elapsed_ns * 100.0).clamp(0.0, 100.0));
    }
    *previous = next;
}

#[derive(Debug, Clone)]
struct ProcessCandidate {
    pid: u32,
    process_start_time_seconds: u64,
    parent_pid: Option<u32>,
    name: String,
    executable_path: Option<PathBuf>,
    cpu_usage_percent: f64,
    memory_bytes: u64,
    disk_read_bytes: u64,
    disk_write_bytes: u64,
    disk_read_bytes_per_second: f64,
    disk_write_bytes_per_second: f64,
    network_receive_bytes: Option<u64>,
    network_transmit_bytes: Option<u64>,
    network_receive_bytes_per_second: Option<f64>,
    network_transmit_bytes_per_second: Option<f64>,
    gpu_time_ns: Option<u64>,
    gpu_usage_percent: Option<f64>,
}

fn rank_candidates(
    candidates: Vec<ProcessCandidate>,
    config: &ProcessConfig,
    collected_at: chrono::DateTime<chrono::Utc>,
) -> ProcessSnapshot {
    let mut selected: HashMap<(u32, u64), ProcessSample> = HashMap::new();
    rank_by(
        &candidates,
        config.top_cpu,
        |candidate| candidate.cpu_usage_percent,
        |sample, rank| sample.cpu_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );
    rank_by(
        &candidates,
        config.top_memory,
        |candidate| candidate.memory_bytes as f64,
        |sample, rank| sample.memory_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );
    rank_by(
        &candidates,
        config.top_disk,
        |candidate| candidate.disk_read_bytes_per_second,
        |sample, rank| sample.disk_read_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );
    rank_by(
        &candidates,
        config.top_disk,
        |candidate| candidate.disk_write_bytes_per_second,
        |sample, rank| sample.disk_write_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );
    rank_optional_by(
        &candidates,
        config.top_network,
        |candidate| candidate.network_receive_bytes_per_second,
        |sample, rank| sample.network_receive_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );
    rank_optional_by(
        &candidates,
        config.top_network,
        |candidate| candidate.network_transmit_bytes_per_second,
        |sample, rank| sample.network_transmit_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );
    rank_optional_by(
        &candidates,
        config.top_gpu,
        |candidate| candidate.gpu_usage_percent,
        |sample, rank| sample.gpu_rank = Some(rank),
        &mut selected,
        config,
        collected_at,
    );

    let mut samples: Vec<_> = selected.into_values().collect();
    samples.sort_by_key(|sample| {
        (
            best_rank(sample),
            sample.pid,
            sample.process_start_time_seconds,
        )
    });
    samples.truncate(config.max_snapshot_processes);
    samples
}

fn rank_by<V, A>(
    candidates: &[ProcessCandidate],
    limit: usize,
    value: V,
    assign: A,
    selected: &mut HashMap<(u32, u64), ProcessSample>,
    config: &ProcessConfig,
    collected_at: chrono::DateTime<chrono::Utc>,
) where
    V: Fn(&ProcessCandidate) -> f64,
    A: Fn(&mut ProcessSample, u32),
{
    let mut order: Vec<_> = (0..candidates.len()).collect();
    order.sort_by(|left, right| {
        value(&candidates[*right])
            .total_cmp(&value(&candidates[*left]))
            .then_with(|| {
                candidate_identity(&candidates[*left]).cmp(&candidate_identity(&candidates[*right]))
            })
    });
    for (rank, index) in order.into_iter().take(limit).enumerate() {
        let candidate = &candidates[index];
        let sample = selected
            .entry(candidate_identity(candidate))
            .or_insert_with(|| sample_from_candidate(candidate, config, collected_at));
        assign(sample, u32::try_from(rank + 1).unwrap_or(u32::MAX));
    }
}

fn rank_optional_by<V, A>(
    candidates: &[ProcessCandidate],
    limit: usize,
    value: V,
    assign: A,
    selected: &mut HashMap<(u32, u64), ProcessSample>,
    config: &ProcessConfig,
    collected_at: chrono::DateTime<chrono::Utc>,
) where
    V: Fn(&ProcessCandidate) -> Option<f64>,
    A: Fn(&mut ProcessSample, u32),
{
    let available: Vec<_> = candidates
        .iter()
        .filter(|candidate| value(candidate).is_some())
        .cloned()
        .collect();
    rank_by(
        &available,
        limit,
        |candidate| value(candidate).unwrap_or(0.0),
        assign,
        selected,
        config,
        collected_at,
    );
}

fn best_rank(sample: &ProcessSample) -> u32 {
    [
        sample.cpu_rank,
        sample.memory_rank,
        sample.disk_read_rank,
        sample.disk_write_rank,
        sample.network_receive_rank,
        sample.network_transmit_rank,
        sample.gpu_rank,
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(u32::MAX)
}

fn candidate_identity(candidate: &ProcessCandidate) -> (u32, u64) {
    (candidate.pid, candidate.process_start_time_seconds)
}

fn sample_from_candidate(
    candidate: &ProcessCandidate,
    config: &ProcessConfig,
    collected_at: chrono::DateTime<chrono::Utc>,
) -> ProcessSample {
    ProcessSample {
        collected_at,
        pid: candidate.pid,
        process_start_time_seconds: candidate.process_start_time_seconds,
        parent_pid: candidate.parent_pid,
        name: candidate.name.clone(),
        executable_path: config
            .include_executable_path
            .then(|| {
                candidate
                    .executable_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .flatten(),
        cpu_usage_percent: candidate.cpu_usage_percent,
        memory_bytes: candidate.memory_bytes,
        disk_read_bytes: candidate.disk_read_bytes,
        disk_write_bytes: candidate.disk_write_bytes,
        disk_read_bytes_per_second: candidate.disk_read_bytes_per_second,
        disk_write_bytes_per_second: candidate.disk_write_bytes_per_second,
        network_receive_bytes: candidate.network_receive_bytes,
        network_transmit_bytes: candidate.network_transmit_bytes,
        network_receive_bytes_per_second: candidate.network_receive_bytes_per_second,
        network_transmit_bytes_per_second: candidate.network_transmit_bytes_per_second,
        gpu_time_ns: candidate.gpu_time_ns,
        gpu_usage_percent: candidate.gpu_usage_percent,
        cpu_rank: None,
        memory_rank: None,
        disk_read_rank: None,
        disk_write_rank: None,
        network_receive_rank: None,
        network_transmit_rank: None,
        gpu_rank: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn candidate(pid: u32, start: u64, cpu: f64, memory: u64) -> ProcessCandidate {
        ProcessCandidate {
            pid,
            process_start_time_seconds: start,
            parent_pid: None,
            name: format!("process-{pid}-{start}"),
            executable_path: Some(PathBuf::from(format!("/private/process-{pid}"))),
            cpu_usage_percent: cpu,
            memory_bytes: memory,
            disk_read_bytes: pid as u64 * 10,
            disk_write_bytes: pid as u64 * 20,
            disk_read_bytes_per_second: pid as f64 * 10.0,
            disk_write_bytes_per_second: pid as f64 * 20.0,
            network_receive_bytes: None,
            network_transmit_bytes: None,
            network_receive_bytes_per_second: None,
            network_transmit_bytes_per_second: None,
            gpu_time_ns: None,
            gpu_usage_percent: None,
        }
    }

    #[test]
    fn keeps_the_union_of_all_available_rankings() {
        let config = ProcessConfig {
            top_cpu: 1,
            top_memory: 1,
            top_disk: 1,
            event_top_n: 1,
            ..ProcessConfig::default()
        };
        let samples = rank_candidates(
            vec![candidate(1, 10, 80.0, 100), candidate(2, 20, 10.0, 1_000)],
            &config,
            Utc::now(),
        );
        assert_eq!(samples.len(), 2);
        assert!(samples.iter().any(|sample| sample.cpu_rank == Some(1)));
        assert!(samples.iter().any(|sample| sample.memory_rank == Some(1)));
        assert!(
            samples
                .iter()
                .any(|sample| sample.disk_write_rank == Some(1))
        );
    }

    #[test]
    fn pid_reuse_is_kept_as_a_distinct_process_identity() {
        let config = ProcessConfig {
            top_cpu: 2,
            top_memory: 2,
            event_top_n: 1,
            ..ProcessConfig::default()
        };
        let samples = rank_candidates(
            vec![candidate(7, 10, 80.0, 100), candidate(7, 20, 70.0, 200)],
            &config,
            Utc::now(),
        );
        assert_eq!(samples.len(), 2);
        assert_ne!(
            samples[0].process_start_time_seconds,
            samples[1].process_start_time_seconds
        );
    }

    #[test]
    fn executable_paths_are_private_by_default() {
        let config = ProcessConfig {
            top_cpu: 1,
            top_memory: 1,
            event_top_n: 1,
            include_executable_path: false,
            ..ProcessConfig::default()
        };
        let samples = rank_candidates(vec![candidate(1, 10, 1.0, 1)], &config, Utc::now());
        assert_eq!(samples[0].executable_path, None);
    }

    #[test]
    fn parses_nettop_rows_using_pid_even_when_name_contains_spaces() {
        let totals = parse_nettop(",bytes_in,bytes_out,\nCodex (Service).1931,685324,1684779,\n")
            .expect("nettop");
        assert_eq!(totals[&1931].received, 685_324);
        assert_eq!(totals[&1931].transmitted, 1_684_779);
    }

    #[test]
    fn counter_reset_never_creates_a_huge_delta() {
        let mut candidates = vec![candidate(7, 10, 1.0, 1)];
        let mut previous = HashMap::from([(
            (7, 10),
            NetworkTotals {
                received: 100,
                transmitted: 200,
            },
        )]);
        let totals = HashMap::from([(
            7,
            NetworkTotals {
                received: 5,
                transmitted: 9,
            },
        )]);
        apply_network_deltas(
            &mut candidates,
            &totals,
            &mut previous,
            Duration::from_secs(1),
        );
        assert_eq!(candidates[0].network_receive_bytes, Some(0));
        assert_eq!(candidates[0].network_transmit_bytes, Some(0));
    }
}
