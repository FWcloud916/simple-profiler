use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Context, Result};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

use crate::config::LoggingConfig;

pub fn init(config: &LoggingConfig) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Some(path) = &config.file {
        let writer = RotatingMakeWriter::new(path, config.max_bytes, config.retained_files)?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(writer)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    Ok(())
}

#[derive(Clone)]
struct RotatingMakeWriter {
    state: Arc<Mutex<RotatingState>>,
}

impl RotatingMakeWriter {
    fn new(path: &Path, max_bytes: u64, retained_files: usize) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create log directory {}", parent.display()))?;
        }
        Ok(Self {
            state: Arc::new(Mutex::new(RotatingState::open(
                path,
                max_bytes,
                retained_files,
            )?)),
        })
    }
}

impl<'a> MakeWriter<'a> for RotatingMakeWriter {
    type Writer = RotatingGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingGuard {
            state: self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }
}

struct RotatingGuard<'a> {
    state: MutexGuard<'a, RotatingState>,
}

impl Write for RotatingGuard<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.state.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state.file.flush()
    }
}

struct RotatingState {
    path: PathBuf,
    file: File,
    bytes_written: u64,
    max_bytes: u64,
    retained_files: usize,
}

impl RotatingState {
    fn open(path: &Path, max_bytes: u64, retained_files: usize) -> Result<Self> {
        let file = open_log(path)?;
        let bytes_written = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            bytes_written,
            max_bytes,
            retained_files,
        })
    }

    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.bytes_written > 0
            && self.bytes_written.saturating_add(buffer.len() as u64) > self.max_bytes
        {
            self.rotate()?;
        }
        let written = self.file.write(buffer)?;
        self.bytes_written = self.bytes_written.saturating_add(written as u64);
        Ok(written)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        for index in (1..=self.retained_files).rev() {
            let source = if index == 1 {
                self.path.clone()
            } else {
                rotated_path(&self.path, index - 1)
            };
            let destination = rotated_path(&self.path, index);
            if index == self.retained_files {
                remove_if_present(&destination)?;
            }
            if source.exists() {
                fs::rename(source, destination)?;
            }
        }
        self.file = open_log(&self.path)?;
        self.bytes_written = 0;
        Ok(())
    }
}

impl Write for RotatingState {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        RotatingState::write(self, buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn open_log(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{index}"));
    PathBuf::from(value)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn rotates_and_limits_log_files() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("profiler.log");
        let mut state = RotatingState::open(&path, 5, 2).expect("writer");

        state.write_all(b"12345").expect("first log");
        state.write_all(b"67890").expect("second log");
        state.write_all(b"abcde").expect("third log");
        state.flush().expect("flush");

        assert_eq!(fs::read_to_string(&path).expect("current"), "abcde");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).expect("first rotated"),
            "67890"
        );
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 2)).expect("second rotated"),
            "12345"
        );
        assert!(!rotated_path(&path, 3).exists());
    }
}
