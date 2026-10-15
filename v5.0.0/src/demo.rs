// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Offline demonstration of every subject type and the review workflow. Nothing here
//! touches the network: every source name starts with `synthetic_`.

use std::collections::BTreeMap;

use serde_json::json;

use crate::error::{Error, Result};
use crate::models::{observation_id, Observation};
use crate::storage::Store;

pub const QUERY: &str = "Synthetic demonstration: people, organizations, and domains";

/// Store one synthetic case with ten observations, two unverified leads, two reviewed
/// links and a same-name pair left for review. Returns the run ID.
pub fn create_demo(store: &Store) -> Result<String> {
    let rows = [
        json!({"source": "synthetic_npi", "source_id": "P-1", "name": "Avery Example",
               "fields": {"occupation": "Engineer", "city": "Sampleton"}}),
        json!({"source": "synthetic_wikidata", "source_id": "P-2", "name": "Avery Example",
               "fields": {"occupation": "Engineer", "city": "Sampleton"}}),
        json!({"source": "synthetic_openalex", "source_id": "P-3", "name": "Alex Example",
               "fields": {"occupation": "Researcher"}}),
        json!({"source": "synthetic_openlibrary", "source_id": "P-4", "name": "Alex Example",
               "fields": {"occupation": "Author"}}),
        json!({"source": "synthetic_crossref", "source_id": "L-1",
               "name": "Search lead for Avery Example", "kind": "lead",
               "fields": {"publication_title": "An Imaginary Paper", "author_name": "Avery Example"}}),
        json!({"source": "synthetic_companies_house", "source_id": "O-1",
               "name": "Example Works Ltd", "subject_type": "organization",
               "fields": {"company_number": "DEMO-001", "city": "Cardiff"}}),
        json!({"source": "synthetic_gleif", "source_id": "O-2",
               "name": "Example Works Ltd", "subject_type": "organization",
               "fields": {"company_number": "DEMO-001", "lei": "DEMO-LEI-001", "city": "Newport"}}),
        json!({"source": "synthetic_register", "source_id": "O-3", "name": "Avery Example",
               "subject_type": "organization", "fields": {"business_type": "Sole proprietorship"}}),
        json!({"source": "synthetic_rdap", "source_id": "D-1", "name": "example.org",
               "subject_type": "domain", "fields": {"status": "active", "nameserver": "ns1.example.org"}}),
        json!({"source": "synthetic_wayback", "source_id": "L-2", "name": "example.org",
               "subject_type": "domain", "kind": "lead",
               "fields": {"archived_url": "https://example.org/about", "capture_time": "20250101000000"}}),
    ];
    let observations = rows
        .iter()
        .map(Observation::from_mapping)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let run_id = store.add_run(QUERY, &observations, &BTreeMap::new())?;
    let stored = store.get_run(&run_id)?;
    let id_of = |source: &str, source_id: &str| -> Result<i64> {
        stored
            .observations
            .iter()
            .find(|item| item.source == source && item.source_id == source_id)
            .map(observation_id)
            .ok_or_else(|| {
                Error::NotFound(format!("demo observation {source} {source_id} missing"))
            })
    };
    store.link(
        id_of("synthetic_npi", "P-1")?,
        id_of("synthetic_wikidata", "P-2")?,
        "Demo only: these two invented person records were manually reviewed",
    )?;
    store.link(
        id_of("synthetic_companies_house", "O-1")?,
        id_of("synthetic_gleif", "O-2")?,
        "Demo only: matching invented company number reviewed; city values differ",
    )?;
    Ok(run_id)
}
