use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(database_path: &Path) -> Result<Self> {
        if let Some(parent) = database_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let path = lock_path(database_path);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open instance lock {}", path.display()))?;
        try_lock(&file).with_context(|| {
            format!(
                "another profiler is already using database {}",
                database_path.display()
            )
        })?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_data()?;

        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unlock(&self._file);
    }
}

fn lock_path(database_path: &Path) -> PathBuf {
    let mut path = database_path.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

#[cfg(unix)]
fn try_lock(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: flock only observes the valid descriptor owned by `file` and does not retain it.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        bail!(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unlock(file: &File) {
    use std::os::fd::AsRawFd;

    // SAFETY: flock only observes the valid descriptor owned by `file` and does not retain it.
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

#[cfg(not(unix))]
fn try_lock(_file: &File) -> Result<()> {
    bail!("single-instance locking is not implemented on this platform")
}

#[cfg(not(unix))]
fn unlock(_file: &File) {}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn prevents_two_collectors_from_using_the_same_database() {
        let directory = tempdir().expect("temp dir");
        let database = directory.path().join("metrics.sqlite3");
        let first = InstanceLock::acquire(&database).expect("first lock");

        assert!(InstanceLock::acquire(&database).is_err());
        assert_eq!(first.path(), directory.path().join("metrics.sqlite3.lock"));

        drop(first);
        InstanceLock::acquire(&database).expect("lock released with process handle");
    }
}
