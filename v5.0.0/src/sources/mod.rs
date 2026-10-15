// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Small, bounded connectors for published programmatic interfaces.
//!
//! Each connector asks for one bounded page, keeps only hits that pass its checks, and
//! returns them as observations with their source ID and source URL. A shared name is never
//! treated as proof of identity, and search results that are only leads are marked so.

pub mod domain;
pub mod organization;
pub mod person;

use serde_json::{Map, Value};

use crate::http::{Http, SourceError};
use crate::models::{normalized_name, Observation};
use crate::pyfmt::strip;
use crate::pyjson;

/// Reads credentials. An empty variable counts as unset, as `os.getenv` checks did.
pub trait Env {
    fn var(&self, name: &str) -> Option<String>;
}

/// The process environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }
}

/// What a connector works with: an HTTP client and the environment.
pub struct Context<'a> {
    pub http: &'a mut dyn Http,
    pub env: &'a dyn Env,
}

/// A person search: first and last name, and an optional location (used by NPI only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersonQuery {
    pub first: String,
    pub last: String,
    pub location: String,
}

pub type PersonSource = fn(&PersonQuery, &mut Context<'_>) -> Result<Vec<Observation>, SourceError>;
pub type NameSource = fn(&str, &mut Context<'_>) -> Result<Vec<Observation>, SourceError>;

/// The connectors one command can use, and the ones it uses by default.
#[derive(Clone)]
pub struct Catalog<F> {
    pub sources: Vec<(&'static str, F)>,
    pub defaults: Vec<&'static str>,
}

impl<F: Copy> Catalog<F> {
    pub fn get(&self, name: &str) -> Option<F> {
        self.sources
            .iter()
            .find(|(source, _)| *source == name)
            .map(|(_, connector)| *connector)
    }

    /// Source names in alphabetical order.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.sources.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names
    }
}

/// The `search` connectors; `npi` and `wikidata` run by default.
pub fn person_sources() -> Catalog<PersonSource> {
    Catalog {
        sources: vec![
            ("npi", person::npi as PersonSource),
            ("wikidata", person::wikidata),
            ("google_cse", person::google_cse),
            ("companies_house_officers", person::companies_house_officers),
            ("openalex", person::openalex),
            ("orcid", person::orcid),
            ("crossref", person::crossref),
            ("bluesky", person::bluesky),
            ("github", person::github),
            ("courtlistener", person::courtlistener),
            ("openlibrary", person::openlibrary),
        ],
        defaults: vec!["npi", "wikidata"],
    }
}

/// The `search-org` connectors; `gleif` runs by default.
pub fn organization_sources() -> Catalog<NameSource> {
    Catalog {
        sources: vec![
            ("gleif", organization::gleif as NameSource),
            (
                "companies_house_companies",
                organization::companies_house_companies,
            ),
            ("opencorporates", organization::opencorporates),
        ],
        defaults: vec!["gleif"],
    }
}

/// The `domain` connectors; `rdap` runs by default.
pub fn domain_sources() -> Catalog<NameSource> {
    Catalog {
        sources: vec![
            ("rdap", domain::rdap as NameSource),
            ("wayback", domain::wayback),
            ("urlscan", domain::urlscan),
        ],
        defaults: vec!["rdap"],
    }
}

/// A local first-and-last-name filter: the hit's name must start with the given names and
/// end with the family names, after case folding. Middle names are allowed.
pub fn exact_name(first: &str, last: &str, actual: &str) -> bool {
    let tokens_text = normalized_name(actual);
    let given_text = normalized_name(first);
    let family_text = normalized_name(last);
    let tokens: Vec<&str> = tokens_text.split(' ').filter(|t| !t.is_empty()).collect();
    let given: Vec<&str> = given_text.split(' ').filter(|t| !t.is_empty()).collect();
    let family: Vec<&str> = family_text.split(' ').filter(|t| !t.is_empty()).collect();
    if tokens.len() < given.len() + family.len() {
        return false;
    }
    // Python's tokens[-0:] is the whole list, so an empty family matches only no tokens.
    let tail = if family.is_empty() {
        &tokens[..]
    } else {
        &tokens[tokens.len() - family.len()..]
    };
    tokens[..given.len()] == given[..] && tail == &family[..]
}

/// The name stored on a lead, which never claims to be the person.
pub fn name_for_lead(first: &str, last: &str) -> String {
    format!("Search lead for {first} {last}")
}

/// `isinstance(value, dict)`.
pub(crate) fn as_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// Python's `(value or {})` followed by an `isinstance(..., dict)` check: a missing or
/// falsy value is an empty object, a truthy non-object is `None`.
pub(crate) fn object_or_empty(value: Option<&Value>) -> Option<&Map<String, Value>> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match value {
        Some(Value::Object(map)) => Some(map),
        Some(other) if pyjson::truthy(other) => None,
        _ => Some(EMPTY.get_or_init(Map::new)),
    }
}

/// A string member, if it is one.
pub(crate) fn str_of<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

/// 4.1.1's `_optional`: store a string or integer (not a boolean or float) when its text
/// is not blank.
pub(crate) fn optional(fields: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    let text = match value {
        Some(Value::String(text)) => text.clone(),
        Some(other @ Value::Number(_)) => match pyjson::integer_text(other) {
            Some(digits) => digits,
            None => return,
        },
        _ => return,
    };
    if !strip(&text).is_empty() {
        fields.insert(key.to_owned(), Value::Array(vec![Value::String(text)]));
    }
}

/// Python's `list(dict.fromkeys(items))`.
pub(crate) fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

pub(crate) fn strings(items: Vec<String>) -> Value {
    Value::Array(items.into_iter().map(Value::String).collect())
}

/// A JSON list member (`isinstance(..., list)`), missing treated as `default_empty`.
pub(crate) fn list_member<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    missing_is_empty: bool,
) -> Option<&'a [Value]> {
    match map.get(key) {
        Some(Value::Array(items)) => Some(items),
        None if missing_is_empty => Some(&[]),
        _ => None,
    }
}

/// Matchers for the fixed-shape identifiers the connectors accept.
pub(crate) mod shapes {
    fn all(text: &str, ok: impl Fn(u8) -> bool) -> bool {
        text.bytes().all(ok)
    }

    pub fn digits(text: &str) -> bool {
        !text.is_empty() && all(text, |b| b.is_ascii_digit())
    }

    /// `/officers/[A-Za-z0-9_-]+/appointments`
    pub fn officer_path(path: &str) -> bool {
        path.strip_prefix("/officers/")
            .and_then(|rest| rest.strip_suffix("/appointments"))
            .is_some_and(|id| {
                !id.is_empty() && all(id, |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
    }

    /// `A\d+`
    pub fn openalex_author(id: &str) -> bool {
        id.strip_prefix('A').is_some_and(digits)
    }

    /// `https://orcid\.org/[0-9X-]{19}`
    pub fn orcid_url(url: &str) -> bool {
        url.strip_prefix("https://orcid.org/").is_some_and(|rest| {
            rest.len() == 19 && all(rest, |b| b.is_ascii_digit() || b == b'X' || b == b'-')
        })
    }

    /// `\d{4}-\d{4}-\d{4}-[\dX]{4}`
    pub fn orcid_id(id: &str) -> bool {
        let parts: Vec<&str> = id.split('-').collect();
        parts.len() == 4
            && parts[..3].iter().all(|p| p.len() == 4 && digits(p))
            && parts[3].len() == 4
            && all(parts[3], |b| b.is_ascii_digit() || b == b'X')
    }

    /// `did:(plc:[a-z2-7]+|web:[A-Za-z0-9.:%_-]+)`
    pub fn did(did: &str) -> bool {
        if let Some(rest) = did.strip_prefix("did:plc:") {
            return !rest.is_empty()
                && all(rest, |b| {
                    b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)
                });
        }
        if let Some(rest) = did.strip_prefix("did:web:") {
            return !rest.is_empty()
                && all(rest, |b| b.is_ascii_alphanumeric() || b".:%_-".contains(&b));
        }
        false
    }

    /// `[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?`
    pub fn github_login(login: &str) -> bool {
        let bytes = login.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 39
            && bytes[0].is_ascii_alphanumeric()
            && bytes[bytes.len() - 1].is_ascii_alphanumeric()
            && all(login, |b| b.is_ascii_alphanumeric() || b == b'-')
    }

    /// `/opinion/\d+/[A-Za-z0-9_/-]+/?`
    pub fn opinion_path(path: &str) -> bool {
        let Some(rest) = path.strip_prefix("/opinion/") else {
            return false;
        };
        let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
        digit_count > 0
            && rest[digit_count..].strip_prefix('/').is_some_and(|tail| {
                !tail.is_empty() && all(tail, |b| b.is_ascii_alphanumeric() || b"_/-".contains(&b))
            })
    }

    /// `OL\d+A`
    pub fn openlibrary_author(id: &str) -> bool {
        id.strip_prefix("OL")
            .and_then(|rest| rest.strip_suffix('A'))
            .is_some_and(digits)
    }

    /// `[A-Z0-9]{min,max}`
    pub fn upper_alnum(text: &str, min: usize, max: usize) -> bool {
        (min..=max).contains(&text.len())
            && all(text, |b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }

    /// `[a-z0-9_]{2,12}`
    pub fn jurisdiction(code: &str) -> bool {
        (2..=12).contains(&code.len())
            && all(code, |b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'
            })
    }

    /// `[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?`
    pub fn ldh_label(label: &str) -> bool {
        let bytes = label.as_bytes();
        let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        !bytes.is_empty()
            && bytes.len() <= 63
            && edge(bytes[0])
            && edge(bytes[bytes.len() - 1])
            && all(label, |b| edge(b) || b == b'-')
    }

    /// `\d{14}`
    pub fn wayback_stamp(stamp: &str) -> bool {
        stamp.len() == 14 && digits(stamp)
    }

    /// `[0-9a-fA-F-]{36}`
    pub fn scan_id(id: &str) -> bool {
        id.len() == 36 && all(id, |b| b.is_ascii_hexdigit() || b == b'-')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_name_allows_middle_names_only() {
        assert!(exact_name("Avery", "Example", "AVERY  Q   example"));
        assert!(exact_name(
            "Mary Ann",
            "Smith Jones",
            "mary ann b smith jones"
        ));
        assert!(!exact_name("Avery", "Example", "Avery Examples"));
        assert!(!exact_name("Avery", "Example", "Example Avery"));
        assert!(!exact_name("Avery", "Example", "Avery"));
        assert!(exact_name("Jürgen", "Strauß", "JÜRGEN STRAUSS"));
        assert!(exact_name("", "", ""));
        assert!(!exact_name("a", "", "a b"));
    }

    #[test]
    fn identifier_shapes() {
        use shapes::*;
        assert!(officer_path("/officers/abc_123/appointments"));
        assert!(!officer_path("/officers/no/x/appointments"));
        assert!(openalex_author("A123") && !openalex_author("A") && !openalex_author("B1"));
        assert!(orcid_id("0000-0001-2345-678X") && !orcid_id("0000-0001-2345-67X"));
        assert!(orcid_url("https://orcid.org/0000-0001-2345-678X"));
        assert!(
            did("did:plc:abcdef234567") && !did("did:plc:invalid?") && did("did:web:example.com")
        );
        assert!(
            github_login("avery-example") && !github_login("bad/username") && !github_login("-x")
        );
        assert!(!github_login(&"a".repeat(40)) && github_login(&"a".repeat(39)));
        assert!(opinion_path("/opinion/234/avery-v-example/") && opinion_path("/opinion/2/a//"));
        assert!(!opinion_path("/opinion/234a/x") && !opinion_path("/opinion/234/"));
        assert!(openlibrary_author("OL321A") && !openlibrary_author("OLA"));
        assert!(upper_alnum("01234567", 2, 12) && !upper_alnum("0123456a", 2, 12));
        assert!(jurisdiction("us_ca") && !jurisdiction("US"));
        assert!(ldh_label("xn--bcher-kva") && !ldh_label("-a") && !ldh_label(&"a".repeat(64)));
        assert!(wayback_stamp("20250101000000") && !wayback_stamp("2025010100000"));
        assert!(scan_id("12345678-1234-1234-1234-123456789abc"));
    }
}
