use std::{collections::HashMap, path::PathBuf, time::Duration, time::Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::{CollectionContext, CollectorError};
use crate::{
    config::ProcessConfig,
    model::{ProcessSample, ProcessSnapshot},
};

pub struct ProcessCollector {
    config: ProcessConfig,
    system: Option<System>,
    last_collection: Option<Instant>,
    warmed_up: bool,
}

impl ProcessCollector {
    pub fn new(config: ProcessConfig) -> Self {
        Self {
            config,
            system: Some(System::new()),
            last_collection: None,
            warmed_up: false,
        }
    }

    pub async fn collect(
        &mut self,
        context: &CollectionContext,
    ) -> Result<Option<ProcessSnapshot>, CollectorError> {
        if !self.config.enabled
            || self.last_collection.is_some_and(|last| {
                last.elapsed() < Duration::from_secs(self.config.interval_seconds)
            })
        {
            return Ok(None);
        }

        let mut system = self.system.take().unwrap_or_default();
        let config = self.config.clone();
        let collected_at = context.collected_at;
        let result = tokio::task::spawn_blocking(move || {
            let mut refresh_kind = ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .without_tasks();
            if config.include_executable_path {
                refresh_kind = refresh_kind.with_exe(UpdateKind::OnlyIfNotSet);
            }
            system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
            let candidates = system
                .processes()
                .values()
                .map(|process| ProcessCandidate {
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
                })
                .collect();
            let samples = rank_candidates(candidates, &config, collected_at);
            (system, samples)
        })
        .await;

        let (system, samples) = match result {
            Ok(result) => result,
            Err(error) => {
                self.system = Some(System::new());
                return Err(CollectorError::Unavailable(format!(
                    "process refresh task failed: {error}"
                )));
            }
        };
        self.system = Some(system);
        self.last_collection = Some(Instant::now());
        if !self.warmed_up {
            self.warmed_up = true;
            return Ok(None);
        }
        Ok(Some(samples))
    }
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
}

fn rank_candidates(
    candidates: Vec<ProcessCandidate>,
    config: &ProcessConfig,
    collected_at: chrono::DateTime<chrono::Utc>,
) -> ProcessSnapshot {
    let mut cpu_order: Vec<_> = (0..candidates.len()).collect();
    cpu_order.sort_by(|left, right| {
        candidates[*right]
            .cpu_usage_percent
            .total_cmp(&candidates[*left].cpu_usage_percent)
            .then_with(|| {
                candidate_identity(&candidates[*left]).cmp(&candidate_identity(&candidates[*right]))
            })
    });
    let mut memory_order: Vec<_> = (0..candidates.len()).collect();
    memory_order.sort_by(|left, right| {
        candidates[*right]
            .memory_bytes
            .cmp(&candidates[*left].memory_bytes)
            .then_with(|| {
                candidate_identity(&candidates[*left]).cmp(&candidate_identity(&candidates[*right]))
            })
    });

    let mut selected = HashMap::new();
    for (rank, index) in cpu_order.into_iter().take(config.top_cpu).enumerate() {
        let candidate = &candidates[index];
        selected
            .entry(candidate_identity(candidate))
            .or_insert_with(|| sample_from_candidate(candidate, config, collected_at))
            .cpu_rank = Some(u32::try_from(rank + 1).unwrap_or(u32::MAX));
    }
    for (rank, index) in memory_order.into_iter().take(config.top_memory).enumerate() {
        let candidate = &candidates[index];
        selected
            .entry(candidate_identity(candidate))
            .or_insert_with(|| sample_from_candidate(candidate, config, collected_at))
            .memory_rank = Some(u32::try_from(rank + 1).unwrap_or(u32::MAX));
    }
    let mut samples: Vec<_> = selected.into_values().collect();
    samples.sort_by_key(|sample| {
        (
            sample
                .cpu_rank
                .unwrap_or(u32::MAX)
                .min(sample.memory_rank.unwrap_or(u32::MAX)),
            sample.pid,
            sample.process_start_time_seconds,
        )
    });
    samples
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
        cpu_rank: None,
        memory_rank: None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn candidate(pid: u32, start: u64, cpu: f64, memory: u64) -> ProcessCandidate {
        ProcessCandidate {
            pid,
            process_start_time_seconds: start,
            parent_pid: None,
            name: format!("process-{pid}-{start}"),
            executable_path: Some(PathBuf::from(format!("/private/process-{pid}"))),
            cpu_usage_percent: cpu,
            memory_bytes: memory,
        }
    }

    #[test]
    fn keeps_the_union_of_cpu_and_memory_rankings() {
        let config = ProcessConfig {
            top_cpu: 1,
            top_memory: 1,
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
}
