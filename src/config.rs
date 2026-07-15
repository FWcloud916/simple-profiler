use std::{collections::HashSet, fs, path::Path, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub database_path: PathBuf,
    pub interval_seconds: u64,
    pub channel_capacity: usize,
    pub sampling: SamplingConfig,
    pub retention: RetentionConfig,
    pub logging: LoggingConfig,
    pub anomaly: AnomalyConfig,
    pub process: ProcessConfig,
    pub gpu: GpuConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuProvider {
    #[default]
    Auto,
    AppleIoreg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GpuConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub command_timeout_seconds: u64,
    pub provider: GpuProvider,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 15,
            command_timeout_seconds: 2,
            provider: GpuProvider::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingConfig {
    pub disk_capacity_interval_seconds: u64,
    pub suppress_idle_io: bool,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            disk_capacity_interval_seconds: 60,
            suppress_idle_io: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub top_cpu: usize,
    pub top_memory: usize,
    pub top_disk: usize,
    pub top_network: usize,
    pub top_gpu: usize,
    pub max_snapshot_processes: usize,
    pub raw_retention_hours: u64,
    pub minute_retention_days: u64,
    pub quarter_hour_retention_days: u64,
    pub event_top_n: usize,
    pub event_evidence_max_rows: usize,
    pub include_executable_path: bool,
    pub network_enabled: bool,
    pub network_command: PathBuf,
    pub network_command_timeout_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_snapshot_path: Option<PathBuf>,
    pub gpu_snapshot_max_age_seconds: u64,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 15,
            top_cpu: 10,
            top_memory: 10,
            top_disk: 10,
            top_network: 10,
            top_gpu: 10,
            max_snapshot_processes: 40,
            raw_retention_hours: 24,
            minute_retention_days: 7,
            quarter_hour_retention_days: 90,
            event_top_n: 5,
            event_evidence_max_rows: 500,
            include_executable_path: false,
            network_enabled: cfg!(target_os = "macos"),
            network_command: PathBuf::from("/usr/bin/nettop"),
            network_command_timeout_seconds: 5,
            gpu_snapshot_path: None,
            gpu_snapshot_max_age_seconds: 45,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    pub max_bytes: u64,
    pub retained_files: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            file: None,
            max_bytes: 10 * 1024 * 1024,
            retained_files: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnomalyConfig {
    pub enabled: bool,
    pub event_retention_days: u64,
    pub prelude_minutes: u64,
    pub evidence_interval_seconds: u64,
    pub delete_batch_rows: usize,
    pub rules: Vec<AnomalyRuleConfig>,
}

impl Default for AnomalyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            event_retention_days: 365,
            prelude_minutes: 5,
            evidence_interval_seconds: 60,
            delete_batch_rows: 1_000,
            rules: default_anomaly_rules(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnomalyRuleConfig {
    pub id: String,
    pub enabled: bool,
    pub metric_name: String,
    pub warning_threshold: f64,
    pub critical_threshold: f64,
    pub recovery_threshold: f64,
    pub trigger_seconds: u64,
    pub critical_trigger_seconds: u64,
    pub recovery_seconds: u64,
    pub min_samples: u64,
    pub critical_min_samples: u64,
    pub recovery_min_samples: u64,
    pub max_sample_gap_seconds: u64,
}

impl Default for AnomalyRuleConfig {
    fn default() -> Self {
        Self {
            id: "custom-high".to_owned(),
            enabled: true,
            metric_name: "cpu.total.usage".to_owned(),
            warning_threshold: 90.0,
            critical_threshold: 97.0,
            recovery_threshold: 75.0,
            trigger_seconds: 120,
            critical_trigger_seconds: 60,
            recovery_seconds: 60,
            min_samples: 12,
            critical_min_samples: 12,
            recovery_min_samples: 12,
            max_sample_gap_seconds: 15,
        }
    }
}

fn default_anomaly_rules() -> Vec<AnomalyRuleConfig> {
    vec![
        AnomalyRuleConfig {
            id: "cpu-sustained-high".to_owned(),
            ..AnomalyRuleConfig::default()
        },
        AnomalyRuleConfig {
            id: "memory-pressure".to_owned(),
            metric_name: "memory.usage".to_owned(),
            warning_threshold: 90.0,
            critical_threshold: 95.0,
            recovery_threshold: 85.0,
            trigger_seconds: 300,
            critical_trigger_seconds: 120,
            recovery_seconds: 120,
            min_samples: 60,
            critical_min_samples: 24,
            recovery_min_samples: 24,
            max_sample_gap_seconds: 15,
            ..AnomalyRuleConfig::default()
        },
        AnomalyRuleConfig {
            id: "disk-space-low".to_owned(),
            metric_name: "disk.space.usage".to_owned(),
            warning_threshold: 90.0,
            critical_threshold: 95.0,
            recovery_threshold: 88.0,
            trigger_seconds: 60,
            critical_trigger_seconds: 60,
            recovery_seconds: 60,
            min_samples: 2,
            critical_min_samples: 2,
            recovery_min_samples: 2,
            max_sample_gap_seconds: 90,
            ..AnomalyRuleConfig::default()
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionConfig {
    pub raw_hours: u64,
    pub minute_days: u64,
    pub quarter_hour_days: u64,
    pub maintenance_interval_seconds: u64,
    pub late_arrival_grace_seconds: u64,
    pub delete_batch_rows: usize,
    pub rollup_batch_buckets: usize,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            raw_hours: 24,
            minute_days: 30,
            quarter_hour_days: 365,
            maintenance_interval_seconds: 60,
            late_arrival_grace_seconds: 30,
            delete_batch_rows: 10_000,
            rollup_batch_buckets: 60,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            database_path: PathBuf::from("data/simple-profiler.sqlite3"),
            interval_seconds: 5,
            channel_capacity: 128,
            sampling: SamplingConfig::default(),
            retention: RetentionConfig::default(),
            logging: LoggingConfig::default(),
            anomaly: AnomalyConfig::default(),
            process: ProcessConfig::default(),
            gpu: GpuConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Self = toml::from_str(&source)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.interval_seconds == 0 {
            bail!("interval_seconds must be greater than zero");
        }
        if self.channel_capacity == 0 {
            bail!("channel_capacity must be greater than zero");
        }
        if self.sampling.disk_capacity_interval_seconds == 0 {
            bail!("sampling.disk_capacity_interval_seconds must be greater than zero");
        }
        if self.logging.max_bytes == 0 || self.logging.retained_files == 0 {
            bail!("logging.max_bytes and logging.retained_files must be greater than zero");
        }
        self.validate_process()?;
        self.validate_gpu()?;
        self.validate_anomaly()?;
        if self.retention.raw_hours == 0
            || self.retention.minute_days == 0
            || self.retention.quarter_hour_days == 0
            || self.retention.maintenance_interval_seconds == 0
            || self.retention.delete_batch_rows == 0
            || self.retention.rollup_batch_buckets == 0
        {
            bail!(
                "retention durations, delete_batch_rows, and rollup_batch_buckets must be greater than zero"
            );
        }
        if self.retention.quarter_hour_days < self.retention.minute_days {
            bail!("retention.quarter_hour_days must not be shorter than minute_days");
        }
        if self.retention.minute_days.saturating_mul(24) < self.retention.raw_hours {
            bail!("retention.minute_days must not be shorter than raw_hours");
        }
        Ok(())
    }

    fn validate_gpu(&self) -> Result<()> {
        if self.gpu.interval_seconds == 0 || self.gpu.command_timeout_seconds == 0 {
            bail!("gpu interval and command timeout must be greater than zero");
        }
        if self.gpu.command_timeout_seconds > 30 {
            bail!("gpu.command_timeout_seconds must not exceed 30");
        }
        Ok(())
    }

    fn validate_process(&self) -> Result<()> {
        let process = &self.process;
        if process.interval_seconds == 0
            || process.top_cpu == 0
            || process.top_memory == 0
            || process.top_disk == 0
            || process.top_network == 0
            || process.top_gpu == 0
            || process.max_snapshot_processes == 0
            || process.raw_retention_hours == 0
            || process.minute_retention_days == 0
            || process.quarter_hour_retention_days == 0
            || process.event_top_n == 0
            || process.event_evidence_max_rows == 0
            || process.network_command_timeout_seconds == 0
            || process.gpu_snapshot_max_age_seconds == 0
        {
            bail!("process intervals, limits, and retention must be greater than zero");
        }
        if [
            process.top_cpu,
            process.top_memory,
            process.top_disk,
            process.top_network,
            process.top_gpu,
            process.max_snapshot_processes,
        ]
        .into_iter()
        .any(|limit| limit > 100)
        {
            bail!("process ranking limits must not exceed 100");
        }
        if process.event_top_n > process.top_cpu.min(process.top_memory) {
            bail!("process event_top_n must not exceed top_cpu or top_memory");
        }
        if process.event_evidence_max_rows < process.event_top_n {
            bail!("process event_evidence_max_rows must be at least event_top_n");
        }
        if process.minute_retention_days.saturating_mul(24) < process.raw_retention_hours
            || process.quarter_hour_retention_days < process.minute_retention_days
        {
            bail!("process retention must satisfy raw <= minute <= quarter_hour");
        }
        if process.network_command_timeout_seconds > 30 {
            bail!("process network provider timeout must not exceed 30 seconds");
        }
        Ok(())
    }

    fn validate_anomaly(&self) -> Result<()> {
        let anomaly = &self.anomaly;
        if anomaly.event_retention_days == 0
            || anomaly.prelude_minutes == 0
            || anomaly.evidence_interval_seconds == 0
            || anomaly.delete_batch_rows == 0
        {
            bail!(
                "anomaly retention, evidence intervals, and delete_batch_rows must be greater than zero"
            );
        }
        let mut ids = HashSet::new();
        for rule in &anomaly.rules {
            if rule.id.trim().is_empty() || rule.metric_name.trim().is_empty() {
                bail!("anomaly rule id and metric_name must not be empty");
            }
            if !ids.insert(&rule.id) {
                bail!("anomaly rule ids must be unique: {}", rule.id);
            }
            if !rule.warning_threshold.is_finite()
                || !rule.critical_threshold.is_finite()
                || !rule.recovery_threshold.is_finite()
                || rule.critical_threshold < rule.warning_threshold
                || rule.recovery_threshold >= rule.warning_threshold
            {
                bail!("anomaly rule {} has invalid high-water thresholds", rule.id);
            }
            if rule.min_samples == 0
                || rule.critical_min_samples == 0
                || rule.recovery_min_samples == 0
                || rule.max_sample_gap_seconds == 0
            {
                bail!(
                    "anomaly rule {} sample limits must be greater than zero",
                    rule.id
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_interval() {
        let config = AppConfig {
            interval_seconds: 0,
            ..AppConfig::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn accepts_the_default_retention_hierarchy() {
        assert!(AppConfig::default().validate().is_ok());
    }

    #[test]
    fn rejects_unbounded_process_cardinality() {
        let config = AppConfig {
            process: ProcessConfig {
                top_cpu: 101,
                ..ProcessConfig::default()
            },
            ..AppConfig::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_an_unbounded_gpu_command_timeout() {
        let config = AppConfig {
            gpu: GpuConfig {
                command_timeout_seconds: 31,
                ..GpuConfig::default()
            },
            ..AppConfig::default()
        };

        assert!(config.validate().is_err());
    }
}
