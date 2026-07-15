use std::{collections::BTreeMap, path::PathBuf, process::Command};

use anyhow::{Context, Result, bail};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct GpuTotal {
    pid: u32,
    gpu_usage_percent: f64,
}

#[derive(Debug, Serialize)]
struct GpuSnapshot {
    collected_at_ms: i64,
    processes: Vec<GpuTotal>,
}

fn main() -> Result<()> {
    // SAFETY: `geteuid` has no arguments, does not dereference memory, and only reads process state.
    if unsafe { libc::geteuid() } != 0 {
        bail!("simple-profiler-gpu-helper must run as root");
    }
    let output_path = helper_output_path()?;
    let output = Command::new("/usr/bin/powermetrics")
        .args(["-n", "1", "--show-process-gpu", "--format", "text"])
        .output()
        .context("failed to run /usr/bin/powermetrics")?;
    if !output.status.success() {
        bail!(
            "powermetrics exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let snapshot = GpuSnapshot {
        collected_at_ms: chrono::Utc::now().timestamp_millis(),
        processes: parse_powermetrics(&output.stdout)?,
    };
    write_snapshot(&output_path, &serde_json::to_vec(&snapshot)?)?;
    Ok(())
}

fn helper_output_path() -> Result<PathBuf> {
    let mut arguments = std::env::args_os().skip(1);
    match (arguments.next(), arguments.next(), arguments.next()) {
        (None, None, None) => Ok(PathBuf::from("/var/run/simple-profiler/process-gpu.json")),
        (Some(flag), Some(path), None) if flag == "--output" => Ok(PathBuf::from(path)),
        _ => bail!("usage: simple-profiler-gpu-helper [--output PATH]"),
    }
}

fn write_snapshot(path: &std::path::Path, source: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let parent = path.parent().context("GPU snapshot path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755))?;
    let temporary = parent.join(format!(".process-gpu-{}.tmp", std::process::id()));
    std::fs::write(&temporary, source)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn parse_powermetrics(source: &[u8]) -> Result<Vec<GpuTotal>> {
    let source = std::str::from_utf8(source).context("powermetrics output was not UTF-8")?;
    let mut lines = source.lines();
    let header = lines
        .find(|line| line.trim_start().starts_with("Name") && line.contains("GPU ms/s"))
        .context("powermetrics returned no per-process GPU column")?;
    if !header.contains("CPU ms/s") || !header.contains("User%") {
        bail!("powermetrics process table had an unsupported column layout");
    }

    let mut totals = BTreeMap::new();
    for line in lines.take_while(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        // The text table ends with PID, CPU, user, two deadline, two wakeup, and GPU fields.
        // Reading that fixed numeric tail avoids treating spaces in process names as separators.
        if fields.len() < 8 {
            continue;
        }
        let tail = &fields[fields.len() - 8..];
        let Ok(pid) = tail[0].parse::<u32>() else {
            continue;
        };
        let Ok(gpu_ms_per_second) = tail[7].parse::<f64>() else {
            continue;
        };
        if !gpu_ms_per_second.is_finite() || gpu_ms_per_second <= 0.0 {
            continue;
        }
        let gpu_usage_percent = (gpu_ms_per_second / 10.0).clamp(0.0, 100.0);
        totals
            .entry(pid)
            .and_modify(|total: &mut f64| *total = total.max(gpu_usage_percent))
            .or_insert(gpu_usage_percent);
    }

    Ok(totals
        .into_iter()
        .map(|(pid, gpu_usage_percent)| GpuTotal {
            pid,
            gpu_usage_percent,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_text_table_and_process_names_with_spaces() {
        let source = br#"*** Running tasks ***

Name                               ID     CPU ms/s  User%  Deadlines (<2 ms, 2-5 ms)  Wakeups (Intr, Pkg idle)  GPU ms/s
Google Chrome Helper (Renderer)    42     9.00      50.00  0.00    0.00               1.00    0.00              125.50
DEAD_TASKS                         -1     1.00      50.00  0.00    0.00               1.00    0.00              99.00

**** Processor usage ****
"#;
        let totals = parse_powermetrics(source).expect("text table");
        assert_eq!(totals[0].pid, 42);
        assert_eq!(totals[0].gpu_usage_percent, 12.55);
    }

    #[test]
    fn accepts_an_idle_sample_as_available() {
        let source = br#"Name ID CPU ms/s User% GPU ms/s
idle 7 0.00 0.00 0.00 0.00 0.00 0.00 0.00

"#;
        assert!(parse_powermetrics(source).expect("idle table").is_empty());
    }
}
