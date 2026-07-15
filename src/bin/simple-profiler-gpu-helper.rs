use std::{collections::BTreeMap, io::Cursor, path::PathBuf, process::Command};

use anyhow::{Context, Result, bail};
use plist::Value;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct GpuTotal {
    pid: u32,
    gpu_time_ns: u64,
}

#[derive(Debug, Serialize)]
struct GpuSnapshot {
    collected_at_ms: i64,
    processes: Vec<GpuTotal>,
}

fn main() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("simple-profiler-gpu-helper must run as root");
    }
    let output_path = helper_output_path()?;
    let output = Command::new("/usr/bin/powermetrics")
        .args(["-n", "1", "--show-process-gpu", "--format", "plist"])
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
    let mut totals = BTreeMap::new();
    let mut parsed = false;
    for document in source
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let value = Value::from_reader(Cursor::new(document))
            .context("failed to parse powermetrics property list")?;
        parsed = true;
        collect_process_gpu(&value, &mut totals);
    }
    if !parsed {
        bail!("powermetrics returned no property list");
    }
    Ok(totals
        .into_iter()
        .map(|(pid, gpu_time_ns)| GpuTotal { pid, gpu_time_ns })
        .collect())
}

fn collect_process_gpu(value: &Value, totals: &mut BTreeMap<u32, u64>) {
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_process_gpu(value, totals)),
        Value::Dictionary(dictionary) => {
            let pid = dictionary.iter().find_map(|(key, value)| {
                let key = normalize_key(key);
                matches!(key.as_str(), "pid" | "processid" | "processidentifier")
                    .then(|| {
                        numeric_value(value).and_then(|value| u32::try_from(value as u64).ok())
                    })
                    .flatten()
            });
            let gpu_time = dictionary.iter().find_map(|(key, value)| {
                let normalized = normalize_key(key);
                (normalized.contains("gpu")
                    && (normalized.contains("time") || normalized.contains("runtime")))
                .then(|| time_as_ns(key, value))
                .flatten()
            });
            if let (Some(pid), Some(gpu_time_ns)) = (pid, gpu_time) {
                totals
                    .entry(pid)
                    .and_modify(|total| *total = (*total).max(gpu_time_ns))
                    .or_insert(gpu_time_ns);
            }
            dictionary
                .values()
                .for_each(|value| collect_process_gpu(value, totals));
        }
        _ => {}
    }
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(value) => value.as_unsigned().map(|value| value as f64),
        Value::Real(value) => Some(*value),
        _ => None,
    }
}

fn time_as_ns(key: &str, value: &Value) -> Option<u64> {
    let value = numeric_value(value)?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let key = normalize_key(key);
    let multiplier = if key.ends_with("ns") || key.contains("nanosecond") {
        1.0
    } else if key.ends_with("us") || key.contains("microsecond") {
        1_000.0
    } else if key.ends_with("ms") || key.contains("millisecond") {
        1_000_000.0
    } else {
        1_000_000_000.0
    };
    Some((value * multiplier).min(u64::MAX as f64).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_process_gpu_time_with_explicit_units() {
        let source = br#"<?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0"><dict><key>processes</key><array><dict>
        <key>pid</key><integer>42</integer><key>gpu_time_ms</key><real>1.5</real>
        </dict></array></dict></plist>"#;
        let totals = parse_powermetrics(source).expect("plist");
        assert_eq!(totals[0].pid, 42);
        assert_eq!(totals[0].gpu_time_ns, 1_500_000);
    }
}
