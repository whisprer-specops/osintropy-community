// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The storage, review, export and import contracts, ported from 4.1.1's
//! `tests/test_pipeline.py` and extended.

mod common;

use std::collections::BTreeMap;
use std::fs;

use common::Scratch;
use osintropy::analysis::summarize;
use osintropy::csvread;
use osintropy::error::Error;
use osintropy::importers::{load_file, load_legacy_db};
use osintropy::models::{observation_id, Kind, Observation, SubjectType};
use osintropy::paths;
use osintropy::report::{render, Format};
use osintropy::storage::Store;
use serde_json::{json, Value};

fn obs(source: &str, source_id: &str, name: &str, kind: &str, fields: Value) -> Observation {
    Observation::from_mapping(&json!({
        "source": source, "source_id": source_id, "name": name, "kind": kind, "fields": fields,
    }))
    .expect("valid observation")
}

fn record(source: &str, source_id: &str) -> Observation {
    obs(source, source_id, "Avery Example", "record", json!({}))
}

fn no_errors() -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[test]
fn separate_same_names_review_link_and_deletion() {
    let dir = Scratch::new();
    let store = Store::open(&dir.join("cases.db")).unwrap();
    let run_id = store
        .add_run(
            "Avery Example",
            &[
                obs("one", "1", "Avery Example", "record", json!({"city": "A"})),
                obs("two", "2", "Avery Example", "record", json!({"city": "B"})),
                obs("one", "3", "Alex Example", "record", json!({})),
            ],
            &no_errors(),
        )
        .unwrap();
    let report = store.get_run(&run_id).unwrap();
    assert!(summarize(&report).reviewed_groups.is_empty());
    assert_eq!(summarize(&report).possible_reviews.len(), 1);
    let ids: Vec<i64> = report.observations.iter().map(observation_id).collect();
    store
        .link(ids[0], ids[1], "Reviewed against a trusted source")
        .unwrap();
    let linked = summarize(&store.get_run(&run_id).unwrap());
    assert_eq!(linked.reviewed_groups, vec![ids[..2].to_vec()]);
    assert_eq!(linked.field_variations.len(), 1);
    assert_eq!(linked.field_variations[0].values, ["A", "B"]);
    assert!(linked.possible_reviews.is_empty());

    let backup = store.backup(&dir.join("backup.db")).unwrap();
    assert_eq!(Store::open(&backup).unwrap().list_runs().unwrap().len(), 1);
    let same = store.backup(&dir.join("cases.db")).unwrap_err();
    assert_eq!(
        same.to_string(),
        "backup destination must differ from the active database"
    );

    assert_eq!(store.delete_run(&run_id).unwrap(), 1);
    let db = rusqlite::Connection::open(dir.join("cases.db")).unwrap();
    let links: i64 = db
        .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))
        .unwrap();
    let observations: i64 = db
        .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
        .unwrap();
    assert_eq!((links, observations), (0, 0));
}

#[test]
fn leads_cannot_be_linked_and_bad_batch_rolls_back() {
    let dir = Scratch::new();
    let store = Store::open(&dir.join("cases.db")).unwrap();
    let run_id = store
        .add_run(
            "query",
            &[
                obs("search", "1", "Avery Example", "lead", json!({})),
                record("record", "2"),
            ],
            &no_errors(),
        )
        .unwrap();
    let ids: Vec<i64> = store
        .get_run(&run_id)
        .unwrap()
        .observations
        .iter()
        .map(observation_id)
        .collect();
    let refused = store.link(ids[0], ids[1], "Same name").unwrap_err();
    assert!(matches!(refused, Error::Invalid(_)));
    let duplicate = store
        .add_run(
            "duplicate",
            &[record("same", "one"), record("same", "one")],
            &no_errors(),
        )
        .unwrap_err();
    assert!(matches!(duplicate, Error::Sqlite(_)), "{duplicate}");
    assert!(duplicate.to_string().contains("UNIQUE constraint failed"));
    assert_eq!(
        store.list_runs().unwrap().len(),
        1,
        "the failed run left nothing behind"
    );
}

#[test]
fn link_rules_and_messages() {
    let dir = Scratch::new();
    let store = Store::open(&dir.join("cases.db")).unwrap();
    let run_id = store
        .add_run(
            "rules",
            &[
                record("a", "1"),
                record("b", "2"),
                Observation::from_mapping(&json!({"source": "c", "source_id": "3",
                    "name": "Avery Example", "subject_type": "organization", "kind": "record"}))
                .unwrap(),
            ],
            &no_errors(),
        )
        .unwrap();
    let ids: Vec<i64> = store
        .get_run(&run_id)
        .unwrap()
        .observations
        .iter()
        .map(observation_id)
        .collect();
    let message = |result: osintropy::error::Result<()>| result.unwrap_err().to_string();
    assert_eq!(
        message(store.link(ids[0], ids[0], "x")),
        "link two different positive observation IDs"
    );
    assert_eq!(
        message(store.link(0, ids[0], "x")),
        "link two different positive observation IDs"
    );
    assert_eq!(
        message(store.link(ids[0], ids[1], "   ")),
        "link reason is required"
    );
    assert_eq!(
        message(store.link(ids[0], 999, "x")),
        "both observations must exist and neither may be an unverified lead"
    );
    assert_eq!(
        message(store.link(ids[0], ids[2], "x")),
        "linked observations must have the same subject type"
    );
    store.link(ids[1], ids[0], "first reason").unwrap();
    store.link(ids[0], ids[1], "second reason").unwrap();
    let links = store.get_run(&run_id).unwrap().reviewed_links;
    assert_eq!(links.len(), 1);
    assert_eq!((links[0].a, links[0].b), (ids[0], ids[1]));
    assert_eq!(links[0].reason, "second reason");
    assert_eq!(store.unlink(ids[1], ids[0]).unwrap(), 1);
    assert_eq!(store.unlink(ids[1], ids[0]).unwrap(), 0);
}

#[test]
fn cross_run_link_can_be_inspected() {
    let dir = Scratch::new();
    let store = Store::open(&dir.join("cases.db")).unwrap();
    let first = store
        .add_run("case 1", &[record("one", "1")], &no_errors())
        .unwrap();
    let second = store
        .add_run("case 2", &[record("two", "2")], &no_errors())
        .unwrap();
    let a = observation_id(&store.get_run(&first).unwrap().observations[0]);
    let b = observation_id(&store.get_run(&second).unwrap().observations[0]);
    store.link(a, b, "Checked original IDs").unwrap();
    assert_eq!(store.get_observation(b).unwrap().0, second);
    assert_eq!(store.get_run(&first).unwrap().reviewed_links[0].b, b);
    // A link reaching outside the run is listed but does not group inside it.
    assert!(summarize(&store.get_run(&first).unwrap())
        .reviewed_groups
        .is_empty());
}

#[test]
fn exports_escape_html_formulas_and_parse_graph() {
    let dir = Scratch::new();
    let store = Store::open(&dir.join("cases.db")).unwrap();
    let run_id = store
        .add_run(
            "<script>alert(1)</script>",
            &[obs(
                "test",
                "1",
                "<img src=x>",
                "import",
                json!({"note": "=HYPERLINK(\"x\")"}),
            )],
            &no_errors(),
        )
        .unwrap();
    let run = store.get_run(&run_id).unwrap();
    let html = render(&run, Format::Html);
    assert!(!html.contains("<img src=x>"));
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("&lt;img src=x&gt;"));
    assert!(html.contains("default-src 'none'"));
    let rows = csvread::records(&render(&run, Format::Csv)).unwrap();
    assert_eq!(rows[1][8], "'=HYPERLINK(\"x\")");
    let graph = render(&run, Format::GraphMl);
    assert!(graph.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<graphml"));
    assert_eq!(graph.matches("<node ").count(), 2);
    assert!(graph.contains("&lt;img src=x&gt;"));
    let saved = paths::save(&dir.join("case.json"), &render(&run, Format::Json)).unwrap();
    let parsed: Value = serde_json::from_str(&fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(parsed["analysis"]["observation_count"], 1);
    let (query, imported) = load_file(&saved).unwrap();
    assert_eq!(query, run.query);
    assert_eq!(imported[0].fields["note"], ["=HYPERLINK(\"x\")"]);
}

#[test]
fn empty_runs_render_in_every_format() {
    let dir = Scratch::new();
    let store = Store::open(&dir.join("cases.db")).unwrap();
    let mut errors = BTreeMap::new();
    errors.insert("npi".to_owned(), "HTTP 500".to_owned());
    let run_id = store.add_run("Nobody Here", &[], &errors).unwrap();
    let run = store.get_run(&run_id).unwrap();
    assert_eq!(run.status, "failed");
    assert!(render(&run, Format::Html).contains("No observations returned."));
    assert!(render(&run, Format::Html).contains("<li>npi: HTTP 500</li>"));
    assert!(render(&run, Format::GraphMl).contains("edgedefault=\"undirected\" /></graphml>"));
    assert_eq!(render(&run, Format::Csv).lines().count(), 1);
    let doc: Value = serde_json::from_str(&render(&run, Format::Json)).unwrap();
    assert_eq!(doc["errors"]["npi"], "HTTP 500");
    assert_eq!(doc["analysis"]["source_diversity_bits"], 0.0);
}

#[test]
fn json_csv_and_legacy_import() {
    let dir = Scratch::new();
    let path = dir.join("records.json");
    fs::write(
        &path,
        r#"{"query": "test", "records": [{"source": "local", "name": "Avery Example", "fields": {"city": "Sampleton"}}]}"#,
    )
    .unwrap();
    let (query, items) = load_file(&path).unwrap();
    assert_eq!(
        (query.as_str(), items[0].fields["city"].clone()),
        ("test", vec!["Sampleton".to_owned()])
    );
    assert_eq!(items[0].kind, Kind::Import);

    let csv_path = dir.join("other.csv");
    fs::write(
        &csv_path,
        "\u{feff}source,name,occupation\nlocal,Avery Example,Engineer\n",
    )
    .unwrap();
    let (query, items) = load_file(&csv_path).unwrap();
    assert_eq!(query, "other");
    assert_eq!(items[0].fields["occupation"], ["Engineer"]);

    let legacy = dir.join("old.db");
    {
        let db = rusqlite::Connection::open(&legacy).unwrap();
        db.execute(
            "CREATE TABLE person_records(primary_id TEXT, data TEXT)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO person_records VALUES (?, ?)",
            [
                "old-1",
                r#"{"names": ["Avery Example", "A. Example"], "sources": ["broker"], "phone_numbers": [{"number": "555-0100"}]}"#,
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO person_records VALUES (?, ?)",
            ["old-2", r#"{"names": []}"#],
        )
        .unwrap();
    }
    let old = load_legacy_db(&legacy).unwrap();
    assert_eq!(old.len(), 1);
    assert!(old[0].fields["provenance_warning"][0].contains("Legacy code merged"));
    assert_eq!(old[0].fields["aliases"], ["A. Example"]);
    assert_eq!(old[0].fields["phone"], ["555-0100"]);
    assert_eq!(
        (old[0].source.as_str(), old[0].source_id.as_str()),
        ("legacy_db", "old-1")
    );
    // The legacy file is read, never written: it can be opened again afterwards and
    // removed at once (on Windows an open handle would block this).
    fs::remove_file(&legacy).unwrap();
}

#[test]
fn import_limits_and_shapes() {
    let dir = Scratch::new();
    let message = |name: &str, body: &str| {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        load_file(&path).unwrap_err().to_string()
    };
    assert_eq!(
        message("a.txt", "x"),
        "import file must end in .json or .csv"
    );
    assert_eq!(
        message("b.json", "5"),
        "JSON import must be a list or an object with observations/records"
    );
    assert_eq!(
        message("c.json", r#"{"query": 5, "records": []}"#),
        "JSON import needs a list of observations and a string query"
    );
    assert_eq!(
        message("d.csv", "who,what\nx,y\n"),
        "CSV requires source and name columns"
    );
    assert_eq!(message("e.csv", ""), "CSV requires source and name columns");
    assert_eq!(
        message("f.csv", "source,name,fields_json\nx,y,[1]\n"),
        "fields_json must be a JSON object"
    );
    assert_eq!(
        message("g.csv", "source,name\nx,y,extra\n"),
        "invalid field name"
    );
    let many: Vec<Value> = (0..10_001)
        .map(|i| json!({"source": "s", "source_id": i.to_string(), "name": "n"}))
        .collect();
    assert_eq!(
        message("h.json", &serde_json::to_string(&many).unwrap()),
        "import exceeds 10,000 observations"
    );
    let big = dir.join("big.json");
    let file = fs::File::create(&big).unwrap();
    file.set_len(20_000_001).unwrap();
    assert_eq!(
        load_file(&big).unwrap_err().to_string(),
        "import file exceeds the 20 MB limit"
    );
    // CSV columns: metadata, fields_json merged with extra columns, empty cells ignored.
    let path = dir.join("full.csv");
    fs::write(
        &path,
        "source,name,source_id,kind,subject_type,collected_at,fields_json,city\r\n\
         reg,Example Ltd,R-1,record,organization,2025-01-01T00:00:00Z,\"{\"\"city\"\": \"\"Old\"\", \"\"lei\"\": [\"\"L1\"\"]}\",Cardiff\r\n\
         reg,Avery Example,,,,2025-01-01T00:00:00Z,,\r\n",
    )
    .unwrap();
    let (_, items) = load_file(&path).unwrap();
    assert_eq!(items[0].subject_type, SubjectType::Organization);
    assert_eq!(items[0].kind, Kind::Record);
    assert_eq!(items[0].fields["city"], ["Cardiff"]);
    assert_eq!(items[0].fields["lei"], ["L1"]);
    assert_eq!(items[1].kind, Kind::Import);
    assert!(items[1].source_id.starts_with("content:"));
}

#[test]
fn existing_database_migrates_and_cross_type_links_are_rejected() {
    let dir = Scratch::new();
    let path = dir.join("old.db");
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE runs(id TEXT PRIMARY KEY, query TEXT NOT NULL, created_at TEXT NOT NULL,
                               status TEXT NOT NULL, errors TEXT NOT NULL);
             CREATE TABLE observations(id INTEGER PRIMARY KEY, run_id TEXT NOT NULL,
                 source TEXT NOT NULL, source_id TEXT NOT NULL, name TEXT NOT NULL,
                 fields TEXT NOT NULL, source_url TEXT NOT NULL, kind TEXT NOT NULL,
                 collected_at TEXT NOT NULL, UNIQUE(run_id,source,source_id));
             CREATE TABLE links(a INTEGER NOT NULL,b INTEGER NOT NULL,reason TEXT NOT NULL,
                 created_at TEXT NOT NULL,PRIMARY KEY(a,b));
             INSERT INTO runs VALUES ('old','Old case','2025-01-01T00:00:00+00:00','complete','{}');
             INSERT INTO observations(run_id,source,source_id,name,fields,source_url,kind,collected_at)
                 VALUES ('old','registry','1','Avery Example','{}','','record','2025-01-01T00:00:00+00:00');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let old = store.get_run("old").unwrap();
    assert_eq!(old.observations[0].subject_type, SubjectType::Person);
    let org_run = store
        .add_run(
            "org",
            &[
                Observation::from_mapping(&json!({"source": "gleif", "source_id": "2",
                "name": "Avery Example", "subject_type": "organization"}))
                .unwrap(),
            ],
            &no_errors(),
        )
        .unwrap();
    let a = observation_id(&old.observations[0]);
    let b = observation_id(&store.get_run(&org_run).unwrap().observations[0]);
    let refused = store.link(a, b, "Same name").unwrap_err().to_string();
    assert!(refused.contains("same subject type"));
    let json = render(&store.get_run(&org_run).unwrap(), Format::Json);
    assert_eq!(
        json.matches("\"subject_type\": \"organization\"").count(),
        1
    );
}

#[test]
fn prune_removes_only_older_runs() {
    let dir = Scratch::new();
    let path = dir.join("cases.db");
    let store = Store::open(&path).unwrap();
    let keep = store
        .add_run("new", &[record("a", "1")], &no_errors())
        .unwrap();
    let old = store
        .add_run("old", &[record("b", "2")], &no_errors())
        .unwrap();
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute(
            "UPDATE runs SET created_at='2001-01-01T00:00:00+00:00' WHERE id=?",
            [&old],
        )
        .unwrap();
    }
    let before = osintropy::isotime::utc_days_ago(30).unwrap();
    assert_eq!(store.prune(&before).unwrap(), 1);
    let left: Vec<String> = store
        .list_runs()
        .unwrap()
        .into_iter()
        .map(|run| run.id)
        .collect();
    assert_eq!(left, [keep]);
}

#[test]
fn new_case_files_are_private_and_tolerate_odd_paths() {
    let dir = Scratch::new();
    let nested = dir.join("a").join("b").join("cases.db");
    Store::open(&nested).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "only the owner may read a new case file");
    }
    let spaced = dir.join("with space é").join("cases.db");
    let store = Store::open(&spaced).unwrap();
    let backup = store
        .backup(&dir.join("another folder").join("copy.db"))
        .unwrap();
    assert!(backup.exists());
}
