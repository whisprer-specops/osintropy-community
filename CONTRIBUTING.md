# Contributing

## Copyright first

whispr.dev holds the copyright in OSINTropy. Before starting work on a change you would like merged, open an issue so the terms for your contribution can be agreed first. Bug reports need no agreement: see the end of this file.

Every code file opens with these two lines, as comments in its own language, after any `#!` line. `tests/notice_contracts.rs` fails a file without them.

```text
Copyright (C) 2026 whispr.dev
SPDX-License-Identifier: GPL-3.0-only
```

## Building and checking

Work in `v5.0.0/`. Use Rust 1.97.0, as pinned in `rust-toolchain.toml` (rustup installs it on the first build), and a C build toolchain for the bundled SQLite and cryptography code. The code keeps to Rust 1.87, the declared minimum in `Cargo.toml`.

Build and validate from `v5.0.0/`:

```text
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
cargo run --locked --release -- --db target/demo/demo-case.db demo --out target/demo/demo.html
```

On Windows, `powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1` runs all of these; `scripts/check.sh` does the same elsewhere.

## Rules for changes

- Tests must never query real people or need network access. Add synthetic fixtures for source contract changes; `tests/common/mod.rs` has a recording fake HTTP client.
- New source adapters must keep per-hit provenance, bound their requests, and never treat a shared name as proof of identity.
- Keep the case-file schema and the report formats unless a documented breaking change is required. Either version must still open the other's case files.
- Explain any dependency change and update `Cargo.lock` intentionally.
- Update `SOURCES.md`, the README and `CHANGELOG.md` with any change to sources or behaviour.

Bug reports should include the version (`osintropy --version`), operating system, exact command, expected result and redacted error output. Never include credentials, case files or real personal data in public reports. Keep discussion constructive and follow `CODE_OF_CONDUCT.md`.
