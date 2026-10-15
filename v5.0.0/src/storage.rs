// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The SQLite case file: runs, their observations, and reviewed links, with cascading
//! deletion. The schema is 4.1.1's to the character, so either version opens the other's
//! files, and 4.0 files gain the `subject_type` column in place.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::isotime::utc_now;
use crate::models::{clean_str, Kind, Link, Observation, Run};
use crate::paths;
use crate::pyjson::{Json, Style};

/// 4.1.1's schema, whitespace included: SQLite keeps the text of each CREATE statement.
const SCHEMA: &str = "
                CREATE TABLE IF NOT EXISTS runs (
                    id TEXT PRIMARY KEY, query TEXT NOT NULL, created_at TEXT NOT NULL,
                    status TEXT NOT NULL, errors TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS observations (
                    id INTEGER PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                    source TEXT NOT NULL, source_id TEXT NOT NULL, name TEXT NOT NULL,
                    fields TEXT NOT NULL, source_url TEXT NOT NULL,
                    kind TEXT NOT NULL, collected_at TEXT NOT NULL,
                    subject_type TEXT NOT NULL DEFAULT 'person',
                    UNIQUE(run_id, source, source_id)
                );
                CREATE INDEX IF NOT EXISTS idx_observations_run ON observations(run_id);
                CREATE TABLE IF NOT EXISTS links (
                    a INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
                    b INTEGER NOT NULL REFERENCES observations(id) ON DELETE CASCADE,
                    reason TEXT NOT NULL, created_at TEXT NOT NULL,
                    PRIMARY KEY(a, b), CHECK(a < b)
                );
            ";

/// One line of `list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSummary {
    pub id: String,
    pub query: String,
    pub created_at: String,
    pub status: String,
    pub count: i64,
}

/// A case file on disk. Each operation opens its own connection and closes it after, so
/// no file stays locked between commands (this matters on Windows).
#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// Open a case file, creating it (readable by this user only) and its folder if needed,
    /// and bring an older schema up to date.
    pub fn open(path: &Path) -> Result<Store> {
        let path = paths::expand_user(path);
        paths::ensure_parent(&path).map_err(|err| Error::io_at(&path, err))?;
        paths::create_private(&path).map_err(|err| Error::io_at(&path, err))?;
        let store = Store { path };
        let db = store.connect()?;
        db.execute_batch(SCHEMA)?;
        let has_subject_type = {
            let mut statement = db.prepare("PRAGMA table_info(observations)")?;
            let names = statement.query_map([], |row| row.get::<_, String>(1))?;
            let mut found = false;
            for name in names {
                found |= name? == "subject_type";
            }
            found
        };
        if !has_subject_type {
            db.execute(
                "ALTER TABLE observations ADD COLUMN subject_type TEXT NOT NULL DEFAULT 'person'",
                [],
            )?;
        }
        Ok(store)
    }

    /// The case file's path, after `~` expansion.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection> {
        let db = Connection::open(&self.path)?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=10000;")?;
        Ok(db)
    }

    /// Save a run and its observations in one transaction and return its new ID. Status is
    /// `complete`, `partial` (some sources failed) or `failed` (errors and no results).
    pub fn add_run(
        &self,
        query: &str,
        observations: &[Observation],
        errors: &BTreeMap<String, String>,
    ) -> Result<String> {
        let query = clean_str(query, 500)?;
        if query.is_empty() {
            return Err(Error::invalid("query is required"));
        }
        // Validate everything before the transaction begins.
        let items = observations
            .iter()
            .map(|item| Observation::from_mapping(&item.to_value()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let status = match (errors.is_empty(), items.is_empty()) {
            (true, _) => "complete",
            (false, false) => "partial",
            (false, true) => "failed",
        };
        let errors_json = Json::Object(
            errors
                .iter()
                .map(|(source, message)| (source.clone(), Json::str(message)))
                .collect(),
        )
        .dumps(Style::COMPACT_ASCII);
        let run_id = Uuid::new_v4().simple().to_string();
        let mut db = self.connect()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        tx.execute(
            "INSERT INTO runs VALUES (?, ?, ?, ?, ?)",
            params![run_id, query, utc_now(), status, errors_json],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO observations
                    (run_id, source, source_id, name, fields, source_url, kind, collected_at, subject_type)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            for item in &items {
                insert.execute(params![
                    run_id,
                    item.source,
                    item.source_id,
                    item.name,
                    crate::models::fields_json(&item.fields).dumps(Style::COMPACT),
                    item.source_url,
                    item.kind.as_str(),
                    item.collected_at,
                    item.subject_type.as_str(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(run_id)
    }

    /// A run with its observations and every reviewed link that touches them.
    pub fn get_run(&self, run_id: &str) -> Result<Run> {
        let db = self.connect()?;
        let header = db
            .query_row(
                "SELECT id, query, created_at, status, errors FROM runs WHERE id = ?",
                [run_id],
                |row| {
                    Ok((
                        text(row, "id")?,
                        text(row, "query")?,
                        text(row, "created_at")?,
                        text(row, "status")?,
                        text(row, "errors")?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("run {run_id} not found")))?;
        let mut statement =
            db.prepare("SELECT * FROM observations WHERE run_id = ? ORDER BY id")?;
        let rows = statement.query_map([run_id], read_observation_row)?;
        let mut observations = Vec::new();
        for row in rows {
            let (_, mapping) = row?;
            observations.push(Observation::from_mapping(&mapping?)?);
        }
        let mut statement = db.prepare(
            "SELECT a, b, reason, created_at FROM links
                WHERE a IN (SELECT id FROM observations WHERE run_id = ?)
                   OR b IN (SELECT id FROM observations WHERE run_id = ?)
                ORDER BY a, b",
        )?;
        let links = statement
            .query_map([run_id, run_id], |row| {
                Ok(Link {
                    a: row.get("a")?,
                    b: row.get("b")?,
                    reason: text(row, "reason")?,
                    created_at: text(row, "created_at")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let errors = parse_errors(&header.4)
            .ok_or_else(|| Error::invalid(format!("run {run_id} has an unreadable error list")))?;
        Ok(Run {
            id: header.0,
            query: header.1,
            created_at: header.2,
            status: header.3,
            errors,
            observations,
            reviewed_links: links,
        })
    }

    /// Every run, newest first, with its observation count.
    pub fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let db = self.connect()?;
        let mut statement = db.prepare(
            "SELECT r.*, COUNT(o.id) AS count FROM runs r
                LEFT JOIN observations o ON r.id=o.run_id GROUP BY r.id
                ORDER BY r.created_at DESC, r.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(RunSummary {
                id: text(row, "id")?,
                query: text(row, "query")?,
                created_at: text(row, "created_at")?,
                status: text(row, "status")?,
                count: row.get("count")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One observation and the ID of the run it belongs to.
    pub fn get_observation(&self, observation_id: i64) -> Result<(String, Observation)> {
        let db = self.connect()?;
        let found = db
            .query_row(
                "SELECT * FROM observations WHERE id=?",
                [observation_id],
                read_observation_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("observation {observation_id} not found")))?;
        let (run_id, mapping) = found;
        Ok((run_id, Observation::from_mapping(&mapping?)?))
    }

    /// Record a reviewed link. Both must be records of the same subject type, never leads;
    /// the reason is required. Linking again replaces the reason and time.
    pub fn link(&self, first: i64, second: i64, reason: &str) -> Result<()> {
        if first == second || first.min(second) < 1 {
            return Err(Error::invalid(
                "link two different positive observation IDs",
            ));
        }
        let reason = clean_str(reason, 500)?;
        if reason.is_empty() {
            return Err(Error::invalid("link reason is required"));
        }
        let (a, b) = (first.min(second), first.max(second));
        let mut db = self.connect()?;
        let tx = db.transaction()?;
        let rows = {
            let mut statement =
                tx.prepare("SELECT id, kind, subject_type FROM observations WHERE id IN (?, ?)")?;
            let found = statement
                .query_map([a, b], |row| {
                    Ok((text(row, "kind")?, text(row, "subject_type")?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            found
        };
        if rows.len() != 2 || rows.iter().any(|(kind, _)| kind == Kind::Lead.as_str()) {
            return Err(Error::invalid(
                "both observations must exist and neither may be an unverified lead",
            ));
        }
        if rows[0].1 != rows[1].1 {
            return Err(Error::invalid(
                "linked observations must have the same subject type",
            ));
        }
        tx.execute(
            "INSERT INTO links VALUES (?, ?, ?, ?) ON CONFLICT(a,b) DO UPDATE SET reason=excluded.reason, created_at=excluded.created_at",
            params![a, b, reason, utc_now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Remove a reviewed link; returns how many were removed (0 or 1).
    pub fn unlink(&self, first: i64, second: i64) -> Result<usize> {
        let (a, b) = (first.min(second), first.max(second));
        let db = self.connect()?;
        Ok(db.execute("DELETE FROM links WHERE a=? AND b=?", [a, b])?)
    }

    /// Delete a run, its observations and their links; returns runs deleted (0 or 1).
    pub fn delete_run(&self, run_id: &str) -> Result<usize> {
        let db = self.connect()?;
        Ok(db.execute("DELETE FROM runs WHERE id=?", [run_id])?)
    }

    /// Delete every run created before `before` (an ISO timestamp); returns runs deleted.
    pub fn prune(&self, before: &str) -> Result<usize> {
        let db = self.connect()?;
        Ok(db.execute("DELETE FROM runs WHERE created_at < ?", [before])?)
    }

    /// Copy the case file with SQLite's backup API into a private temporary file beside the
    /// destination, close it, then move it into place. Returns the destination.
    pub fn backup(&self, destination: &Path) -> Result<PathBuf> {
        let target = paths::expand_user(destination);
        paths::ensure_parent(&target).map_err(|err| Error::io_at(&target, err))?;
        if paths::resolve(&target) == paths::resolve(&self.path) {
            return Err(Error::invalid(
                "backup destination must differ from the active database",
            ));
        }
        let folder = paths::parent_dir(&target);
        let (temp, file) = paths::create_temp(&folder, ".osintropy-backup-", ".db")
            .map_err(|err| Error::io_at(&folder, err))?;
        drop(file);
        let copied = self
            .copy_into(&temp)
            .and_then(|()| fs::rename(&temp, &target).map_err(|err| Error::io_at(&target, err)));
        if temp.exists() {
            let _ = fs::remove_file(&temp);
        }
        copied.map(|()| target)
    }

    fn copy_into(&self, temp: &Path) -> Result<()> {
        let source = self.connect()?;
        let mut copy = Connection::open(temp)?;
        {
            // All pages in one step, as Python's `Connection.backup` does by default. A
            // busy or locked source is retried every 250 ms, for up to 10 seconds.
            let backup = Backup::new(&source, &mut copy)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match backup.step(-1)? {
                    StepResult::Done => break,
                    StepResult::Busy | StepResult::Locked if Instant::now() >= deadline => {
                        return Err(Error::invalid(
                            "the case file stayed locked for 10 seconds; try again when nothing else is writing to it",
                        ));
                    }
                    StepResult::Busy | StepResult::Locked => {
                        thread::sleep(Duration::from_millis(250))
                    }
                    _ => {}
                }
            }
        }
        // Both connections close here, before the file is moved.
        copy.close().map_err(|(_, err)| Error::Sqlite(err))?;
        source.close().map_err(|(_, err)| Error::Sqlite(err))?;
        Ok(())
    }
}

/// A TEXT column. Other storage classes are read as Python would print them.
fn text(row: &Row<'_>, name: &str) -> rusqlite::Result<String> {
    Ok(match row.get::<_, SqlValue>(name)? {
        SqlValue::Text(value) => value,
        SqlValue::Integer(value) => value.to_string(),
        SqlValue::Real(value) => crate::pyfmt::float_repr(value),
        SqlValue::Null => "None".to_owned(),
        SqlValue::Blob(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// A column as the JSON value Python's sqlite3 would have handed to `from_mapping`.
fn json_column(row: &Row<'_>, name: &str) -> rusqlite::Result<Value> {
    Ok(match row.get::<_, SqlValue>(name)? {
        SqlValue::Text(value) => Value::String(value),
        SqlValue::Integer(value) => Value::from(value),
        SqlValue::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        SqlValue::Null => Value::Null,
        SqlValue::Blob(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
    })
}

type ObservationRow = (String, std::result::Result<Value, Error>);

/// An observation row as the mapping `Observation::from_mapping` checks, plus its run ID.
fn read_observation_row(row: &Row<'_>) -> rusqlite::Result<ObservationRow> {
    let run_id = text(row, "run_id")?;
    let fields_text = text(row, "fields")?;
    let fields = serde_json::from_str::<Value>(&fields_text)
        .map_err(|err| Error::invalid(format!("stored fields are not valid JSON: {err}")));
    let mut map = Map::new();
    map.insert("id".into(), json_column(row, "id")?);
    map.insert("source".into(), json_column(row, "source")?);
    map.insert("source_id".into(), json_column(row, "source_id")?);
    map.insert("name".into(), json_column(row, "name")?);
    map.insert("source_url".into(), json_column(row, "source_url")?);
    map.insert("kind".into(), json_column(row, "kind")?);
    map.insert("collected_at".into(), json_column(row, "collected_at")?);
    map.insert("subject_type".into(), json_column(row, "subject_type")?);
    let mapping = fields.map(|fields| {
        map.insert("fields".into(), fields);
        Value::Object(map)
    });
    Ok((run_id, mapping))
}

fn parse_errors(text: &str) -> Option<BTreeMap<String, String>> {
    let Value::Object(map) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    map.into_iter()
        .map(|(source, message)| match message {
            Value::String(message) => Some((source, message)),
            _ => None,
        })
        .collect()
}
