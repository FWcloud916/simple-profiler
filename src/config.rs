use std::{fs, path::Path, path::PathBuf};

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
}
