// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! CSV reading with the rules of Python's `csv.reader` (the default "excel" dialect, not
//! strict) and `csv.DictReader`, which 4.1.1 used for imports.
//!
//! This is a port of the state machine in CPython's `Modules/_csv.c`, so quoted fields,
//! doubled quotes, line breaks inside quotes, stray quotes and short or long rows all come
//! out as they did before.

/// Python's default `csv.field_size_limit()`.
pub const FIELD_LIMIT: usize = 131_072;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

struct Parser {
    state: State,
    fields: Vec<String>,
    field: String,
    field_len: usize,
}

impl Parser {
    fn save_field(&mut self) {
        self.fields.push(std::mem::take(&mut self.field));
        self.field_len = 0;
    }

    fn add_char(&mut self, c: char) -> Result<(), String> {
        if self.field_len >= FIELD_LIMIT {
            return Err(format!("field larger than field limit ({FIELD_LIMIT})"));
        }
        self.field.push(c);
        self.field_len += 1;
        Ok(())
    }

    /// One character of a line, or `None` for the end of the line.
    fn process(&mut self, c: Option<char>) -> Result<(), String> {
        let line_break = matches!(c, Some('\n' | '\r'));
        match self.state {
            State::StartRecord => {
                match c {
                    None => return Ok(()),
                    Some(_) if line_break => {
                        self.state = State::EatCrnl;
                        return Ok(());
                    }
                    Some(_) => {}
                }
                self.state = State::StartField;
                self.start_field(c)
            }
            State::StartField => self.start_field(c),
            State::InField => match c {
                None => {
                    self.save_field();
                    self.state = State::StartRecord;
                    Ok(())
                }
                Some(_) if line_break => {
                    self.save_field();
                    self.state = State::EatCrnl;
                    Ok(())
                }
                Some(',') => {
                    self.save_field();
                    self.state = State::StartField;
                    Ok(())
                }
                Some(other) => self.add_char(other),
            },
            State::InQuotedField => match c {
                None => Ok(()),
                Some('"') => {
                    self.state = State::QuoteInQuotedField;
                    Ok(())
                }
                Some(other) => self.add_char(other),
            },
            State::QuoteInQuotedField => match c {
                Some('"') => {
                    self.state = State::InQuotedField;
                    self.add_char('"')
                }
                Some(',') => {
                    self.save_field();
                    self.state = State::StartField;
                    Ok(())
                }
                None => {
                    self.save_field();
                    self.state = State::StartRecord;
                    Ok(())
                }
                Some(_) if line_break => {
                    self.save_field();
                    self.state = State::EatCrnl;
                    Ok(())
                }
                Some(other) => {
                    // Not strict: a character after a closing quote joins the field.
                    self.state = State::InField;
                    self.add_char(other)
                }
            },
            State::EatCrnl => match c {
                Some(_) if line_break => Ok(()),
                None => {
                    self.state = State::StartRecord;
                    Ok(())
                }
                Some(_) => Err(
                    "new-line character seen in unquoted field - do you need to open \
                     the file with newline=''?"
                        .to_owned(),
                ),
            },
        }
    }

    fn start_field(&mut self, c: Option<char>) -> Result<(), String> {
        match c {
            None => {
                self.save_field();
                self.state = State::StartRecord;
                Ok(())
            }
            Some('\n' | '\r') => {
                self.save_field();
                self.state = State::EatCrnl;
                Ok(())
            }
            Some('"') => {
                self.state = State::InQuotedField;
                Ok(())
            }
            Some(',') => {
                self.save_field();
                Ok(())
            }
            Some(other) => {
                self.state = State::InField;
                self.add_char(other)
            }
        }
    }
}

/// Lines as a file opened with `newline=""` yields them: each ends after `\n`, `\r\n`,
/// or a lone `\r`, and keeps its line break.
fn lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&text[start..=i]);
                start = i + 1;
            }
            b'\r' => {
                let end = if bytes.get(i + 1) == Some(&b'\n') {
                    i + 1
                } else {
                    i
                };
                out.push(&text[start..=end]);
                start = end + 1;
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

/// Every record `csv.reader` yields, blank lines included as empty records.
pub fn records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut parser = Parser {
        state: State::StartRecord,
        fields: Vec::new(),
        field: String::new(),
        field_len: 0,
    };
    let mut out = Vec::new();
    for line in lines(text) {
        for c in line.chars() {
            parser.process(Some(c))?;
        }
        parser.process(None)?;
        if parser.state == State::StartRecord {
            out.push(std::mem::take(&mut parser.fields));
        }
    }
    if parser.field_len != 0 || parser.state == State::InQuotedField {
        parser.save_field();
        out.push(std::mem::take(&mut parser.fields));
    }
    Ok(out)
}

/// A cell of a `csv.DictReader` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// A value from the file.
    Text(String),
    /// A column the row was too short to reach (`restval`, which is `None`).
    Missing,
}

/// One `csv.DictReader` row: columns in header order (a repeated header keeps its first
/// position and its last value), plus any cells beyond the header (`restkey`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DictRow {
    pub cells: Vec<(String, Cell)>,
    pub extra: Option<Vec<String>>,
}

impl DictRow {
    /// Python's `row.get(key)` for a named column, as text.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.cells
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, cell)| match cell {
                Cell::Text(text) => Some(text.as_str()),
                Cell::Missing => None,
            })
    }
}

/// `csv.DictReader` over the text: the header (or `None` for an empty file) and the rows,
/// with blank lines skipped.
pub fn dict_rows(text: &str) -> Result<(Option<Vec<String>>, Vec<DictRow>), String> {
    let mut all = records(text)?.into_iter();
    let Some(header) = all.next() else {
        return Ok((None, Vec::new()));
    };
    let mut rows = Vec::new();
    for record in all {
        if record.is_empty() {
            continue;
        }
        let mut row = DictRow::default();
        for (name, value) in header.iter().zip(record.iter()) {
            set(&mut row.cells, name, Cell::Text(value.clone()));
        }
        if record.len() > header.len() {
            row.extra = Some(record[header.len()..].to_vec());
        } else {
            for name in &header[record.len()..] {
                set(&mut row.cells, name, Cell::Missing);
            }
        }
        rows.push(row);
    }
    Ok((Some(header), rows))
}

fn set(cells: &mut Vec<(String, Cell)>, name: &str, cell: Cell) {
    match cells.iter_mut().find(|(existing, _)| existing == name) {
        Some(slot) => slot.1 = cell,
        None => cells.push((name.to_owned(), cell)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_like_python_csv() {
        let text = "a,\"b \"\"q\"\"\",c\r\n\r\n\"multi\nline\",x\"y\"z,\"q\"w\nlast";
        assert_eq!(
            records(text).unwrap(),
            vec![
                vec!["a".to_owned(), "b \"q\"".to_owned(), "c".to_owned()],
                vec![],
                vec![
                    "multi\nline".to_owned(),
                    "x\"y\"z".to_owned(),
                    "qw".to_owned()
                ],
                vec!["last".to_owned()],
            ]
        );
        assert_eq!(
            records("x,\"open").unwrap(),
            vec![vec!["x".to_owned(), "open".to_owned()]]
        );
        assert_eq!(records("a\rb").unwrap().len(), 2);
        assert_eq!(
            records(",").unwrap(),
            vec![vec![String::new(), String::new()]]
        );
        assert!(records(&"x".repeat(FIELD_LIMIT + 1)).is_err());
    }

    #[test]
    fn dict_rows_follow_dictreader() {
        let (header, rows) = dict_rows("source,name,source\n\nA,B,C,D\nE\n").unwrap();
        assert_eq!(header.unwrap(), ["source", "name", "source"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("source"), Some("C"));
        assert_eq!(rows[0].extra, Some(vec!["D".to_owned()]));
        assert_eq!(rows[1].get("name"), None);
        assert_eq!(rows[1].cells[1].1, Cell::Missing);
        assert_eq!(dict_rows("").unwrap().0, None);
    }
}
