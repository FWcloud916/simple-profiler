(() => {
  "use strict";

  const state = {
    range: { last: "1h" },
    snapshot: null,
    status: null,
    processSort: "cpu",
    refreshing: false,
    timer: null,
  };

  const elements = {
    badge: document.querySelector("#connectionBadge"),
    refresh: document.querySelector("#refreshButton"),
    presets: document.querySelector("#presetGroup"),
    custom: document.querySelector("#customRange"),
    customForm: document.querySelector("#customRangeForm"),
    from: document.querySelector("#fromInput"),
    to: document.querySelector("#toInput"),
    autoRefresh: document.querySelector("#autoRefresh"),
    cards: document.querySelector("#summaryCards"),
    coverage: document.querySelector("#coverageNotice"),
    rangeLabel: document.querySelector("#rangeLabel"),
    resolution: document.querySelector("#resolutionLabel"),
    charts: document.querySelector("#chartGrid"),
    eventCount: document.querySelector("#eventCount"),
    events: document.querySelector("#eventList"),
    storage: document.querySelector("#storagePanel"),
    processRows: document.querySelector("#processRows"),
    processEmpty: document.querySelector("#processEmpty"),
    sortCpu: document.querySelector("#sortCpu"),
    sortMemory: document.querySelector("#sortMemory"),
    dialog: document.querySelector("#eventDialog"),
    dialogTitle: document.querySelector("#dialogTitle"),
    dialogBody: document.querySelector("#dialogBody"),
    closeDialog: document.querySelector("#closeDialog"),
    toast: document.querySelector("#toast"),
  };

  function apiUrl(path, params) {
    const url = new URL(path, window.location.href);
    if (params) {
      Object.entries(params).forEach(([key, value]) => {
        if (value != null) url.searchParams.set(key, value);
      });
    }
    return url;
  }

  async function requestJson(path, params) {
    const response = await fetch(apiUrl(path, params), {
      headers: { Accept: "application/json" },
      cache: "no-store",
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(body.error || `Request failed (${response.status})`);
    return body;
  }

  async function refresh() {
    if (state.refreshing) return;
    state.refreshing = true;
    elements.refresh.disabled = true;
    setConnection("loading", "Refreshing");
    try {
      const [snapshot, storageStatus] = await Promise.all([
        requestJson("api/v1/snapshot", state.range),
        requestJson("api/v1/status"),
      ]);
      state.snapshot = snapshot;
      state.status = storageStatus;
      render();
      setConnection("healthy", freshnessLabel(snapshot.metric_newest_ms));
    } catch (error) {
      setConnection("error", "Unavailable");
      showToast(error.message);
      renderError(error.message);
    } finally {
      state.refreshing = false;
      elements.refresh.disabled = false;
      scheduleRefresh();
    }
  }

  function render() {
    renderOverview();
    renderCharts();
    renderEvents();
    renderStorage();
    renderProcesses();
  }

  function renderOverview() {
    const snapshot = state.snapshot;
    const status = state.status;
    const cpu = findSeries("cpu.total.usage");
    const memory = findSeries("memory.usage");
    const gpu = findSeries("gpu.device.usage");
    const gpuCapability = findCapability("gpu.device.usage");
    const disks = findAllSeries("disk.space.usage");
    const maxDisk = disks.length ? Math.max(...disks.map((series) => series.max_value)) : null;
    const openEvents = snapshot.events.filter((event) => event.status === "open");
    const critical = openEvents.filter((event) => event.severity === "critical").length;
    const cards = [
      {
        label: "CPU usage",
        value: cpu ? formatValue(lastValue(cpu), cpu.unit) : "No data",
        detail: cpu ? `Peak ${formatValue(cpu.max_value, cpu.unit)}` : "No retained samples",
        state: valueState(cpu?.max_value, 90, 97),
      },
      {
        label: "Memory usage",
        value: memory ? formatValue(lastValue(memory), memory.unit) : "No data",
        detail: memory ? `Peak ${formatValue(memory.max_value, memory.unit)}` : "No retained samples",
        state: valueState(memory?.max_value, 90, 95),
      },
      {
        label: "GPU usage",
        value: gpu ? formatValue(lastValue(gpu), gpu.unit) : gpuCapability ? capabilityLabel(gpuCapability.state) : "No data",
        detail: gpu ? `Peak ${formatValue(gpu.max_value, gpu.unit)}` : gpuCapability?.detail || "No retained samples",
        state: gpu && gpuCapability?.state !== "degraded" ? "healthy" : "warning",
      },
      {
        label: "Highest disk usage",
        value: maxDisk == null ? "No data" : formatValue(maxDisk, "percent"),
        detail: disks.length ? `${disks.length} mount point${disks.length === 1 ? "" : "s"}` : "No retained samples",
        state: valueState(maxDisk, 90, 95),
      },
      {
        label: "Open anomalies",
        value: String(openEvents.length),
        detail: critical ? `${critical} critical` : `${status.open_warning_count} warning · no critical`,
        state: critical ? "error" : openEvents.length ? "warning" : "healthy",
      },
    ];
    elements.cards.replaceChildren(...cards.map(summaryCard));
    elements.rangeLabel.textContent = `${formatTime(snapshot.range.from_ms)} → ${formatTime(snapshot.range.to_ms)}`;
    elements.resolution.textContent = `${resolutionLabel(snapshot.resolution)} · ${formatDuration(snapshot.bucket_span_ms)} chart bucket`;
    const metricCoverage = coverageText(snapshot.metric_oldest_ms, snapshot.metric_newest_ms);
    const processCoverage = coverageText(snapshot.process_oldest_ms, snapshot.process_newest_ms);
    elements.coverage.replaceChildren(
      textNode("Metric coverage "), strong(metricCoverage),
      textNode(" · Process coverage "), strong(processCoverage),
      textNode(` · Generated ${formatRelative(snapshot.generated_at_ms)}`),
    );
    elements.coverage.classList.remove("loading-block");
  }

  function summaryCard(item) {
    const card = element("article", "summary-card");
    const top = element("div", "summary-card-top");
    top.append(textNode(item.label), element("i", `mini-status ${item.state === "healthy" ? "" : item.state}`));
    const value = element("strong");
    value.textContent = item.value;
    const detail = element("p");
    detail.textContent = item.detail;
    card.append(top, value, detail);
    return card;
  }

  function renderCharts() {
    const series = [...state.snapshot.series].sort((left, right) => {
      const metric = metricOrder(left.metric_name) - metricOrder(right.metric_name);
      return metric || left.resource.localeCompare(right.resource);
    });
    if (!series.length) {
      elements.charts.replaceChildren(emptyCard("No metric data is available in this range."));
      return;
    }
    elements.charts.replaceChildren(...series.map(chartCard));
  }

  function chartCard(series) {
    const card = element("article", "chart-card");
    const head = element("div", "chart-head");
    const title = element("div", "chart-title");
    const heading = element("h3");
    heading.textContent = metricLabel(series.metric_name);
    const resource = element("p");
    resource.textContent = series.resource || "system";
    title.append(heading, resource);
    const stats = element("div", "chart-stat");
    const max = element("strong");
    max.textContent = `max ${formatValue(series.max_value, series.unit)}`;
    const average = element("span");
    average.textContent = `avg ${formatValue(series.average_value, series.unit)}`;
    stats.append(max, average);
    head.append(title, stats);
    card.append(head, renderSvgChart(series));
    const axis = element("div", "chart-axis");
    axis.append(
      textNode(formatShortTime(state.snapshot.range.from_ms)),
      textNode(formatShortTime(state.snapshot.range.to_ms)),
    );
    card.append(axis);
    return card;
  }

  function renderSvgChart(series) {
    const frame = element("div", "chart-frame");
    const svg = svgElement("svg", { viewBox: "0 0 700 190", role: "img", "aria-label": `${metricLabel(series.metric_name)} trend` });
    [20, 60, 100, 140, 180].forEach((y) => svg.append(svgElement("line", { x1: 0, y1: y, x2: 700, y2: y, class: "chart-grid-line" })));
    if (!series.points.length) {
      frame.append(svg);
      return frame;
    }
    const lower = series.unit === "percent" ? 0 : Math.min(0, series.min_value);
    const upper = series.unit === "percent" ? Math.max(100, series.max_value) : Math.max(series.max_value, lower + 1);
    const duration = Math.max(1, state.snapshot.range.to_ms - state.snapshot.range.from_ms);
    const scale = (point, field) => {
      const x = ((point.timestamp_ms - state.snapshot.range.from_ms) / duration) * 700;
      const y = 180 - ((point[field] - lower) / Math.max(Number.EPSILON, upper - lower)) * 160;
      return [clamp(x, 0, 700), clamp(y, 10, 180)];
    };
    splitSegments(series.points, state.snapshot.bucket_span_ms).forEach((points) => {
      if (!points.length) return;
      const upperPoints = points.map((point) => scale(point, "max_value"));
      const lowerPoints = [...points].reverse().map((point) => scale(point, "min_value"));
      const band = [...upperPoints, ...lowerPoints].map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
      svg.append(svgElement("polygon", { points: band, class: "chart-band" }));
      const line = points.map((point) => scale(point, "average_value")).map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
      svg.append(svgElement("polyline", { points: line, class: "chart-line" }));
    });
    frame.append(svg);
    return frame;
  }

  function renderEvents() {
    const events = state.snapshot.events;
    elements.eventCount.textContent = events.length + (state.snapshot.events_truncated ? "+" : "");
    if (!events.length) {
      elements.events.replaceChildren(emptyCard("No anomaly events overlap this range."));
      return;
    }
    elements.events.replaceChildren(...events.map((eventData) => {
      const button = element("button", "event-item");
      button.type = "button";
      button.addEventListener("click", () => openEvent(eventData.id));
      const row = element("div", "event-row");
      const copy = element("div", "event-copy");
      const severity = element("span", `severity ${eventData.severity}`);
      severity.textContent = eventData.severity;
      const heading = element("h3");
      heading.textContent = `#${eventData.id} ${metricLabel(eventData.metric_name)}`;
      const description = element("p");
      description.textContent = `${eventData.resource || "system"} · ${formatTime(eventData.started_at_ms)} → ${eventData.ended_at_ms ? formatTime(eventData.ended_at_ms) : "open"}`;
      copy.append(severity, heading, description);
      const value = element("div", "event-value");
      value.textContent = `peak ${formatNumber(eventData.peak_value)}`;
      row.append(copy, value);
      button.append(row);
      return button;
    }));
  }

  async function openEvent(id) {
    elements.dialogTitle.textContent = `Event #${id}`;
    elements.dialogBody.replaceChildren(elementWithText("p", "loading-block", "Loading preserved evidence…"));
    elements.dialog.showModal();
    try {
      const detail = await requestJson(`api/v1/events/${id}`);
      elements.dialogTitle.textContent = `Event #${id} · ${metricLabel(detail.summary.metric_name)}`;
      renderEventDetail(detail);
    } catch (error) {
      elements.dialogBody.replaceChildren(elementWithText("p", "empty-state", error.message));
    }
  }

  function renderEventDetail(detail) {
    const grid = element("div", "detail-grid");
    const stats = [
      ["State", `${detail.summary.severity} · ${detail.summary.status}`],
      ["Resource", detail.summary.resource || "system"],
      ["Peak", `${formatNumber(detail.summary.peak_value)} ${detail.unit}`],
      ["Warning", `${formatNumber(detail.warning_threshold)} ${detail.unit}`],
      ["Critical", `${formatNumber(detail.critical_threshold)} ${detail.unit}`],
      ["Samples / gaps", `${detail.sample_count} / ${detail.data_gap_count}`],
    ];
    stats.forEach(([label, value]) => {
      const stat = element("div", "detail-stat");
      stat.append(elementWithText("span", "", label), elementWithText("strong", "", value));
      grid.append(stat);
    });
    const fragment = document.createDocumentFragment();
    fragment.append(grid);
    fragment.append(detailTableSection("Metric checkpoints", ["Time", "Value", "Kind"], detail.evidence.slice(0, 50).map((item) => [formatTime(item.collected_at_ms), `${formatNumber(item.value)} ${detail.unit}`, item.kind])));
    fragment.append(detailTableSection("Related processes", ["Checkpoint", "Process", "CPU", "Memory"], detail.process_evidence.slice(0, 50).map((item) => [`${item.kind} · ${formatTime(item.sample.collected_at_ms)}`, `${item.sample.name} (PID ${item.sample.pid})`, `${formatNumber(item.sample.cpu_usage_percent)}%`, formatBytes(item.sample.memory_bytes)])));
    elements.dialogBody.replaceChildren(fragment);
  }

  function detailTableSection(title, headings, rows) {
    const section = element("section", "dialog-section");
    section.append(elementWithText("h3", "", title));
    if (!rows.length) {
      section.append(elementWithText("p", "empty-state", "No preserved evidence for this section."));
      return section;
    }
    const shell = element("div", "dialog-table");
    const table = element("table");
    const head = element("thead");
    const headRow = element("tr");
    headings.forEach((heading) => headRow.append(elementWithText("th", "", heading)));
    head.append(headRow);
    const body = element("tbody");
    rows.forEach((values) => {
      const row = element("tr");
      values.forEach((value) => row.append(elementWithText("td", "", value)));
      body.append(row);
    });
    table.append(head, body);
    shell.append(table);
    section.append(shell);
    return section;
  }

  function renderStorage() {
    const status = state.status;
    const items = [
      ["Latest metric", status.raw.newest_ms ? formatRelative(status.raw.newest_ms) : "No data"],
      ["Database", formatBytes(status.database_bytes)],
      ["WAL", formatBytes(status.wal_bytes)],
      ["Raw rows", formatInteger(status.raw.row_count)],
      ["1-minute rows", formatInteger(status.minute.row_count)],
      ["15-minute rows", formatInteger(status.quarter_hour.row_count)],
      ["Capabilities", capabilitySummary(status.capabilities)],
      ["Schema", `v${status.schema_version}`],
    ];
    const list = element("dl", "health-list");
    items.forEach(([label, value]) => {
      const row = element("div", "health-row");
      row.append(elementWithText("dt", "", label), elementWithText("dd", "", value));
      list.append(row);
    });
    elements.storage.replaceChildren(list);
    elements.storage.classList.remove("loading-block");
  }

  function renderProcesses() {
    const processes = [...state.snapshot.processes];
    processes.sort((left, right) => state.processSort === "memory"
      ? right.peak_memory_bytes - left.peak_memory_bytes || right.peak_cpu_percent - left.peak_cpu_percent
      : right.peak_cpu_percent - left.peak_cpu_percent || right.peak_memory_bytes - left.peak_memory_bytes);
    elements.processRows.replaceChildren(...processes.map((process) => {
      const row = element("tr");
      row.append(
        elementWithText("td", "process-name", process.name),
        elementWithText("td", "", process.pid),
        elementWithText("td", "", `${formatNumber(process.peak_cpu_percent)}%`),
        elementWithText("td", "", formatBytes(process.peak_memory_bytes)),
        elementWithText("td", "", formatInteger(process.sample_count)),
        elementWithText("td", "", formatRelative(process.last_seen_ms)),
      );
      return row;
    }));
    elements.processEmpty.hidden = processes.length > 0;
  }

  function renderError(message) {
    if (!state.snapshot) {
      elements.cards.replaceChildren(emptyCard(message));
      elements.charts.replaceChildren(emptyCard("Resource trends are unavailable."));
      elements.events.replaceChildren(emptyCard("Anomaly history is unavailable."));
      elements.storage.textContent = "Storage status is unavailable.";
    }
  }

  function setPreset(range) {
    state.range = { last: range };
    elements.presets.querySelectorAll("button").forEach((button) => {
      const selected = button.dataset.range === range;
      button.classList.toggle("selected", selected);
      button.setAttribute("aria-pressed", String(selected));
    });
    elements.custom.open = false;
    refresh();
  }

  function applyCustomRange(event) {
    event.preventDefault();
    const from = new Date(elements.from.value);
    const to = new Date(elements.to.value);
    if (!Number.isFinite(from.getTime()) || !Number.isFinite(to.getTime()) || from >= to) {
      showToast("Choose a valid start and end time.");
      return;
    }
    state.range = { from: from.toISOString(), to: to.toISOString() };
    elements.presets.querySelectorAll("button").forEach((button) => {
      button.classList.remove("selected");
      button.setAttribute("aria-pressed", "false");
    });
    elements.custom.open = false;
    refresh();
  }

  function scheduleRefresh() {
    window.clearTimeout(state.timer);
    if (elements.autoRefresh.checked) state.timer = window.setTimeout(refresh, 15_000);
  }

  function setConnection(kind, label) {
    elements.badge.className = `status-badge ${kind === "healthy" ? "" : kind}`;
    elements.badge.lastChild.textContent = label;
  }

  function showToast(message) {
    elements.toast.textContent = message;
    elements.toast.hidden = false;
    window.setTimeout(() => { elements.toast.hidden = true; }, 5_000);
  }

  function findSeries(metricName) { return state.snapshot.series.find((series) => series.metric_name === metricName); }
  function findAllSeries(metricName) { return state.snapshot.series.filter((series) => series.metric_name === metricName); }
  function findCapability(name) { return state.status.capabilities.find((capability) => capability.capability === name); }
  function capabilityLabel(value) { return ({ available: "Available", degraded: "Degraded", unavailable: "Unavailable" })[value] || value; }
  function capabilitySummary(capabilities) {
    if (!capabilities.length) return "No data";
    const available = capabilities.filter((capability) => capability.state === "available").length;
    const degraded = capabilities.filter((capability) => capability.state === "degraded").length;
    return degraded ? `${available}/${capabilities.length} available · ${degraded} degraded` : `${available}/${capabilities.length} available`;
  }
  function lastValue(series) { return series.points.at(-1)?.average_value ?? series.average_value; }
  function valueState(value, warning, critical) { return value == null ? "warning" : value >= critical ? "error" : value >= warning ? "warning" : "healthy"; }
  function clamp(value, min, max) { return Math.min(max, Math.max(min, value)); }
  function splitSegments(points, bucketSpan) {
    const segments = [];
    let current = [];
    points.forEach((point) => {
      if (current.length && point.timestamp_ms - current.at(-1).timestamp_ms > bucketSpan * 2.5) {
        segments.push(current);
        current = [];
      }
      current.push(point);
    });
    if (current.length) segments.push(current);
    return segments;
  }
  function metricOrder(name) { const index = ["cpu.total.usage", "memory.usage", "gpu.device.usage", "gpu.renderer.usage", "gpu.tiler.usage", "gpu.memory.used", "gpu.memory.allocated", "disk.space.usage", "disk.io.read.rate", "disk.io.write.rate", "network.receive.rate", "network.transmit.rate"].indexOf(name); return index < 0 ? 999 : index; }
  function metricLabel(name) { return ({ "cpu.total.usage": "CPU usage", "memory.usage": "Memory usage", "gpu.device.usage": "GPU usage", "gpu.renderer.usage": "GPU renderer usage", "gpu.tiler.usage": "GPU tiler usage", "gpu.memory.used": "GPU memory in use", "gpu.memory.allocated": "GPU allocated memory", "disk.space.usage": "Disk space usage", "disk.io.read.rate": "Disk read rate", "disk.io.write.rate": "Disk write rate", "network.receive.rate": "Network receive rate", "network.transmit.rate": "Network transmit rate" })[name] || name; }
  function resolutionLabel(value) { return ({ raw: "Raw samples", minute: "1-minute rollup", quarter_hour: "15-minute rollup" })[value] || value; }
  function coverageText(oldest, newest) { return oldest == null || newest == null ? "no data" : `${formatTime(oldest)} → ${formatTime(newest)}`; }
  function freshnessLabel(timestamp) {
    if (timestamp == null) return "No data";
    const seconds = Math.max(0, (Date.now() - timestamp) / 1000);
    return seconds < 30 ? "Live" : seconds < 120 ? `${Math.round(seconds)}s behind` : "Data delayed";
  }
  function formatValue(value, unit) { return unit === "percent" ? `${formatNumber(value)}%` : unit === "bytes" ? formatBytes(value) : unit === "bytes_per_second" ? `${formatBytes(value)}/s` : `${formatNumber(value)} ${unit}`; }
  function formatNumber(value) { return new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 }).format(value); }
  function formatInteger(value) { return new Intl.NumberFormat().format(value); }
  function formatBytes(value) {
    const bytes = Math.max(0, Number(value));
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let amount = bytes;
    let index = 0;
    while (amount >= 1024 && index < units.length - 1) { amount /= 1024; index += 1; }
    return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: index ? 2 : 0 }).format(amount)} ${units[index]}`;
  }
  function formatDuration(milliseconds) { return milliseconds >= 3_600_000 ? `${formatNumber(milliseconds / 3_600_000)}h` : milliseconds >= 60_000 ? `${formatNumber(milliseconds / 60_000)}m` : `${formatNumber(milliseconds / 1000)}s`; }
  function formatTime(timestamp) { return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(timestamp)); }
  function formatShortTime(timestamp) { return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(new Date(timestamp)); }
  function formatRelative(timestamp) {
    const seconds = Math.round((timestamp - Date.now()) / 1000);
    const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
    if (Math.abs(seconds) < 60) return formatter.format(seconds, "second");
    const minutes = Math.round(seconds / 60);
    if (Math.abs(minutes) < 60) return formatter.format(minutes, "minute");
    const hours = Math.round(minutes / 60);
    if (Math.abs(hours) < 24) return formatter.format(hours, "hour");
    return formatter.format(Math.round(hours / 24), "day");
  }
  function element(tag, className = "") { const node = document.createElement(tag); if (className) node.className = className; return node; }
  function elementWithText(tag, className, value) { const node = element(tag, className); node.textContent = String(value); return node; }
  function textNode(value) { return document.createTextNode(value); }
  function strong(value) { return elementWithText("strong", "", value); }
  function emptyCard(message) { return elementWithText("div", "empty-card", message); }
  function svgElement(tag, attributes) { const node = document.createElementNS("http://www.w3.org/2000/svg", tag); Object.entries(attributes).forEach(([name, value]) => node.setAttribute(name, value)); return node; }

  elements.refresh.addEventListener("click", refresh);
  elements.presets.addEventListener("click", (event) => { const button = event.target.closest("button[data-range]"); if (button) setPreset(button.dataset.range); });
  elements.customForm.addEventListener("submit", applyCustomRange);
  elements.autoRefresh.addEventListener("change", scheduleRefresh);
  elements.sortCpu.addEventListener("click", () => { state.processSort = "cpu"; toggleSort(); renderProcesses(); });
  elements.sortMemory.addEventListener("click", () => { state.processSort = "memory"; toggleSort(); renderProcesses(); });
  elements.closeDialog.addEventListener("click", () => elements.dialog.close());
  elements.dialog.addEventListener("click", (event) => { if (event.target === elements.dialog) elements.dialog.close(); });
  function toggleSort() {
    const cpu = state.processSort === "cpu";
    elements.sortCpu.classList.toggle("selected", cpu);
    elements.sortCpu.setAttribute("aria-pressed", String(cpu));
    elements.sortMemory.classList.toggle("selected", !cpu);
    elements.sortMemory.setAttribute("aria-pressed", String(!cpu));
  }

  const now = new Date();
  elements.to.value = localInputValue(now);
  elements.from.value = localInputValue(new Date(now.getTime() - 3_600_000));
  function localInputValue(date) {
    const offset = date.getTimezoneOffset() * 60_000;
    return new Date(date.getTime() - offset).toISOString().slice(0, 16);
  }

  refresh();
})();
