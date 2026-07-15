use std::{fs, path::Path, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub database_path: PathBuf,
    pub interval_seconds: u64,
    pub channel_capacity: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            database_path: PathBuf::from("data/simple-profiler.sqlite3"),
            interval_seconds: 5,
            channel_capacity: 128,
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
}
