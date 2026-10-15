// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The command line, in-process with stand-in sources and as the built binary.

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Output};

use common::{FakeEnv, FakeHttp, Scratch};
use osintropy::analysis::summarize;
use osintropy::cli::{run, Runtime};
use osintropy::http::{Http, SourceError};
use osintropy::models::{observation_id, Kind, Observation};
use osintropy::sources::{self, Catalog, Context, NameSource, PersonQuery, PersonSource};
use osintropy::storage::Store;
use serde_json::json;

fn runtime() -> Runtime {
    Runtime {
        env: Box::new(FakeEnv::with(&[])),
        http: Box::new(|| -> Box<dyn Http> { panic!("this command must stay offline") }),
        people: sources::person_sources(),
        organizations: sources::organization_sources(),
        domains: sources::domain_sources(),
    }
}

fn call(rt: &Runtime, args: &[&str]) -> (u8, String, String) {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(&args, rt, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn db_arg(dir: &Scratch) -> String {
    dir.join("case.db").to_string_lossy().into_owned()
}

#[test]
fn demo_end_to_end_is_offline() {
    let dir = Scratch::new();
    let html = dir.join("demo.html");
    let db = db_arg(&dir);
    let (code, out, err) = call(
        &runtime(),
        &["--db", &db, "demo", "--out", &html.to_string_lossy()],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("Run: ") && out.contains("| 10 observations | complete"));
    let rendered = std::fs::read_to_string(&html).unwrap();
    for text in [
        "Avery Example",
        "Example Works Ltd",
        "example.org",
        "Cardiff / Newport",
    ] {
        assert!(rendered.contains(text), "{text}");
    }
    let store = Store::open(Path::new(&db)).unwrap();
    let runs = store.list_runs().unwrap();
    assert_eq!(runs.len(), 1);
    let run_record = store.get_run(&runs[0].id).unwrap();
    assert_eq!(run_record.observations.len(), 10);
    let types: HashSet<&str> = run_record
        .observations
        .iter()
        .map(|item| item.subject_type.as_str())
        .collect();
    assert_eq!(types, HashSet::from(["person", "organization", "domain"]));
    assert_eq!(
        run_record
            .observations
            .iter()
            .filter(|item| item.kind == Kind::Lead)
            .count(),
        2
    );
    assert_eq!(run_record.reviewed_links.len(), 2);
    let analysis = summarize(&run_record);
    assert_eq!(analysis.possible_reviews.len(), 1);
    let names: HashSet<&str> = analysis.possible_reviews[0]
        .iter()
        .map(|id| {
            run_record
                .observations
                .iter()
                .find(|item| observation_id(item) == *id)
                .map(|item| item.name.as_str())
                .unwrap()
        })
        .collect();
    assert_eq!(names, HashSet::from(["Alex Example"]));
}

fn company(name: &str, _: &mut Context<'_>) -> Result<Vec<Observation>, SourceError> {
    Ok(vec![Observation::from_mapping(
        &json!({"source": "fake", "source_id": "1",
        "name": name, "subject_type": "organization"}),
    )?])
}

fn broken(_: &str, _: &mut Context<'_>) -> Result<Vec<Observation>, SourceError> {
    Err(SourceError::new("fixture source unavailable"))
}

fn domain(name: &str, _: &mut Context<'_>) -> Result<Vec<Observation>, SourceError> {
    Ok(vec![Observation::from_mapping(
        &json!({"source": "fake", "source_id": name,
        "name": name, "subject_type": "domain"}),
    )?])
}

fn repeating(_: &PersonQuery, _: &mut Context<'_>) -> Result<Vec<Observation>, SourceError> {
    let hit = Observation::from_mapping(&json!({"source": "fake", "source_id": "same",
        "name": "Avery Example", "kind": "record"}))?;
    Ok(vec![hit.clone(), hit])
}

fn with_fakes() -> Runtime {
    let mut rt = runtime();
    rt.http = Box::new(|| Box::new(FakeHttp::new(Vec::new())) as Box<dyn Http>);
    rt.organizations = Catalog {
        sources: vec![("fake", company as NameSource)],
        defaults: vec!["fake"],
    };
    rt.domains = Catalog {
        sources: vec![("fake", domain as NameSource), ("broken", broken)],
        defaults: vec!["fake"],
    };
    rt.people = Catalog {
        sources: vec![("fake", repeating as PersonSource)],
        defaults: vec!["fake"],
    };
    rt
}

#[test]
fn org_and_domain_commands_store_typed_records_and_partial_errors() {
    let dir = Scratch::new();
    let db = db_arg(&dir);
    let rt = with_fakes();
    let (code, _, _) = call(
        &rt,
        &["--db", &db, "search-org", "Avery Ltd", "--source", "fake"],
    );
    assert_eq!(code, 0);
    let (code, out, err) = call(
        &rt,
        &[
            "--db",
            &db,
            "domain",
            "EXAMPLE.ORG",
            "--source",
            "fake",
            "--source",
            "broken",
        ],
    );
    assert_eq!(code, 3, "a failed source means exit code 3");
    assert!(out.contains("| partial"));
    assert_eq!(err, "  broken: fixture source unavailable\n");
    let store = Store::open(Path::new(&db)).unwrap();
    let runs: Vec<_> = store
        .list_runs()
        .unwrap()
        .iter()
        .map(|summary| store.get_run(&summary.id).unwrap())
        .collect();
    let types: HashSet<&str> = runs
        .iter()
        .map(|run| run.observations[0].subject_type.as_str())
        .collect();
    assert_eq!(types, HashSet::from(["domain", "organization"]));
    let partial = runs.iter().find(|run| run.status == "partial").unwrap();
    assert_eq!(partial.errors["broken"], "fixture source unavailable");
    assert_eq!(partial.query, "Domain: example.org");
    let (code, _, err) = call(
        &rt,
        &["--db", &db, "domain", "example.org", "--source", "broken"],
    );
    assert_eq!(code, 3);
    assert!(err.contains("broken"));
    let failed = store
        .list_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.status == "failed");
    assert!(failed.is_some_and(|run| run.count == 0));
}

#[test]
fn a_repeated_hit_is_kept_once() {
    let dir = Scratch::new();
    let db = db_arg(&dir);
    let (code, out, err) = call(&with_fakes(), &["--db", &db, "search", "Avery", "Example"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("| 1 observations | complete"));
}

#[test]
fn search_input_is_checked_before_any_lookup() {
    let dir = Scratch::new();
    let db = db_arg(&dir);
    let rt = runtime();
    let cases: [(&[&str], &str); 5] = [
        (
            &["search", " ", "Lovelace"],
            "Error: first and last name are required\n",
        ),
        (
            &["search", "Ada", "Lovelace\u{1}"],
            "Error: field contains control or direction characters\n",
        ),
        (
            &["search-org", "  "],
            "Error: organization name is required\n",
        ),
        (
            &["domain", "https://example.org"],
            "Error: supply a domain name, without a URL, port, or wildcard\n",
        ),
        (
            &["domain", "10.0.0.1"],
            "Error: IP addresses are not domain names\n",
        ),
    ];
    for (args, expected) in cases {
        let mut full = vec!["--db", db.as_str()];
        full.extend_from_slice(args);
        let (code, _, err) = call(&rt, &full);
        assert_eq!((code, err.as_str()), (2, expected), "{args:?}");
    }
}

#[test]
fn review_commands_print_what_4_1_1_printed() {
    let dir = Scratch::new();
    let db = db_arg(&dir);
    let rt = runtime();
    call(&rt, &["--db", &db, "demo"]);
    let (code, out, _) = call(&rt, &["--db", &db, "inspect", "3"]);
    assert_eq!(code, 0);
    let first_lines: Vec<&str> = out.lines().take(3).collect();
    assert_eq!(first_lines[0], "{");
    assert!(first_lines[1].starts_with("  \"run_id\": \""));
    assert_eq!(first_lines[2], "  \"source\": \"synthetic_openalex\",");
    assert_eq!(
        call(&rt, &["--db", &db, "link", "3", "4", "--reason", "Checked"]).1,
        "Linked #3 and #4\n"
    );
    assert_eq!(
        call(&rt, &["--db", &db, "unlink", "4", "3"]).1,
        "Removed 1 link(s)\n"
    );
    assert_eq!(
        call(&rt, &["--db", &db, "unlink", "4", "3"]).1,
        "Removed 0 link(s)\n"
    );
    let (code, _, err) = call(&rt, &["--db", &db, "link", "1", "5", "--reason", "x"]);
    assert_eq!(code, 2);
    assert_eq!(
        err,
        "Error: both observations must exist and neither may be an unverified lead\n"
    );
    let (code, _, err) = call(&rt, &["--db", &db, "show", "missing"]);
    assert_eq!((code, err.as_str()), (2, "Error: run missing not found\n"));
    let listed = call(&rt, &["--db", &db, "list"]).1;
    let run_id = listed.split_whitespace().next().unwrap().to_owned();
    assert!(listed.contains("  complete    10  Synthetic demonstration"));
    let target = dir.join("out").join("case.graphml");
    let (code, out, _) = call(
        &rt,
        &[
            "--db",
            &db,
            "export",
            &run_id,
            "--format",
            "graphml",
            "--out",
            &target.to_string_lossy(),
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(out.trim_end(), target.to_string_lossy());
    assert!(std::fs::read_to_string(&target)
        .unwrap()
        .contains("<graphml"));
    let (code, out, _) = call(&rt, &["--db", &db, "show", &run_id]);
    assert_eq!(code, 0);
    assert!(out.starts_with("{\n  \"analysis\": {") && out.ends_with("}\n"));
    let copy = dir.join("copy.db");
    assert_eq!(
        call(
            &rt,
            &["--db", &db, "backup", "--out", &copy.to_string_lossy()]
        )
        .0,
        0
    );
    assert_eq!(
        call(&rt, &["--db", &db, "prune", "--days", "-1"]).2,
        "Error: days cannot be negative\n"
    );
    assert_eq!(
        call(&rt, &["--db", &db, "prune", "--days", "30"]).1,
        "Deleted 0 run(s)\n"
    );
    {
        let raw = rusqlite::Connection::open(&db).unwrap();
        raw.execute("UPDATE runs SET created_at='2001-01-01T00:00:00+00:00'", [])
            .unwrap();
    }
    assert_eq!(
        call(&rt, &["--db", &db, "prune", "--days", "30"]).1,
        "Deleted 1 run(s)\n"
    );
    assert_eq!(
        call(&rt, &["--db", &db, "delete", &run_id]).1,
        "Deleted 0 run(s)\n"
    );
    let saved = Store::open(&copy).unwrap();
    assert_eq!(
        saved.list_runs().unwrap().len(),
        1,
        "the backup kept the run"
    );
}

#[test]
fn usage_mistakes_exit_two_and_explain() {
    let rt = runtime();
    for args in [
        &["search", "Ada"][..],
        &["search", "Ada", "Lovelace", "--source", "nope"],
        &["search", "Ada", "Lovelace", "--frob"],
        &["export", "abc"],
        &["export", "abc", "--out", "x", "--format", "pdf"],
        &["link", "1", "2"],
        &["link", "one", "2", "--reason", "x"],
        &["prune", "--days", "soon"],
        &["frobnicate"],
        &["--db"],
    ] {
        let (code, out, err) = call(&rt, args);
        assert_eq!(code, 2, "{args:?}");
        assert!(out.is_empty());
        assert!(err.starts_with("usage: osintropy"), "{args:?}: {err}");
        assert!(err.contains("error: "));
    }
}

#[test]
fn help_and_version() {
    let rt = runtime();
    let (code, out, _) = call(&rt, &["--version"]);
    assert_eq!(code, 0);
    let mut lines = out.lines();
    assert_eq!(
        lines.next(),
        Some(concat!("osintropy ", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(lines.next(), Some("Copyright (C) 2026 whispr.dev"));
    assert!(out.contains("GPL-3.0-only") && out.contains("NO WARRANTY"));
    for args in [&["help"][..], &["--help"], &["-h"]] {
        let (code, out, _) = call(&rt, args);
        assert_eq!(code, 0);
        assert!(out.starts_with("osintropy - "));
        assert!(out.contains("Copyright (C) 2026 whispr.dev. Free software under GPL-3.0-only"));
    }
    let (code, out, _) = call(&rt, &["search", "--help"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("usage: osintropy [--db FILE] search FIRST LAST"));
    let (code, out, err) = call(&rt, &[]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("USAGE:"));
}

fn binary(args: &[&str], dir: &Scratch) -> Output {
    Command::new(env!("CARGO_BIN_EXE_osintropy"))
        .args(args)
        .env("HOME", dir.path())
        .env("USERPROFILE", dir.path())
        .env("LOCALAPPDATA", dir.join("local"))
        .env("XDG_DATA_HOME", dir.join("xdg"))
        .env_remove("https_proxy")
        .env_remove("no_proxy")
        .env_remove("NO_PROXY")
        .output()
        .expect("run the osintropy binary")
}

#[test]
fn binary_runs_offline_commands() {
    let dir = Scratch::new();
    let output = binary(&["sources"], &dir);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout)
        .starts_with("PERSON SEARCH (first and last name)\nnpi "));
    let default_db = if cfg!(windows) {
        dir.join("local").join("osintropy").join("cases.db")
    } else {
        dir.join("xdg").join("osintropy").join("cases.db")
    };
    assert!(
        !default_db.exists(),
        "`sources` must not create a case file"
    );
    let output = binary(&["list"], &dir);
    assert!(output.status.success());
    assert!(
        default_db.exists(),
        "the default case file lives in the user's data folder"
    );
    let output = binary(&["--version"], &dir);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("osintropy 5."));
    let output = binary(&["frob"], &dir);
    assert_eq!(output.status.code(), Some(2));
}

/// A real search through a proxy that refuses connections: the run is saved as failed,
/// the exit code is 3, and the error names no host, URL or token.
#[test]
fn binary_network_failure_is_saved_without_leaking_the_request() {
    let dir = Scratch::new();
    let db = db_arg(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_osintropy"))
        .args([
            "--db", &db, "search", "Ada", "Lovelace", "--source", "github",
        ])
        // Removals first: on Windows variable names ignore case.
        .env_remove("https_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("GITHUB_TOKEN", "secret-token-value")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("| 0 observations | failed"), "{stdout}");
    assert!(stderr.starts_with("  github: network error: "), "{stderr}");
    for secret in ["github.com", "secret-token-value", "https://", "Lovelace"] {
        assert!(!stderr.contains(secret), "{secret} leaked: {stderr}");
    }
}

/// A proxy address the client cannot use makes the search fail; it never goes direct.
#[test]
fn binary_refuses_an_unusable_proxy() {
    let dir = Scratch::new();
    let db = db_arg(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_osintropy"))
        .args(["--db", &db, "domain", "example.org"])
        .env_remove("https_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env("HTTPS_PROXY", "ftp://127.0.0.1:9")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "  rdap: network error: HTTPS_PROXY holds an unsupported proxy address\n"
    );
}
