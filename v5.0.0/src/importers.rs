// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Local data ingestion: JSON and CSV observation files, and the original project's
//! SQLite `person_records` format.

use std::fs;
use std::path::Path;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::csvread::{self, Cell};
use crate::error::{Error, Result};
use crate::models::Observation;
use crate::paths::expand_user;
use crate::pyjson;

pub const MAX_FILE_BYTES: u64 = 20_000_000;
pub const MAX_OBSERVATIONS: usize = 10_000;

const CSV_METADATA: [&str; 8] = [
    "source",
    "name",
    "source_id",
    "source_url",
    "kind",
    "subject_type",
    "collected_at",
    "fields_json",
];

/// Python's `Path.stem`: the file name without its last extension.
fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_utf8(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::io_at(path, err))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::invalid(format!("{} is not valid UTF-8 text", path.display())))?;
    // Python's "utf-8-sig": one byte-order mark at the start is dropped.
    Ok(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_owned(),
        None => text,
    })
}

/// Read a `.json` or `.csv` observation file; returns its query and observations.
///
/// JSON is a list of observations, or an object with `query` and `records` (or
/// `observations`). CSV needs `source` and `name` columns; `source_id`, `source_url`,
/// `kind`, `subject_type`, `collected_at` and `fields_json` are understood, and any other
/// column becomes a field. Files are capped at 20 MB and 10,000 observations.
pub fn load_file(path: &Path) -> Result<(String, Vec<Observation>)> {
    let file = expand_user(path);
    let size = fs::metadata(&file)
        .map_err(|err| Error::io_at(&file, err))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(Error::invalid("import file exceeds the 20 MB limit"));
    }
    let extension = file
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let (query, rows) = match extension.as_str() {
        "json" => load_json(&file)?,
        "csv" => load_csv(&file)?,
        _ => return Err(Error::invalid("import file must end in .json or .csv")),
    };
    if rows.len() > MAX_OBSERVATIONS {
        return Err(Error::invalid("import exceeds 10,000 observations"));
    }
    let observations = rows
        .iter()
        .map(|(row, unnamed_extra)| match row {
            Value::Object(map) => Observation::from_object(map, *unnamed_extra),
            other => Observation::from_mapping(other),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok((query, observations))
}

type Rows = Vec<(Value, bool)>;

fn load_json(file: &Path) -> Result<(String, Rows)> {
    let text = read_utf8(file)?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|err| Error::invalid(format!("invalid JSON in {}: {err}", file.display())))?;
    let (rows, query) = match data {
        Value::Array(rows) => (Value::Array(rows), Value::String(stem(file))),
        Value::Object(mut map) => {
            let rows = match map.shift_remove("observations") {
                Some(rows) => rows,
                None => map.shift_remove("records").unwrap_or(Value::Null),
            };
            let query = map
                .shift_remove("query")
                .unwrap_or_else(|| Value::String(stem(file)));
            (rows, query)
        }
        _ => {
            return Err(Error::invalid(
                "JSON import must be a list or an object with observations/records",
            ))
        }
    };
    match (rows, query) {
        (Value::Array(rows), Value::String(query)) => {
            Ok((query, rows.into_iter().map(|row| (row, false)).collect()))
        }
        _ => Err(Error::invalid(
            "JSON import needs a list of observations and a string query",
        )),
    }
}

fn load_csv(file: &Path) -> Result<(String, Rows)> {
    let text = read_utf8(file)?;
    let (header, rows) = csvread::dict_rows(&text).map_err(Error::Invalid)?;
    let header = header.unwrap_or_default();
    if header.is_empty()
        || !header.iter().any(|name| name == "source")
        || !header.iter().any(|name| name == "name")
    {
        return Err(Error::invalid("CSV requires source and name columns"));
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let fields_text = row.get("fields_json").filter(|text| !text.is_empty());
        let mut fields = match fields_text {
            Some(text) => match serde_json::from_str::<Value>(text) {
                Ok(Value::Object(map)) => map,
                Ok(_) => return Err(Error::invalid("fields_json must be a JSON object")),
                Err(err) => return Err(Error::invalid(format!("invalid fields_json: {err}"))),
            },
            None => Map::new(),
        };
        let mut mapping = Map::new();
        for (key, cell) in &row.cells {
            let Cell::Text(value) = cell else { continue };
            if value.is_empty() {
                continue;
            }
            if CSV_METADATA.contains(&key.as_str()) {
                mapping.insert(key.clone(), Value::String(value.clone()));
            } else {
                fields.insert(key.clone(), Value::String(value.clone()));
            }
        }
        mapping.insert("fields".into(), Value::Object(fields));
        out.push((Value::Object(mapping), row.extra.is_some()));
    }
    Ok((stem(file), out))
}

/// Python's `str()` of a legacy column value.
fn legacy_text(value: SqlValue) -> Option<String> {
    match value {
        SqlValue::Text(text) => Some(text),
        SqlValue::Integer(number) => Some(number.to_string()),
        SqlValue::Real(number) => Some(crate::pyfmt::float_repr(number)),
        SqlValue::Null => Some("None".to_owned()),
        SqlValue::Blob(_) => None,
    }
}

/// A legacy list field: lists pass through, `null` is empty; Python iterated strings and
/// objects too, which never yield usable values, so they count as empty.
fn legacy_list<'a>(data: &'a Map<String, Value>, key: &str) -> Option<Vec<&'a Value>> {
    match data.get(key) {
        None => Some(Vec::new()),
        Some(Value::Array(items)) => Some(items.iter().collect()),
        Some(Value::String(_) | Value::Object(_)) => Some(Vec::new()),
        Some(_) => None,
    }
}

/// Read the original project's `person_records` table without changing the file. Its
/// records were already merged by older code, so each becomes one unverified imported
/// observation carrying a provenance warning.
pub fn load_legacy_db(path: &Path) -> Result<Vec<Observation>> {
    let file = expand_user(path);
    // The file must exist, as Python's `resolve(strict=True)` required. It is opened by the
    // name given, because a resolved Windows path (`\\?\C:\...`) is not one to hand SQLite.
    fs::canonicalize(&file).map_err(|err| Error::io_at(&file, err))?;
    let rows: Vec<(SqlValue, SqlValue)> = {
        let db = Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let exists: Option<i64> = db
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='person_records'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(Error::invalid("not an OSINTropy legacy database"));
        }
        let mut statement =
            db.prepare("SELECT primary_id, data FROM person_records LIMIT 10001")?;
        let found = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        found
    };
    if rows.len() > MAX_OBSERVATIONS {
        return Err(Error::invalid("legacy import exceeds 10,000 records"));
    }
    let bad = || Error::invalid("invalid legacy record");
    let mut items = Vec::new();
    for (primary_id, payload) in rows {
        let payload = match payload {
            SqlValue::Text(text) => text,
            SqlValue::Blob(bytes) => String::from_utf8(bytes).map_err(|_| bad())?,
            _ => return Err(bad()),
        };
        let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&payload) else {
            return Err(bad());
        };
        let names: Vec<&Value> = match data.get("names") {
            Some(Value::Array(names)) => names.iter().collect(),
            Some(value) if pyjson::truthy(value) => return Err(bad()),
            _ => Vec::new(),
        };
        let Some(first) = names.first() else { continue };
        let name = pyjson::scalar_text(first).ok_or_else(bad)?;
        let pick = |key: &str, inner: &str| -> Option<Vec<Value>> {
            Some(
                legacy_list(&data, key)?
                    .into_iter()
                    .filter_map(|item| item.as_object())
                    .map(|item| item.get(inner).cloned().unwrap_or(Value::Null))
                    .collect(),
            )
        };
        let fields = json!({
            "provenance_warning":
                "Legacy code merged source hits without preserving field provenance; verify each claim.",
            "legacy_sources": data.get("sources").cloned().unwrap_or_else(|| json!([])),
            "aliases": names[1..].iter().map(|value| (*value).clone()).collect::<Vec<_>>(),
            "email": data.get("emails").cloned().unwrap_or_else(|| json!([])),
            "phone": pick("phone_numbers", "number").ok_or_else(bad)?,
            "address": pick("addresses", "full_address").ok_or_else(bad)?,
            "employer": data.get("employers").cloned().unwrap_or_else(|| json!([])),
            "occupation": data.get("occupations").cloned().unwrap_or_else(|| json!([])),
        });
        let source_id = legacy_text(primary_id).ok_or_else(bad)?;
        items.push(Observation::from_mapping(&json!({
            "source": "legacy_db",
            "source_id": source_id,
            "name": name,
            "kind": "import",
            "fields": fields,
        }))?);
    }
    Ok(items)
}
