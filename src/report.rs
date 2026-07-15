use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Serialize;

use crate::{
    model::{CapabilityState, CollectorCapability},
    storage::{EventDetail, EventSummary},
};

pub const MAX_REPORT_RANGE_DAYS: i64 = 365;
pub const MAX_CHART_POINTS: i64 = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportResolution {
    Raw,
    Minute,
    QuarterHour,
}

impl ReportResolution {
    pub fn seconds(self) -> i64 {
        match self {
            Self::Raw => 5,
            Self::Minute => 60,
            Self::QuarterHour => 900,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Raw => "raw samples",
            Self::Minute => "1 minute",
            Self::QuarterHour => "15 minutes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReportRange {
    pub from_ms: i64,
    pub to_ms: i64,
}

impl ReportRange {
    pub fn new(from_ms: i64, to_ms: i64) -> Result<Self> {
        if from_ms >= to_ms {
            bail!("report start time must be earlier than end time");
        }
        if to_ms.saturating_sub(from_ms) > MAX_REPORT_RANGE_DAYS * 24 * 60 * 60 * 1_000 {
            bail!("report range must not exceed {MAX_REPORT_RANGE_DAYS} days");
        }
        Ok(Self { from_ms, to_ms })
    }

    pub fn duration_ms(self) -> i64 {
        self.to_ms.saturating_sub(self.from_ms)
    }

    pub fn preferred_resolution(self) -> ReportResolution {
        const TWO_HOURS_MS: i64 = 2 * 60 * 60 * 1_000;
        const ONE_DAY_MS: i64 = 24 * 60 * 60 * 1_000;
        if self.duration_ms() <= TWO_HOURS_MS {
            ReportResolution::Raw
        } else if self.duration_ms() <= ONE_DAY_MS {
            ReportResolution::Minute
        } else {
            ReportResolution::QuarterHour
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportPoint {
    pub timestamp_ms: i64,
    pub sample_count: i64,
    pub min_value: f64,
    pub max_value: f64,
    pub average_value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportSeries {
    pub collector: String,
    pub resource: String,
    pub metric_name: String,
    pub unit: String,
    pub sample_count: i64,
    pub min_value: f64,
    pub max_value: f64,
    pub average_value: f64,
    pub points: Vec<ReportPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportProcessSummary {
    pub pid: u32,
    pub process_start_time_seconds: u64,
    pub name: String,
    pub peak_cpu_percent: f64,
    pub peak_memory_bytes: u64,
    pub peak_disk_read_bytes_per_second: f64,
    pub peak_disk_write_bytes_per_second: f64,
    pub peak_network_receive_bytes_per_second: Option<f64>,
    pub peak_network_transmit_bytes_per_second: Option<f64>,
    pub peak_gpu_usage_percent: Option<f64>,
    pub sample_count: i64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardProcessPoint {
    pub timestamp_ms: i64,
    pub average_cpu_percent: f64,
    pub peak_cpu_percent: f64,
    pub average_memory_bytes: f64,
    pub peak_memory_bytes: u64,
    pub average_disk_read_bytes_per_second: f64,
    pub peak_disk_read_bytes_per_second: f64,
    pub average_disk_write_bytes_per_second: f64,
    pub peak_disk_write_bytes_per_second: f64,
    pub average_network_receive_bytes_per_second: Option<f64>,
    pub peak_network_receive_bytes_per_second: Option<f64>,
    pub average_network_transmit_bytes_per_second: Option<f64>,
    pub peak_network_transmit_bytes_per_second: Option<f64>,
    pub average_gpu_usage_percent: Option<f64>,
    pub peak_gpu_usage_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardProcessSeries {
    pub pid: u32,
    pub process_start_time_seconds: u64,
    pub name: String,
    pub cpu_rank: Option<u8>,
    pub memory_rank: Option<u8>,
    pub disk_read_rank: Option<u8>,
    pub disk_write_rank: Option<u8>,
    pub network_receive_rank: Option<u8>,
    pub network_transmit_rank: Option<u8>,
    pub gpu_rank: Option<u8>,
    pub points: Vec<DashboardProcessPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportData {
    pub generated_at_ms: i64,
    pub range: ReportRange,
    pub resolution: ReportResolution,
    pub bucket_span_ms: i64,
    pub metric_oldest_ms: Option<i64>,
    pub metric_newest_ms: Option<i64>,
    pub process_oldest_ms: Option<i64>,
    pub process_newest_ms: Option<i64>,
    pub series: Vec<ReportSeries>,
    pub events: Vec<EventDetail>,
    pub events_truncated: bool,
    pub processes: Vec<ReportProcessSummary>,
    pub capabilities: Vec<CollectorCapability>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardSnapshot {
    pub generated_at_ms: i64,
    pub range: ReportRange,
    pub resolution: ReportResolution,
    pub bucket_span_ms: i64,
    pub metric_oldest_ms: Option<i64>,
    pub metric_newest_ms: Option<i64>,
    pub process_oldest_ms: Option<i64>,
    pub process_newest_ms: Option<i64>,
    pub series: Vec<ReportSeries>,
    pub events: Vec<EventSummary>,
    pub events_truncated: bool,
    pub processes: Vec<ReportProcessSummary>,
    pub process_bucket_span_ms: i64,
    pub system_memory_bytes: Option<u64>,
    pub process_series: Vec<DashboardProcessSeries>,
}

pub fn resolve_range(
    last: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    now_ms: i64,
) -> Result<ReportRange> {
    if last.is_some() && (from.is_some() || to.is_some()) {
        bail!("last cannot be combined with from or to");
    }
    match (from, to) {
        (Some(from), Some(to)) => {
            ReportRange::new(parse_rfc3339_millis(from)?, parse_rfc3339_millis(to)?)
        }
        (None, None) => {
            let duration = parse_relative_duration(last.unwrap_or("1h"))?;
            let duration_ms = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
            ReportRange::new(now_ms.saturating_sub(duration_ms), now_ms)
        }
        _ => bail!("from and to must be provided together"),
    }
}

pub fn parse_relative_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .context("duration must use a number followed by m, h, or d")?;
    let (amount, unit) = value.split_at(split);
    if amount.is_empty() || unit.len() != 1 {
        bail!("duration must use a number followed by m, h, or d");
    }
    let amount: u64 = amount.parse().context("duration amount is invalid")?;
    if amount == 0 {
        bail!("duration must be greater than zero");
    }
    let seconds = match unit {
        "m" => amount.checked_mul(60),
        "h" => amount.checked_mul(60 * 60),
        "d" => amount.checked_mul(24 * 60 * 60),
        _ => None,
    }
    .context("duration unit must be m, h, or d")?;
    let duration = Duration::from_secs(seconds);
    if duration < Duration::from_secs(60)
        || duration > Duration::from_secs((MAX_REPORT_RANGE_DAYS * 24 * 60 * 60) as u64)
    {
        bail!("duration must be between 1 minute and {MAX_REPORT_RANGE_DAYS} days");
    }
    Ok(duration)
}

pub fn parse_rfc3339_millis(value: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid RFC 3339 timestamp {value:?}"))?
        .timestamp_millis())
}

pub fn default_output_path(now: DateTime<Utc>) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join("Documents")
        .join("SimpleProfiler Reports")
        .join(format!(
            "simple-profiler-report-{}.html",
            now.format("%Y%m%d-%H%M%S")
        )))
}

pub fn write_html_atomically(path: &Path, html: &str) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create report directory {}", parent.display()))?;
    }
    let temporary = path.with_extension("html.tmp");
    fs::write(&temporary, html)
        .with_context(|| format!("failed to write report {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to finalize report {}", path.display()))?;
    Ok(())
}

pub fn open_report(path: &Path) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("--open is currently supported only on macOS");
    }
    let status = Command::new("/usr/bin/open")
        .arg(path)
        .status()
        .context("failed to launch /usr/bin/open")?;
    if !status.success() {
        bail!("could not open report {}", path.display());
    }
    Ok(())
}

pub fn render_html(data: &ReportData) -> String {
    let mut html = String::with_capacity(128 * 1_024);
    html.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    html.push_str("<title>Simple Profiler Diagnostic Report</title><style>");
    html.push_str(REPORT_CSS);
    html.push_str("</style></head><body><main>");
    html.push_str("<header><p class=\"eyebrow\">SIMPLE PROFILER</p>");
    html.push_str("<h1>System diagnostic report</h1>");
    let _ = write!(
        html,
        "<p class=\"muted\">{} → {} · generated {}</p></header>",
        format_time(data.range.from_ms),
        format_time(data.range.to_ms),
        format_time(data.generated_at_ms)
    );
    render_overview(data, &mut html);
    render_capabilities(data, &mut html);
    render_series(data, &mut html);
    render_events(data, &mut html);
    render_processes(data, &mut html);
    html.push_str("<footer>Local-only report. Command lines, environments, and working directories are not collected.</footer>");
    html.push_str("</main></body></html>");
    html
}

fn render_capabilities(data: &ReportData, html: &mut String) {
    html.push_str("<section><h2>Collector capabilities</h2>");
    if data.capabilities.is_empty() {
        html.push_str("<p class=\"empty\">No collector capability status has been recorded yet.</p></section>");
        return;
    }
    html.push_str("<table><thead><tr><th>Resource</th><th>Capability</th><th>State</th><th>Provider</th><th>Detail</th></tr></thead><tbody>");
    for capability in &data.capabilities {
        let state = match capability.state {
            CapabilityState::Available => "available",
            CapabilityState::Degraded => "degraded",
            CapabilityState::Unavailable => "unavailable",
        };
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape_html(&capability.resource),
            escape_html(&metric_label(&capability.capability)),
            state,
            escape_html(&capability.provider),
            escape_html(capability.detail.as_deref().unwrap_or("—")),
        );
    }
    html.push_str("</tbody></table></section>");
}

fn render_overview(data: &ReportData, html: &mut String) {
    html.push_str("<section><h2>Overview</h2><div class=\"cards\">");
    summary_card(html, "Resolution", data.resolution.label());
    summary_card(html, "Metric series", &data.series.len().to_string());
    summary_card(html, "Anomaly events", &data.events.len().to_string());
    summary_card(html, "Processes", &data.processes.len().to_string());
    html.push_str("</div>");
    let _ = write!(
        html,
        "<p class=\"notice\">Metric coverage: {} → {}. Process coverage: {} → {}. Chart bucket: {}.</p>",
        format_optional_time(data.metric_oldest_ms),
        format_optional_time(data.metric_newest_ms),
        format_optional_time(data.process_oldest_ms),
        format_optional_time(data.process_newest_ms),
        format_duration_ms(data.bucket_span_ms)
    );
    html.push_str("</section>");
}

fn summary_card(html: &mut String, label: &str, value: &str) {
    let _ = write!(
        html,
        "<article class=\"card\"><span>{}</span><strong>{}</strong></article>",
        escape_html(label),
        escape_html(value)
    );
}

fn render_series(data: &ReportData, html: &mut String) {
    html.push_str("<section><h2>Resource trends</h2>");
    if data.series.is_empty() {
        html.push_str("<p class=\"empty\">No metric data is available in this range.</p>");
    }
    for series in &data.series {
        let resource = if series.resource.is_empty() {
            "system"
        } else {
            &series.resource
        };
        let _ = write!(
            html,
            "<article class=\"panel\"><div class=\"panel-head\"><div><h3>{}</h3><p>{}</p></div><div class=\"stats\"><b>max {}</b><span>avg {}</span></div></div>",
            escape_html(&metric_label(&series.metric_name)),
            escape_html(resource),
            format_value(series.max_value, &series.unit),
            format_value(series.average_value, &series.unit),
        );
        render_chart(series, data.range, html);
        html.push_str("</article>");
    }
    html.push_str("</section>");
}

fn render_chart(series: &ReportSeries, range: ReportRange, html: &mut String) {
    const WIDTH: f64 = 900.0;
    const HEIGHT: f64 = 180.0;
    const PADDING: f64 = 16.0;
    if series.points.is_empty() {
        html.push_str("<p class=\"empty\">No chart points.</p>");
        return;
    }
    let lower = if series.unit == "percent" {
        0.0
    } else {
        series.min_value.min(0.0)
    };
    let upper = if series.unit == "percent" {
        100.0_f64.max(series.max_value)
    } else {
        series.max_value.max(lower + 1.0)
    };
    let duration = range.duration_ms().max(1) as f64;
    let value_span = (upper - lower).max(f64::EPSILON);
    let mut points = String::new();
    for point in &series.points {
        let x = PADDING
            + ((point.timestamp_ms - range.from_ms) as f64 / duration).clamp(0.0, 1.0)
                * (WIDTH - PADDING * 2.0);
        let y = HEIGHT
            - PADDING
            - ((point.average_value - lower) / value_span).clamp(0.0, 1.0)
                * (HEIGHT - PADDING * 2.0);
        let _ = write!(points, "{x:.1},{y:.1} ");
    }
    let _ = write!(
        html,
        "<svg class=\"chart\" viewBox=\"0 0 900 180\" role=\"img\" aria-label=\"{} trend\"><line x1=\"16\" y1=\"164\" x2=\"884\" y2=\"164\"/><polyline points=\"{}\"/></svg>",
        escape_html(&metric_label(&series.metric_name)),
        points
    );
}

fn render_events(data: &ReportData, html: &mut String) {
    html.push_str("<section><h2>Anomaly timeline</h2>");
    if data.events.is_empty() {
        html.push_str("<p class=\"empty\">No anomaly events overlap this range.</p>");
    }
    if data.events_truncated {
        html.push_str("<p class=\"notice\">Only the newest 200 overlapping events are shown.</p>");
    }
    for event in &data.events {
        let resource = if event.summary.resource.is_empty() {
            "system"
        } else {
            &event.summary.resource
        };
        let _ = write!(
            html,
            "<article class=\"event {}\"><div class=\"panel-head\"><div><h3>#{} {} · {}</h3><p>{} → {}</p></div><div class=\"stats\"><b>peak {:.2} {}</b><span>{}</span></div></div>",
            escape_html(&event.summary.severity),
            event.summary.id,
            escape_html(&metric_label(&event.summary.metric_name)),
            escape_html(resource),
            format_time(event.summary.started_at_ms),
            format_optional_time(event.summary.ended_at_ms),
            event.summary.peak_value,
            escape_html(&event.unit),
            escape_html(&event.summary.status),
        );
        let _ = write!(
            html,
            "<p class=\"muted\">thresholds: warning {:.2}, critical {:.2}, recovery {:.2} · samples {} · gaps {}</p>",
            event.warning_threshold,
            event.critical_threshold,
            event.recovery_threshold,
            event.sample_count,
            event.data_gap_count,
        );
        if !event.evidence.is_empty() {
            html.push_str("<table><thead><tr><th>Metric checkpoint</th><th>Value</th><th>Kind</th></tr></thead><tbody>");
            for evidence in event.evidence.iter().take(50) {
                let _ = write!(
                    html,
                    "<tr><td>{}</td><td>{:.2} {}</td><td>{}</td></tr>",
                    format_time(evidence.collected_at_ms),
                    evidence.value,
                    escape_html(&event.unit),
                    escape_html(&evidence.kind),
                );
            }
            html.push_str("</tbody></table>");
            if event.evidence.len() > 50 {
                html.push_str(
                    "<p class=\"muted\">Only the first 50 metric checkpoints are shown.</p>",
                );
            }
        }
        if !event.process_evidence.is_empty() {
            html.push_str("<table><thead><tr><th>Checkpoint</th><th>Process</th><th>CPU</th><th>Memory</th></tr></thead><tbody>");
            for evidence in event.process_evidence.iter().take(50) {
                let _ = write!(
                    html,
                    "<tr><td>{} {}</td><td>{} (PID {})</td><td>{:.2}%</td><td>{}</td></tr>",
                    escape_html(&evidence.kind),
                    format_time(evidence.sample.collected_at_ms),
                    escape_html(&evidence.sample.name),
                    evidence.sample.pid,
                    evidence.sample.cpu_usage_percent,
                    format_bytes(evidence.sample.memory_bytes),
                );
            }
            html.push_str("</tbody></table>");
            if event.process_evidence.len() > 50 {
                html.push_str(
                    "<p class=\"muted\">Only the first 50 related-process rows are shown.</p>",
                );
            }
        }
        html.push_str("</article>");
    }
    html.push_str("</section>");
}

fn render_processes(data: &ReportData, html: &mut String) {
    html.push_str("<section><h2>Resource-heavy processes</h2>");
    if data.processes.is_empty() {
        html.push_str("<p class=\"empty\">No retained process snapshots overlap this range.</p>");
    } else {
        html.push_str("<table><thead><tr><th>Process</th><th>PID</th><th>Peak CPU</th><th>Peak memory</th><th>Peak disk read/write</th><th>Peak network in/out</th><th>Peak GPU</th><th>Observed</th></tr></thead><tbody>");
        for process in &data.processes {
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{}</td><td>{:.2}%</td><td>{}</td><td>{} / {}</td><td>{} / {}</td><td>{}</td><td>{}</td></tr>",
                escape_html(&process.name),
                process.pid,
                process.peak_cpu_percent,
                format_bytes(process.peak_memory_bytes),
                format_rate(process.peak_disk_read_bytes_per_second),
                format_rate(process.peak_disk_write_bytes_per_second),
                process
                    .peak_network_receive_bytes_per_second
                    .map_or_else(|| "n/a".to_owned(), format_rate),
                process
                    .peak_network_transmit_bytes_per_second
                    .map_or_else(|| "n/a".to_owned(), format_rate),
                process
                    .peak_gpu_usage_percent
                    .map_or_else(|| "n/a".to_owned(), |value| format!("{value:.2}%")),
                process.sample_count,
            );
        }
        html.push_str("</tbody></table>");
    }
    html.push_str("</section>");
}

fn metric_label(name: &str) -> String {
    match name {
        "cpu.total.usage" => "CPU usage".to_owned(),
        "memory.usage" => "Memory usage".to_owned(),
        "disk.space.usage" => "Disk space usage".to_owned(),
        "disk.io.read.rate" => "Disk read rate".to_owned(),
        "disk.io.write.rate" => "Disk write rate".to_owned(),
        "network.receive.rate" => "Network receive rate".to_owned(),
        "network.transmit.rate" => "Network transmit rate".to_owned(),
        "gpu.device.usage" => "GPU usage".to_owned(),
        "gpu.renderer.usage" => "GPU renderer usage".to_owned(),
        "gpu.tiler.usage" => "GPU tiler usage".to_owned(),
        "gpu.memory.used" => "GPU memory in use".to_owned(),
        "gpu.memory.allocated" => "GPU allocated memory".to_owned(),
        "gpu.memory.total" => "GPU memory total".to_owned(),
        "gpu.device.identity" => "GPU identity".to_owned(),
        "gpu.power" => "GPU power".to_owned(),
        "gpu.temperature" => "GPU temperature".to_owned(),
        _ => name.to_owned(),
    }
}

fn format_value(value: f64, unit: &str) -> String {
    match unit {
        "percent" => format!("{value:.2}%"),
        "bytes" => format_bytes(value.max(0.0) as u64),
        "bytes_per_second" => format!("{}/s", format_bytes(value.max(0.0) as u64)),
        _ => format!("{value:.2} {}", escape_html(unit)),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1_024.0;
    const MIB: f64 = KIB * 1_024.0;
    const GIB: f64 = MIB * 1_024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn format_rate(bytes_per_second: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_second.max(0.0) as u64))
}

fn format_duration_ms(milliseconds: i64) -> String {
    if milliseconds % (60 * 1_000) == 0 {
        format!("{} min", milliseconds / (60 * 1_000))
    } else {
        format!("{} sec", milliseconds / 1_000)
    }
}

fn format_optional_time(milliseconds: Option<i64>) -> String {
    milliseconds.map_or_else(|| "no data".to_owned(), format_time)
}

fn format_time(milliseconds: i64) -> String {
    Local
        .timestamp_millis_opt(milliseconds)
        .single()
        .map_or_else(|| milliseconds.to_string(), |time| time.to_rfc3339())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const REPORT_CSS: &str = r#"
:root{color-scheme:light;--bg:#f4f7fb;--panel:#fff;--ink:#172033;--muted:#667085;--line:#dfe5ee;--accent:#246bfd;--warning:#f59e0b;--critical:#dc2626}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}main{max-width:1120px;margin:auto;padding:48px 24px}header{margin-bottom:34px}.eyebrow{color:var(--accent);font-size:12px;font-weight:800;letter-spacing:.14em}h1{font-size:38px;line-height:1.1;margin:8px 0}h2{font-size:22px;margin:38px 0 16px}h3{margin:0;font-size:16px}.muted,.panel-head p{color:var(--muted)}.cards{display:grid;grid-template-columns:repeat(4,1fr);gap:12px}.card,.panel,.event{background:var(--panel);border:1px solid var(--line);border-radius:14px;box-shadow:0 5px 18px #1720330a}.card{padding:16px}.card span{display:block;color:var(--muted)}.card strong{display:block;font-size:22px;margin-top:7px}.panel,.event{padding:18px;margin:12px 0}.panel-head{display:flex;align-items:flex-start;justify-content:space-between;gap:20px}.panel-head p{margin:4px 0}.stats{text-align:right}.stats b,.stats span{display:block}.stats span{color:var(--muted)}.chart{display:block;width:100%;height:180px;margin-top:14px;background:linear-gradient(#fff,#fbfcff);border-radius:10px}.chart line{stroke:var(--line);stroke-width:1}.chart polyline{fill:none;stroke:var(--accent);stroke-width:2.5;stroke-linecap:round;stroke-linejoin:round}.notice,.empty{padding:12px 14px;background:#eef4ff;border-radius:10px;color:#344054}.event.warning{border-left:4px solid var(--warning)}.event.critical{border-left:4px solid var(--critical)}table{width:100%;border-collapse:collapse;margin-top:14px}th,td{padding:9px 10px;border-bottom:1px solid var(--line);text-align:left;font-size:13px}th{color:var(--muted);font-weight:600}footer{margin-top:42px;padding-top:20px;border-top:1px solid var(--line);color:var(--muted);font-size:12px}@media(max-width:760px){main{padding:28px 14px}.cards{grid-template-columns:repeat(2,1fr)}.panel-head{display:block}.stats{text-align:left;margin-top:8px}table{display:block;overflow-x:auto}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_report_durations() {
        assert_eq!(
            parse_relative_duration("15m").expect("minutes").as_secs(),
            900
        );
        assert_eq!(
            parse_relative_duration("24h").expect("hours").as_secs(),
            86_400
        );
        assert_eq!(
            parse_relative_duration("7d").expect("days").as_secs(),
            604_800
        );
        assert!(parse_relative_duration("0h").is_err());
        assert!(parse_relative_duration("1w").is_err());
    }

    #[test]
    fn selects_resolution_from_requested_duration() {
        assert_eq!(
            ReportRange::new(0, 2 * 60 * 60 * 1_000)
                .expect("range")
                .preferred_resolution(),
            ReportResolution::Raw
        );
        assert_eq!(
            ReportRange::new(0, 24 * 60 * 60 * 1_000)
                .expect("range")
                .preferred_resolution(),
            ReportResolution::Minute
        );
        assert_eq!(
            ReportRange::new(0, 7 * 24 * 60 * 60 * 1_000)
                .expect("range")
                .preferred_resolution(),
            ReportResolution::QuarterHour
        );
    }

    #[test]
    fn resolves_relative_and_explicit_ranges() {
        let relative = resolve_range(Some("1h"), None, None, 7_200_000).expect("relative");
        assert_eq!(relative, ReportRange::new(3_600_000, 7_200_000).unwrap());

        let explicit = resolve_range(
            None,
            Some("2026-07-15T00:00:00Z"),
            Some("2026-07-15T01:00:00Z"),
            0,
        )
        .expect("explicit");
        assert_eq!(explicit.duration_ms(), 3_600_000);
        assert!(resolve_range(Some("1h"), Some("2026-07-15T00:00:00Z"), None, 0).is_err());
    }

    #[test]
    fn escapes_untrusted_report_text() {
        assert_eq!(
            escape_html("<script>'&\""),
            "&lt;script&gt;&#39;&amp;&quot;"
        );
    }

    #[test]
    fn writes_reports_atomically_and_creates_parent_directories() {
        let directory = tempfile::tempdir().expect("temp dir");
        let output = directory.path().join("nested").join("report.html");

        write_html_atomically(&output, "<html>done</html>").expect("write report");

        assert_eq!(
            fs::read_to_string(&output).expect("report"),
            "<html>done</html>"
        );
        assert!(!output.with_extension("html.tmp").exists());
    }
}
