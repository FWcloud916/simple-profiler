# Changelog

All notable changes to Simple Profiler are recorded in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-07-16

### Added

- Continuous background collection of CPU, memory, disk capacity/I/O, network, and bounded
  per-process resource rankings on macOS.
- Local SQLite retention tiers, anomaly detection, event evidence, diagnostic reports, and a
  token-scoped loopback dashboard with an interactive historical timeline.
- Per-user LaunchAgent installation, lifecycle commands, managed shell launcher, graceful
  shutdown, single-instance locking, and bounded log rotation.
- Process attribution for CPU, memory, disk read/write, and network receive/transmit, including
  chart overlays, hover values, event evidence, CLI rankings, and retained rollups.

### Changed

- Storage schema reached version 7, with transactional migrations and bounded maintenance.
- Dashboard charts support slider, pointer, button, and keyboard time navigation with ranked
  process colors and line patterns.

### Removed

- GPU collection, process attribution, configuration, helper service, historical columns, and
  dashboard/report surfaces were removed because the platform data source was not reliable enough.

### Security

- Dashboard access is loopback-only, protected by a random per-launch token and strict response,
  Host, concurrency, range, and query limits.
- Process collection excludes command lines, environments, and working directories; executable
  paths remain disabled by default.

[Unreleased]: https://github.com/FWcloud916/simple-profiler/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/FWcloud916/simple-profiler/releases/tag/v0.1.0
