use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::{AppConfig, config::LoggingConfig};

pub const SERVICE_LABEL: &str = "com.simple-profiler.agent";
const CLI_LAUNCHER_PREFIX: &str = "#!/bin/sh\n# Managed by Simple Profiler. Do not edit.\n";
const LEGACY_CLI_LAUNCHER: &str = r#"#!/bin/sh

exec "$HOME/Library/Application Support/SimpleProfiler/bin/simple-profiler" \
  --config "$HOME/Library/Application Support/SimpleProfiler/config.toml" \
  "$@"
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePaths {
    pub application_dir: PathBuf,
    pub binary: PathBuf,
    pub config: PathBuf,
    pub database: PathBuf,
    pub logs: PathBuf,
    pub plist: PathBuf,
    pub cli_launcher: PathBuf,
}

impl ServicePaths {
    pub fn for_home(home: &Path) -> Self {
        let application_dir = home
            .join("Library")
            .join("Application Support")
            .join("SimpleProfiler");
        let logs = home.join("Library").join("Logs").join("SimpleProfiler");
        Self {
            binary: application_dir.join("bin").join("simple-profiler"),
            config: application_dir.join("config.toml"),
            database: application_dir.join("data").join("simple-profiler.sqlite3"),
            plist: home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{SERVICE_LABEL}.plist")),
            cli_launcher: home.join(".local").join("bin").join("simple-profiler"),
            application_dir,
            logs,
        }
    }

    pub fn from_environment() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(Self::for_home(Path::new(&home)))
    }

    fn stdout_log(&self) -> PathBuf {
        self.logs.join("service.stdout.log")
    }

    fn stderr_log(&self) -> PathBuf {
        self.logs.join("service.stderr.log")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub installed: bool,
    pub loaded: bool,
    pub state: Option<String>,
    pub pid: Option<u32>,
    pub last_exit_code: Option<i32>,
}

impl ServiceStatus {
    pub fn running(&self) -> bool {
        self.state.as_deref() == Some("running") || self.pid.is_some()
    }
}

pub struct ServiceManager {
    paths: ServicePaths,
}

impl ServiceManager {
    pub fn from_environment() -> Result<Self> {
        require_macos()?;
        Ok(Self {
            paths: ServicePaths::from_environment()?,
        })
    }

    pub fn paths(&self) -> &ServicePaths {
        &self.paths
    }

    pub fn install(&self, source_binary: &Path) -> Result<()> {
        install_files(&self.paths, source_binary)?;
        if self.status()?.loaded {
            run_launchctl([OsStr::new("bootout"), self.target().as_os_str()])?;
            self.wait_until_unloaded(Duration::from_secs(5))?;
        }
        run_launchctl([
            OsStr::new("bootstrap"),
            self.domain().as_os_str(),
            self.paths.plist.as_os_str(),
        ])?;
        self.start()
    }

    pub fn start(&self) -> Result<()> {
        let status = self.status()?;
        if !status.installed {
            bail!("service is not installed; run `simple-profiler service install`");
        }
        if !status.loaded {
            run_launchctl([
                OsStr::new("bootstrap"),
                self.domain().as_os_str(),
                self.paths.plist.as_os_str(),
            ])?;
            thread::sleep(Duration::from_millis(100));
        }
        if !self.status()?.running() {
            run_launchctl([OsStr::new("kickstart"), self.target().as_os_str()])?;
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let status = self.status()?;
        if status.loaded && status.running() {
            run_launchctl([
                OsStr::new("kill"),
                OsStr::new("SIGTERM"),
                self.target().as_os_str(),
            ])?;
            self.wait_until_stopped(Duration::from_secs(20))?;
        }
        Ok(())
    }

    pub fn restart(&self) -> Result<()> {
        self.stop()?;
        run_launchctl([OsStr::new("kickstart"), self.target().as_os_str()])?;
        Ok(())
    }

    pub fn status(&self) -> Result<ServiceStatus> {
        let installed = self.paths.plist.is_file();
        if !installed {
            return Ok(ServiceStatus {
                installed: false,
                loaded: false,
                state: None,
                pid: None,
                last_exit_code: None,
            });
        }

        let output = launchctl_output([OsStr::new("print"), self.target().as_os_str()])?;
        if !output.status.success() {
            return Ok(ServiceStatus {
                installed: true,
                loaded: false,
                state: None,
                pid: None,
                last_exit_code: None,
            });
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(parse_launchctl_status(&text, installed))
    }

    pub fn uninstall(&self, purge: bool) -> Result<()> {
        if self.status()?.loaded {
            run_launchctl([OsStr::new("bootout"), self.target().as_os_str()])?;
        }
        uninstall_files(&self.paths, purge)
    }

    fn domain(&self) -> PathBuf {
        PathBuf::from(format!("gui/{}", effective_uid()))
    }

    fn target(&self) -> PathBuf {
        PathBuf::from(format!("{}/{SERVICE_LABEL}", self.domain().display()))
    }

    fn wait_until_stopped(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while self.status()?.running() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        if self.status()?.running() {
            bail!("service did not stop within {} seconds", timeout.as_secs());
        }
        Ok(())
    }

    fn wait_until_unloaded(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while self.status()?.loaded && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        if self.status()?.loaded {
            bail!(
                "service did not unload within {} seconds",
                timeout.as_secs()
            );
        }
        Ok(())
    }
}

pub fn install_files(paths: &ServicePaths, source_binary: &Path) -> Result<()> {
    ensure_cli_launcher_replaceable(paths)?;
    fs::create_dir_all(paths.binary.parent().context("binary has no parent")?)?;
    fs::create_dir_all(paths.database.parent().context("database has no parent")?)?;
    fs::create_dir_all(&paths.logs)?;
    fs::create_dir_all(paths.plist.parent().context("plist has no parent")?)?;
    fs::create_dir_all(
        paths
            .cli_launcher
            .parent()
            .context("CLI launcher has no parent")?,
    )?;

    copy_binary_atomically(source_binary, &paths.binary)?;
    if !paths.config.exists() {
        let config = AppConfig {
            database_path: paths.database.clone(),
            logging: LoggingConfig {
                file: Some(paths.logs.join("simple-profiler.log")),
                ..LoggingConfig::default()
            },
            ..AppConfig::default()
        };
        atomic_write(&paths.config, toml::to_string_pretty(&config)?.as_bytes())?;
        set_mode(&paths.config, 0o600)?;
    }
    atomic_write(&paths.plist, render_plist(paths).as_bytes())?;
    set_mode(&paths.plist, 0o644)?;
    atomic_write(&paths.cli_launcher, render_cli_launcher(paths).as_bytes())?;
    set_mode(&paths.cli_launcher, 0o755)?;
    Ok(())
}

pub fn uninstall_files(paths: &ServicePaths, purge: bool) -> Result<()> {
    remove_managed_cli_launcher(paths)?;
    remove_file_if_present(&paths.plist)?;
    remove_file_if_present(&paths.binary)?;
    if purge {
        remove_dir_if_present(&paths.application_dir)?;
        remove_dir_if_present(&paths.logs)?;
    }
    Ok(())
}

pub fn render_cli_launcher(paths: &ServicePaths) -> String {
    format!(
        "{CLI_LAUNCHER_PREFIX}\nexec {} --config {} \"$@\"\n",
        shell_quote(&paths.binary),
        shell_quote(&paths.config),
    )
}

pub fn render_plist(paths: &ServicePaths) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--config</string>
        <string>{config}</string>
        <string>run</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{working_directory}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ExitTimeOut</key>
    <integer>20</integer>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>info</string>
    </dict>
</dict>
</plist>
"#,
        label = SERVICE_LABEL,
        binary = xml_escape(&paths.binary.to_string_lossy()),
        config = xml_escape(&paths.config.to_string_lossy()),
        working_directory = xml_escape(&paths.application_dir.to_string_lossy()),
        stdout = xml_escape(&paths.stdout_log().to_string_lossy()),
        stderr = xml_escape(&paths.stderr_log().to_string_lossy()),
    )
}

fn parse_launchctl_status(text: &str, installed: bool) -> ServiceStatus {
    let mut status = ServiceStatus {
        installed,
        loaded: true,
        state: None,
        pid: None,
        last_exit_code: None,
    };
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("state = ") {
            status.state = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("pid = ") {
            status.pid = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("last exit code = ") {
            status.last_exit_code = value.parse().ok();
        }
    }
    status
}

fn copy_binary_atomically(source: &Path, destination: &Path) -> Result<()> {
    let temporary = destination.with_extension("installing");
    fs::copy(source, &temporary).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            temporary.display()
        )
    })?;
    set_mode(&temporary, 0o755)?;
    fs::rename(&temporary, destination)?;
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, content)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn ensure_cli_launcher_replaceable(paths: &ServicePaths) -> Result<()> {
    let metadata = match fs::symlink_metadata(&paths.cli_launcher) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        if fs::read_link(&paths.cli_launcher)? == paths.binary {
            return Ok(());
        }
    } else if metadata.is_file() {
        let content = fs::read_to_string(&paths.cli_launcher)?;
        if is_managed_cli_launcher(&content) {
            return Ok(());
        }
    }
    bail!(
        "refusing to overwrite existing unmanaged CLI launcher at {}",
        paths.cli_launcher.display()
    )
}

fn remove_managed_cli_launcher(paths: &ServicePaths) -> Result<()> {
    let metadata = match fs::symlink_metadata(&paths.cli_launcher) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let managed = if metadata.file_type().is_symlink() {
        fs::read_link(&paths.cli_launcher)? == paths.binary
    } else if metadata.is_file() {
        fs::read_to_string(&paths.cli_launcher)
            .map(|content| is_managed_cli_launcher(&content))
            .unwrap_or(false)
    } else {
        false
    };
    if managed {
        remove_file_if_present(&paths.cli_launcher)?;
    }
    Ok(())
}

fn is_managed_cli_launcher(content: &str) -> bool {
    content.starts_with(CLI_LAUNCHER_PREFIX) || content == LEGACY_CLI_LAUNCHER
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_dir_if_present(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn run_launchctl<I, S>(arguments: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = launchctl_output(arguments)?;
    if !output.status.success() {
        bail!(
            "launchctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn launchctl_output<I, S>(arguments: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("/bin/launchctl")
        .args(arguments)
        .output()
        .context("failed to execute /bin/launchctl")
}

#[cfg(target_os = "macos")]
fn effective_uid() -> u32 {
    // SAFETY: geteuid takes no arguments and has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(not(target_os = "macos"))]
fn effective_uid() -> u32 {
    0
}

fn require_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("background service management is currently supported only on macOS")
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn renders_absolute_launch_agent_paths_and_escapes_xml() {
        let paths = ServicePaths::for_home(Path::new("/Users/Test & User"));
        let plist = render_plist(&paths);

        assert!(plist.contains("com.simple-profiler.agent"));
        assert!(plist.contains("/Users/Test &amp; User/Library/Application Support"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
        assert!(plist.contains("<key>ExitTimeOut</key>"));
    }

    #[test]
    fn renders_a_managed_cli_launcher_with_shell_quoted_paths() {
        let paths = ServicePaths::for_home(Path::new("/Users/Test's Home"));
        let launcher = render_cli_launcher(&paths);

        assert!(launcher.starts_with(CLI_LAUNCHER_PREFIX));
        assert!(launcher.contains("'/Users/Test'\"'\"'s Home/Library/Application Support"));
        assert!(launcher.contains("--config"));
        assert!(launcher.ends_with("\"$@\"\n"));
    }

    #[test]
    fn installs_files_without_overwriting_an_existing_config() {
        let directory = tempdir().expect("temp dir");
        let home = directory.path().join("home");
        let source = directory.path().join("source-binary");
        fs::write(&source, "binary-v1").expect("source binary");
        let paths = ServicePaths::for_home(&home);

        install_files(&paths, &source).expect("first install");
        fs::write(&paths.config, "custom = true\n").expect("custom config");
        fs::write(&source, "binary-v2").expect("updated source");
        install_files(&paths, &source).expect("second install");

        assert_eq!(
            fs::read_to_string(&paths.binary).expect("binary"),
            "binary-v2"
        );
        assert_eq!(
            fs::read_to_string(&paths.config).expect("config"),
            "custom = true\n"
        );
        assert_eq!(
            fs::metadata(&paths.config)
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            fs::read_to_string(&paths.cli_launcher)
                .expect("CLI launcher")
                .starts_with(CLI_LAUNCHER_PREFIX)
        );
        assert_eq!(
            fs::metadata(&paths.cli_launcher)
                .expect("launcher metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn refuses_to_overwrite_an_unmanaged_cli_launcher() {
        let directory = tempdir().expect("temp dir");
        let home = directory.path().join("home");
        let source = directory.path().join("source-binary");
        fs::write(&source, "binary-v1").expect("source binary");
        let paths = ServicePaths::for_home(&home);
        fs::create_dir_all(paths.cli_launcher.parent().expect("launcher parent"))
            .expect("launcher directory");
        fs::write(&paths.cli_launcher, "user-owned\n").expect("foreign launcher");

        let error = install_files(&paths, &source).expect_err("conflict must fail");

        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(
            fs::read_to_string(&paths.cli_launcher).expect("foreign launcher"),
            "user-owned\n"
        );
        assert!(!paths.binary.exists());
    }

    #[test]
    fn parses_running_launchctl_output() {
        let status = parse_launchctl_status(
            "state = running\n\tpid = 1234\n\tlast exit code = 2\n",
            true,
        );

        assert!(status.installed);
        assert!(status.loaded);
        assert!(status.running());
        assert_eq!(status.pid, Some(1234));
        assert_eq!(status.last_exit_code, Some(2));
    }

    #[test]
    fn uninstall_preserves_data_until_purge_is_requested() {
        let directory = tempdir().expect("temp dir");
        let home = directory.path().join("home");
        let source = directory.path().join("source-binary");
        fs::write(&source, "binary").expect("source binary");
        let paths = ServicePaths::for_home(&home);
        install_files(&paths, &source).expect("install");
        fs::write(&paths.database, "metrics").expect("database");
        fs::write(paths.logs.join("simple-profiler.log"), "log").expect("log");

        uninstall_files(&paths, false).expect("normal uninstall");
        assert!(!paths.cli_launcher.exists());
        assert!(!paths.binary.exists());
        assert!(!paths.plist.exists());
        assert!(paths.config.exists());
        assert!(paths.database.exists());
        assert!(paths.logs.exists());

        uninstall_files(&paths, true).expect("purge");
        assert!(!paths.application_dir.exists());
        assert!(!paths.logs.exists());
    }

    #[test]
    fn uninstall_preserves_an_unmanaged_cli_launcher() {
        let directory = tempdir().expect("temp dir");
        let paths = ServicePaths::for_home(directory.path());
        fs::create_dir_all(paths.cli_launcher.parent().expect("launcher parent"))
            .expect("launcher directory");
        fs::write(&paths.cli_launcher, "user-owned\n").expect("foreign launcher");

        uninstall_files(&paths, false).expect("uninstall");

        assert_eq!(
            fs::read_to_string(&paths.cli_launcher).expect("foreign launcher"),
            "user-owned\n"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn generated_plist_passes_plutil_validation() {
        let directory = tempdir().expect("temp dir");
        let paths = ServicePaths::for_home(directory.path());
        fs::create_dir_all(paths.plist.parent().expect("plist parent")).expect("launch agents");
        fs::write(&paths.plist, render_plist(&paths)).expect("plist");

        let status = Command::new("/usr/bin/plutil")
            .args(["-lint", "--"])
            .arg(&paths.plist)
            .status()
            .expect("plutil");
        assert!(status.success());
    }
}
