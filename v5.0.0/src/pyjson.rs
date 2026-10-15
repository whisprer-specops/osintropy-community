// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! JSON written byte for byte as Python's `json.dumps` writes it, and Python's view of
//! parsed JSON values (truthiness, `str()`, int versus float).
//!
//! Reports, the database's JSON columns and content fingerprints all depend on the exact
//! text, so OSINTropy writes its own JSON instead of trusting a library's layout.

use serde_json::{Number, Value};

use crate::pyfmt::float_repr;

/// A JSON value with objects kept in insertion order, as Python dicts are.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// The `json.dumps` options OSINTropy uses.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    indent: Option<usize>,
    sort_keys: bool,
    ensure_ascii: bool,
}

impl Style {
    /// `json.dumps(doc, indent=2, ensure_ascii=False, sort_keys=True)`: reports and `show`.
    pub const REPORT: Style = Style {
        indent: Some(2),
        sort_keys: true,
        ensure_ascii: false,
    };
    /// `json.dumps(obj, indent=2, ensure_ascii=False)`: `inspect`, keys in insertion order.
    pub const INSPECT: Style = Style {
        indent: Some(2),
        sort_keys: false,
        ensure_ascii: false,
    };
    /// `json.dumps(obj, sort_keys=True, ensure_ascii=False)`: stored fields and fingerprints.
    pub const COMPACT: Style = Style {
        indent: None,
        sort_keys: true,
        ensure_ascii: false,
    };
    /// `json.dumps(obj, sort_keys=True)`: the stored source errors.
    pub const COMPACT_ASCII: Style = Style {
        indent: None,
        sort_keys: true,
        ensure_ascii: true,
    };
}

impl Json {
    /// Text identical to `json.dumps(self, ...)` with the given options.
    pub fn dumps(&self, style: Style) -> String {
        let mut out = String::new();
        write_value(&mut out, self, style, 0);
        out
    }

    /// An object from key/value pairs, kept in the order given.
    pub fn object<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// A string value.
    pub fn str(text: impl Into<String>) -> Json {
        Json::Str(text.into())
    }

    /// An array of strings.
    pub fn strings<S: AsRef<str>>(items: impl IntoIterator<Item = S>) -> Json {
        Json::Array(
            items
                .into_iter()
                .map(|s| Json::Str(s.as_ref().to_owned()))
                .collect(),
        )
    }
}

fn newline(out: &mut String, style: Style, level: usize) {
    if let Some(indent) = style.indent {
        out.push('\n');
        for _ in 0..indent * level {
            out.push(' ');
        }
    }
}

fn write_value(out: &mut String, value: &Json, style: Style, level: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(number) => out.push_str(&number.to_string()),
        Json::Float(number) => out.push_str(&float_text(*number)),
        Json::Str(text) => write_string(out, text, style.ensure_ascii),
        Json::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(item_separator(style));
                }
                newline(out, style, level + 1);
                write_value(out, item, style, level + 1);
            }
            newline(out, style, level);
            out.push(']');
        }
        Json::Object(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut ordered: Vec<&(String, Json)> = entries.iter().collect();
            if style.sort_keys {
                ordered.sort_by(|a, b| a.0.cmp(&b.0));
            }
            out.push('{');
            for (index, (key, item)) in ordered.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(item_separator(style));
                }
                newline(out, style, level + 1);
                write_string(out, key, style.ensure_ascii);
                out.push_str(": ");
                write_value(out, item, style, level + 1);
            }
            newline(out, style, level);
            out.push('}');
        }
    }
}

fn item_separator(style: Style) -> &'static str {
    if style.indent.is_some() {
        ","
    } else {
        ", "
    }
}

/// How `json.dumps` prints a float.
fn float_text(number: f64) -> String {
    if number.is_nan() {
        "NaN".to_owned()
    } else if number.is_infinite() {
        if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_owned()
    } else {
        float_repr(number)
    }
}

/// Python's string escaping: `"`, `\`, the named controls, other controls as `\u00XX`,
/// and, when `ensure_ascii` is set, everything outside printable ASCII as `\uXXXX`.
fn write_string(out: &mut String, text: &str, ensure_ascii: bool) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c if ensure_ascii && !(' '..='~').contains(&c) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's truthiness of a parsed JSON value.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => {
            if is_integer(number) {
                int_text(number) != "0"
            } else {
                number_f64(number) != 0.0
            }
        }
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// True when Python's json module would have parsed this number as an `int`.
pub fn is_integer(number: &Number) -> bool {
    !number.to_string().contains(['.', 'e', 'E'])
}

/// The number as a float, as Python's `float(...)` would read its text.
pub fn number_f64(number: &Number) -> f64 {
    number.to_string().parse().unwrap_or(f64::NAN)
}

fn int_text(number: &Number) -> String {
    let text = number.to_string();
    // JSON allows "-0", which Python reads as the integer 0.
    if text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

/// Python's `str()` of a parsed JSON number: integers keep every digit, floats use repr.
pub fn number_text(number: &Number) -> String {
    if is_integer(number) {
        int_text(number)
    } else {
        float_repr(number_f64(number))
    }
}

/// Python's `str()` of a parsed JSON scalar (`None`, `True`, numbers, strings). Lists and
/// objects give `None` here: OSINTropy 5 skips them where 4.1.1 stored Python's printout.
pub fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("None".to_owned()),
        Value::Bool(true) => Some("True".to_owned()),
        Value::Bool(false) => Some("False".to_owned()),
        Value::Number(number) => Some(number_text(number)),
        Value::String(text) => Some(text.clone()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// The digits of a JSON integer (Python `isinstance(value, int)`, booleans excluded).
pub fn integer_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) if is_integer(number) => Some(int_text(number)),
        _ => None,
    }
}

/// Python's `value == 0` for a parsed JSON value (`False` counts, as in Python).
pub fn equals_zero(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => !flag,
        Value::Number(number) => number_f64(number) == 0.0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_json_dumps() {
        let doc = Json::object([
            ("b", Json::Array(vec![Json::Int(1), Json::Float(2.5)])),
            ("a", Json::object(Vec::<(String, Json)>::new())),
            ("c", Json::Array(vec![])),
        ]);
        assert_eq!(
            doc.dumps(Style::REPORT),
            "{\n  \"a\": {},\n  \"b\": [\n    1,\n    2.5\n  ],\n  \"c\": []\n}"
        );
        assert_eq!(
            doc.dumps(Style::COMPACT),
            "{\"a\": {}, \"b\": [1, 2.5], \"c\": []}"
        );
        assert_eq!(doc.dumps(Style::INSPECT).lines().nth(1), Some("  \"b\": ["));
    }

    #[test]
    fn strings_escape_like_python() {
        let text = Json::str("q\"\\\n\t\u{1}\u{7f}é😀");
        assert_eq!(
            text.dumps(Style::COMPACT),
            "\"q\\\"\\\\\\n\\t\\u0001\u{7f}é😀\""
        );
        assert_eq!(
            text.dumps(Style::COMPACT_ASCII),
            "\"q\\\"\\\\\\n\\t\\u0001\\u007f\\u00e9\\ud83d\\ude00\""
        );
    }

    #[test]
    fn python_view_of_values() {
        let parsed: Value = serde_json::from_str(
            r#"[0, -0, 0.0, 1.50, 1e5, 123456789012345678901234567890, "", [], {}, null, true]"#,
        )
        .unwrap();
        let items = parsed.as_array().unwrap();
        let truth: Vec<bool> = items.iter().map(truthy).collect();
        assert_eq!(
            truth,
            [false, false, false, true, true, true, false, false, false, false, true]
        );
        let text: Vec<Option<String>> = items.iter().map(scalar_text).collect();
        assert_eq!(text[1].as_deref(), Some("0"));
        assert_eq!(text[3].as_deref(), Some("1.5"));
        assert_eq!(text[4].as_deref(), Some("100000.0"));
        assert_eq!(text[5].as_deref(), Some("123456789012345678901234567890"));
        assert_eq!(text[7], None);
        assert_eq!(text[9].as_deref(), Some("None"));
        assert_eq!(text[10].as_deref(), Some("True"));
        assert!(equals_zero(&items[2]) && equals_zero(&Value::Bool(false)));
        assert_eq!(integer_text(&items[3]), None);
    }
}
