// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! `osintropy` command-line entry point.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(raw) => {
                let _ = writeln!(
                    io::stderr(),
                    "osintropy: error: argument is not valid Unicode: {}",
                    raw.to_string_lossy()
                );
                return ExitCode::from(2);
            }
        }
    }
    let runtime = osintropy::cli::Runtime::standard();
    let code = osintropy::cli::run(
        &args,
        &runtime,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(code)
}
