// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Validated observations. A source hit is evidence, not a resolved person.

use std::collections::BTreeMap;
use std::fmt;

use ring::digest::{digest, SHA256};
use serde_json::{Map, Value};

use crate::isotime;
use crate::pyfmt::collapse_whitespace;
use crate::pyjson::{self, Json, Style};
use crate::urlparse::urlsplit;

pub use crate::pyfmt::normalized_name;

/// A rule an observation, query or link broke. The text is shown to the user as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub String);

impl Invalid {
    pub fn new(message: impl Into<String>) -> Self {
        Invalid(message.into())
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Invalid {}

/// How strong an observation is: a source record, an unverified search lead, or local data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Record,
    Lead,
    Import,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Record => "record",
            Kind::Lead => "lead",
            Kind::Import => "import",
        }
    }

    fn parse(text: &str) -> Option<Kind> {
        match text {
            "record" => Some(Kind::Record),
            "lead" => Some(Kind::Lead),
            "import" => Some(Kind::Import),
            _ => None,
        }
    }
}

/// What an observation is about. Types never mix in links or review groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubjectType {
    Person,
    Organization,
    Domain,
}

impl SubjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectType::Person => "person",
            SubjectType::Organization => "organization",
            SubjectType::Domain => "domain",
        }
    }

    fn parse(text: &str) -> Option<SubjectType> {
        match text {
            "person" => Some(SubjectType::Person),
            "organization" => Some(SubjectType::Organization),
            "domain" => Some(SubjectType::Domain),
            _ => None,
        }
    }
}

/// One source hit or imported entry, with its provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub source: String,
    pub source_id: String,
    pub name: String,
    /// Field name to distinct values, first-seen order kept within each field.
    pub fields: BTreeMap<String, Vec<String>>,
    pub source_url: String,
    pub kind: Kind,
    pub subject_type: SubjectType,
    pub collected_at: String,
    pub id: Option<i64>,
}

const DIRECTION_MARKS: [char; 9] = [
    '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}',
    '\u{2069}',
];

/// 4.1.1's `clean_text`: strings and numbers only, whitespace collapsed, no control or
/// text-direction characters, at most `limit` characters. A missing value is empty.
pub fn clean_text(value: &Value, limit: usize) -> Result<String, Invalid> {
    let raw = match value {
        Value::Null => return Ok(String::new()),
        Value::String(text) => text.clone(),
        Value::Number(number) => pyjson::number_text(number),
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => {
            return Err(Invalid::new("field values must be strings or numbers"))
        }
    };
    clean_str(&raw, limit)
}

/// `clean_text` for text already in hand.
pub fn clean_str(raw: &str, limit: usize) -> Result<String, Invalid> {
    let result = collapse_whitespace(raw);
    if result
        .chars()
        .any(|c| u32::from(c) < 32 || c == '\u{7f}' || DIRECTION_MARKS.contains(&c))
    {
        return Err(Invalid::new(
            "field contains control or direction characters",
        ));
    }
    if result.chars().count() > limit {
        return Err(Invalid(format!("field exceeds {limit} characters")));
    }
    Ok(result)
}

impl Observation {
    /// Validate a JSON object the way 4.1.1's `Observation.from_mapping` did, checks in
    /// the same order so the same mistake gives the same message.
    pub fn from_mapping(data: &Value) -> Result<Observation, Invalid> {
        match data {
            Value::Object(map) => Self::from_object(map, false),
            _ => Err(Invalid::new("observation must be an object")),
        }
    }

    /// As `from_mapping`; `unnamed_extra` stands for the nameless field Python's
    /// `csv.DictReader` adds when a row has more cells than the header.
    pub(crate) fn from_object(
        data: &Map<String, Value>,
        unnamed_extra: bool,
    ) -> Result<Observation, Invalid> {
        let null = Value::Null;
        let source = clean_text(data.get("source").unwrap_or(&null), 100)?;
        let name = clean_text(data.get("name").unwrap_or(&null), 250)?;
        if source.is_empty() || name.is_empty() {
            return Err(Invalid::new("source and name are required"));
        }
        let kind = match data.get("kind") {
            Some(value) => clean_text(value, 20)?,
            None => "import".to_owned(),
        };
        let kind = Kind::parse(&kind)
            .ok_or_else(|| Invalid::new("kind must be record, lead, or import"))?;
        let subject_type = match data.get("subject_type") {
            Some(value) => clean_text(value, 20)?,
            None => "person".to_owned(),
        };
        let subject_type = SubjectType::parse(&subject_type)
            .ok_or_else(|| Invalid::new("subject_type must be person, organization, or domain"))?;
        let source_url = clean_text(data.get("source_url").unwrap_or(&null), 2000)?;
        if !source_url.is_empty() {
            check_source_url(&source_url)?;
        }
        let fields = clean_fields(data.get("fields"), unnamed_extra)?;
        let collected_at = match data.get("collected_at") {
            Some(value) => clean_text(value, 64)?,
            None => isotime::utc_now(),
        };
        let collected_at = isotime::normalize_with_offset(&collected_at).ok_or_else(|| {
            Invalid::new("collected_at must be an ISO 8601 timestamp with timezone")
        })?;
        let mut source_id = clean_text(data.get("source_id").unwrap_or(&null), 250)?;
        if source_id.is_empty() {
            source_id = content_id(&source, &name, &source_url, &fields);
        }
        let id = match data.get("id") {
            None | Some(Value::Null) => None,
            Some(value) => Some(positive_id(value)?),
        };
        Ok(Observation {
            source,
            source_id,
            name,
            fields,
            source_url,
            kind,
            subject_type,
            collected_at,
            id: id.flatten(),
        })
    }

    /// The observation as Python's `dataclasses.asdict` laid it out.
    pub fn to_json(&self) -> Json {
        Json::object([
            ("source", Json::str(&self.source)),
            ("source_id", Json::str(&self.source_id)),
            ("name", Json::str(&self.name)),
            ("fields", fields_json(&self.fields)),
            ("source_url", Json::str(&self.source_url)),
            ("kind", Json::str(self.kind.as_str())),
            ("subject_type", Json::str(self.subject_type.as_str())),
            ("collected_at", Json::str(&self.collected_at)),
            ("id", self.id.map_or(Json::Null, Json::Int)),
        ])
    }

    /// The observation as the JSON object `from_mapping` reads, for re-validation.
    pub fn to_value(&self) -> Value {
        let fields: Map<String, Value> = self
            .fields
            .iter()
            .map(|(key, values)| {
                let list = values.iter().cloned().map(Value::String).collect();
                (key.clone(), Value::Array(list))
            })
            .collect();
        let mut map = Map::new();
        map.insert("source".into(), Value::String(self.source.clone()));
        map.insert("source_id".into(), Value::String(self.source_id.clone()));
        map.insert("name".into(), Value::String(self.name.clone()));
        map.insert("fields".into(), Value::Object(fields));
        map.insert("source_url".into(), Value::String(self.source_url.clone()));
        map.insert("kind".into(), Value::String(self.kind.as_str().into()));
        map.insert(
            "subject_type".into(),
            Value::String(self.subject_type.as_str().into()),
        );
        map.insert(
            "collected_at".into(),
            Value::String(self.collected_at.clone()),
        );
        map.insert("id".into(), self.id.map_or(Value::Null, Value::from));
        Value::Object(map)
    }
}

/// A reviewed link between two observations, made by a person after checking both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub a: i64,
    pub b: i64,
    pub reason: String,
    pub created_at: String,
}

impl Link {
    /// The link as 4.1.1's reports laid it out.
    pub fn to_json(&self) -> Json {
        Json::object([
            ("a", Json::Int(self.a)),
            ("b", Json::Int(self.b)),
            ("reason", Json::str(&self.reason)),
            ("created_at", Json::str(&self.created_at)),
        ])
    }
}

/// One stored search or import: its observations and every reviewed link touching them.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: String,
    pub query: String,
    pub created_at: String,
    pub status: String,
    pub errors: BTreeMap<String, String>,
    /// Ordered by observation ID; every one has its `id` set.
    pub observations: Vec<Observation>,
    /// Ordered by `a`, then `b`.
    pub reviewed_links: Vec<Link>,
}

impl Run {
    /// The run as 4.1.1's `Store.get_run` returned it.
    pub fn to_json(&self) -> Json {
        Json::object([
            ("id", Json::str(&self.id)),
            ("query", Json::str(&self.query)),
            ("created_at", Json::str(&self.created_at)),
            ("status", Json::str(&self.status)),
            (
                "errors",
                Json::Object(
                    self.errors
                        .iter()
                        .map(|(k, v)| (k.clone(), Json::str(v)))
                        .collect(),
                ),
            ),
            (
                "observations",
                Json::Array(self.observations.iter().map(Observation::to_json).collect()),
            ),
            (
                "reviewed_links",
                Json::Array(self.reviewed_links.iter().map(Link::to_json).collect()),
            ),
        ])
    }
}

/// An observation's stored ID (0 only for one never stored).
pub fn observation_id(item: &Observation) -> i64 {
    item.id.unwrap_or_default()
}

/// The fields object, keys sorted, as stored and reported.
pub fn fields_json(fields: &BTreeMap<String, Vec<String>>) -> Json {
    Json::Object(
        fields
            .iter()
            .map(|(key, values)| (key.clone(), Json::strings(values)))
            .collect(),
    )
}

fn check_source_url(url: &str) -> Result<(), Invalid> {
    let parts = urlsplit(url).map_err(Invalid)?;
    let has_user = parts.username().is_some_and(|user| !user.is_empty());
    let has_password = parts.password().is_some_and(|pass| !pass.is_empty());
    if !matches!(parts.scheme.as_str(), "https" | "http")
        || parts.netloc.is_empty()
        || has_user
        || has_password
    {
        return Err(Invalid::new(
            "source_url must be an HTTP(S) URL without credentials",
        ));
    }
    Ok(())
}

fn clean_fields(
    raw: Option<&Value>,
    unnamed_extra: bool,
) -> Result<BTreeMap<String, Vec<String>>, Invalid> {
    let empty = Map::new();
    let map = match raw {
        None => &empty,
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(Invalid::new(
                "fields must be an object with at most 64 keys",
            ))
        }
    };
    if map.len() + usize::from(unnamed_extra) > 64 {
        return Err(Invalid::new(
            "fields must be an object with at most 64 keys",
        ));
    }
    let mut fields = BTreeMap::new();
    for (key, values) in map {
        if key.is_empty() || key.chars().count() > 64 {
            return Err(Invalid::new("invalid field name"));
        }
        let single;
        let values: &[Value] = match values {
            Value::Array(list) => list,
            other => {
                single = [other.clone()];
                &single
            }
        };
        if values.len() > 100 {
            return Err(Invalid::new("too many field values"));
        }
        let mut cleaned: Vec<String> = Vec::new();
        for value in values {
            let text = clean_text(value, 4096)?;
            if !text.is_empty() && !cleaned.contains(&text) {
                cleaned.push(text);
            }
        }
        if !cleaned.is_empty() {
            fields.insert(key.clone(), cleaned);
        }
    }
    if unnamed_extra {
        return Err(Invalid::new("invalid field name"));
    }
    Ok(fields)
}

/// `"content:"` plus 24 hex characters of the SHA-256 of
/// `json.dumps([source, name, url, fields], sort_keys=True, ensure_ascii=False)`.
pub fn content_id(
    source: &str,
    name: &str,
    url: &str,
    fields: &BTreeMap<String, Vec<String>>,
) -> String {
    let fingerprint = Json::Array(vec![
        Json::str(source),
        Json::str(name),
        Json::str(url),
        fields_json(fields),
    ])
    .dumps(Style::COMPACT);
    format!("content:{}", &sha256_hex(fingerprint.as_bytes())[..24])
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(data: &[u8]) -> String {
    digest(&SHA256, data)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// An `id` must be a positive integer. Python ints have no ceiling; one too large for the
/// database is accepted and dropped, since stored IDs come from the database anyway.
fn positive_id(value: &Value) -> Result<Option<i64>, Invalid> {
    let digits = pyjson::integer_text(value)
        .filter(|text| !text.starts_with('-') && text != "0")
        .ok_or_else(|| Invalid::new("id must be a positive integer"))?;
    Ok(digits.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_like_from_mapping() {
        let item = Observation::from_mapping(&json!({
            "source": " local\tregister ", "name": "Avery  Example",
            "fields": {"city": ["Sampleton", "Sampleton", ""], "n": 7, "empty": [null]},
            "collected_at": "2025-01-01T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(item.source, "local register");
        assert_eq!(item.kind, Kind::Import);
        assert_eq!(item.subject_type, SubjectType::Person);
        assert_eq!(item.fields["city"], ["Sampleton"]);
        assert_eq!(item.fields["n"], ["7"]);
        assert!(!item.fields.contains_key("empty"));
        assert_eq!(item.collected_at, "2025-01-01T00:00:00+00:00");
        assert!(item.source_id.starts_with("content:") && item.source_id.len() == 32);
    }

    #[test]
    fn rejects_bad_input_with_python_messages() {
        let cases = [
            (json!({"source": "s"}), "source and name are required"),
            (
                json!({"source": "s", "name": "n", "kind": null}),
                "kind must be record, lead, or import",
            ),
            (
                json!({"source": "s", "name": "n", "subject_type": "place"}),
                "subject_type must be person, organization, or domain",
            ),
            (
                json!({"source": "s", "name": "n", "source_url": "ftp://x"}),
                "source_url must be an HTTP(S) URL without credentials",
            ),
            (
                json!({"source": "s", "name": "n", "source_url": "https://u:p@x"}),
                "source_url must be an HTTP(S) URL without credentials",
            ),
            (
                json!({"source": "s", "name": "n", "fields": []}),
                "fields must be an object with at most 64 keys",
            ),
            (
                json!({"source": "s", "name": "n", "fields": {"": "x"}}),
                "invalid field name",
            ),
            (
                json!({"source": "s", "name": "n\u{1}"}),
                "field contains control or direction characters",
            ),
            (
                json!({"source": "s", "name": "a\u{202e}b"}),
                "field contains control or direction characters",
            ),
            (
                json!({"source": true, "name": "n"}),
                "field values must be strings or numbers",
            ),
            (
                json!({"source": "s", "name": "n", "collected_at": "2025-01-01"}),
                "collected_at must be an ISO 8601 timestamp with timezone",
            ),
            (
                json!({"source": "s", "name": "n", "id": 0}),
                "id must be a positive integer",
            ),
            (
                json!({"source": "s", "name": "n", "id": 1.0}),
                "id must be a positive integer",
            ),
            (
                json!(["not", "an", "object"]),
                "observation must be an object",
            ),
        ];
        for (input, message) in cases {
            assert_eq!(
                Observation::from_mapping(&input).unwrap_err().0,
                message,
                "{input}"
            );
        }
        let long = "x".repeat(251);
        assert_eq!(
            Observation::from_mapping(&json!({"source": "s", "name": long}))
                .unwrap_err()
                .0,
            "field exceeds 250 characters"
        );
    }

    #[test]
    fn content_id_matches_python_fingerprint() {
        // Python 4.1.1: Observation.from_mapping({"source": "local", "name": "Avery Example",
        // "fields": {"city": "Sampleton"}}).source_id
        let mut fields = BTreeMap::new();
        fields.insert("city".to_owned(), vec!["Sampleton".to_owned()]);
        assert_eq!(
            content_id("local", "Avery Example", "", &fields),
            "content:60ac9856781f26af93e02694"
        );
        let mut fields = BTreeMap::new();
        fields.insert("city".to_owned(), vec!["Sampleton".to_owned()]);
        fields.insert("n".to_owned(), vec!["7".to_owned()]);
        assert_eq!(
            content_id("local register", "Avery Example", "", &fields),
            "content:a86ebd9a7e8dcfe50721c295"
        );
    }
}
