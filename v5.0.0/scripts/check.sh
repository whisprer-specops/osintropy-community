#!/usr/bin/env bash
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
cargo fmt --all --check
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
# The offline demo, into a fresh folder that git ignores.
rm -rf target/demo
cargo run --locked --release -- --db target/demo/demo-case.db demo --out target/demo/demo.html
printf '%s\n' 'OSINTropy validation passed.'
