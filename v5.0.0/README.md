# OSINTropy 5.0.0

This folder is the Rust source of OSINTropy 5.0.0, a local, provenance-first case notebook for public-source observations. Copyright © 2026 whispr.dev, free software under GPL-3.0-only.

The full guide is the repository's [README](../README.md). The source catalogue, with each connector's fields, credentials and evidence rules, is [SOURCES.md](SOURCES.md).

## Build, check and install

On Windows 11, in PowerShell, from this folder:

```powershell
cargo build --locked --release
powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1
cargo install --locked --path .
```

The first command builds `target\release\osintropy.exe`. The second runs every check: formatting, the tests, the strict lints, a release build and the offline demo. The third puts `osintropy` on your `PATH`. On Linux or macOS, use `bash scripts/check.sh` for the checks.

## Layout

| Path | What it holds |
| --- | --- |
| `src/main.rs`, `src/cli.rs` | The command line: parsing, help and every command |
| `src/models.rs` | Observations, their checks and content fingerprints |
| `src/storage.rs` | The SQLite case file: runs, observations, links, backup and pruning |
| `src/analysis.rs` | Source diversity, review groups and field variations |
| `src/report.rs` | JSON, CSV, HTML and GraphML exports |
| `src/importers.rs`, `src/csvread.rs` | JSON, CSV and legacy-database imports |
| `src/demo.rs` | The synthetic offline demo |
| `src/http.rs` | The bounded HTTPS client and proxy rules |
| `src/sources/` | The connectors: `person.rs`, `organization.rs`, `domain.rs`, and the catalogues in `mod.rs` |
| `src/pyjson.rs`, `src/pyfmt.rs`, `src/urlparse.rs`, `src/isotime.rs`, `src/casefold_data.rs` | Python 4.1.1's text, number, JSON, URL and time rules, kept so reports match byte for byte |
| `tests/` | Offline contract tests with synthetic data |
| `scripts/` | `check.ps1` and `check.sh` |
