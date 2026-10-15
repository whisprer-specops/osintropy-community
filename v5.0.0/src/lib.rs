// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! OSINTropy: a local, provenance-first case notebook for public-source observations.

#![forbid(unsafe_code)]

pub mod analysis;
mod casefold_data;
pub mod cli;
pub mod csvread;
pub mod demo;
pub mod error;
pub mod http;
pub mod importers;
pub mod isotime;
pub mod models;
pub mod paths;
pub mod pyfmt;
pub mod pyjson;
pub mod report;
pub mod sources;
pub mod storage;
pub mod urlparse;
