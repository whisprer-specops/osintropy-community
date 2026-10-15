// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! ISO 8601 timestamps as Python's `datetime` reads and writes them.
//!
//! `collected_at` values from imports were checked with `datetime.fromisoformat` and stored
//! as `isoformat(timespec="seconds")`. This is a port of CPython 3.14's C parser
//! (`Modules/_datetimemodule.c`), so every string 4.1.1 accepted is accepted here and
//! written back identically, from `2025-01-01T12:00:00Z` to week dates such as `2025W011T12`.

use chrono::{Datelike, NaiveDate, Utc};

/// A parsed timestamp. `offset_us` is the UTC offset in microseconds, if one was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
    pub offset_us: Option<i64>,
}

const DAY_US: i64 = 86_400_000_000;

/// The current UTC time as `YYYY-MM-DDTHH:MM:SS+00:00`, Python's
/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
pub fn utc_now() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// The UTC time `days` days ago, in the same form; `None` if that falls before year 1,
/// where Python's `datetime` ends.
pub fn utc_days_ago(days: i64) -> Option<String> {
    let delta = chrono::TimeDelta::try_days(days)?;
    let then = Utc::now().checked_sub_signed(delta)?;
    if then.year() < 1 {
        return None;
    }
    Some(then.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
}

/// `datetime.fromisoformat(text.replace("Z", "+00:00"))`, requiring a UTC offset, written
/// back as `isoformat(timespec="seconds")`. `None` when Python would have refused it.
pub fn normalize_with_offset(text: &str) -> Option<String> {
    let parsed = parse(&text.replace('Z', "+00:00"))?;
    parsed.offset_us?;
    Some(format_seconds(&parsed))
}

/// Python's `datetime.isoformat(timespec="seconds")`.
pub fn format_seconds(ts: &Timestamp) -> String {
    let mut out = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        ts.year, ts.month, ts.day, ts.hour, ts.minute, ts.second
    );
    if let Some(offset) = ts.offset_us {
        let sign = if offset < 0 { '-' } else { '+' };
        let magnitude = offset.unsigned_abs();
        let micros = magnitude % 1_000_000;
        let total_seconds = magnitude / 1_000_000;
        let seconds = total_seconds % 60;
        let minutes = (total_seconds / 60) % 60;
        let hours = total_seconds / 3600;
        out.push_str(&format!("{sign}{hours:02}:{minutes:02}"));
        if micros != 0 {
            out.push_str(&format!(":{seconds:02}.{micros:06}"));
        } else if seconds != 0 {
            out.push_str(&format!(":{seconds:02}"));
        }
    }
    out
}

/// Python 3.14's `datetime.fromisoformat(text)`.
pub fn parse(text: &str) -> Option<Timestamp> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 7 {
        return None;
    }
    let separator = find_separator(&chars)?;
    let (mut year, mut month, mut day) = parse_date(&chars, separator)?;
    let mut time = Clock::default();
    if chars.len() > separator {
        time = parse_time(&chars[separator + 1..])?;
    }
    if let Some(offset) = time.offset_us {
        // timezone() takes offsets strictly inside one day either way.
        if offset <= -DAY_US || offset >= DAY_US {
            return None;
        }
    }
    let mut hour = time.hour;
    // Python 3.14: 24:00 is midnight at the start of the next day.
    if hour == 24 && (1..=12).contains(&month) {
        let month_days = days_in_month(year, month);
        if day <= month_days {
            if time.minute != 0 || time.second != 0 || time.microsecond != 0 {
                return None;
            }
            hour = 0;
            day += 1;
            if day > month_days {
                day = 1;
                month += 1;
                if month > 12 {
                    month = 1;
                    year += 1;
                }
            }
        }
    }
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || time.minute > 59
        || time.second > 59
    {
        return None;
    }
    Some(Timestamp {
        year,
        month,
        day,
        hour,
        minute: time.minute,
        second: time.second,
        microsecond: time.microsecond,
        offset_us: time.offset_us,
    })
}

#[derive(Default)]
struct Clock {
    hour: u32,
    minute: u32,
    second: u32,
    microsecond: u32,
    offset_us: Option<i64>,
}

/// The C parser reads a NUL byte past the end of the string.
fn at(chars: &[char], index: usize) -> char {
    chars.get(index).copied().unwrap_or('\0')
}

fn digits(chars: &[char], pos: &mut usize, count: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..count {
        let digit = at(chars, *pos).to_digit(10)?;
        *pos += 1;
        value = value * 10 + digit;
    }
    Some(value)
}

/// `_find_isoformat_datetime_separator`: where the date part ends.
fn find_separator(s: &[char]) -> Option<usize> {
    let len = s.len();
    if len == 7 {
        return Some(7);
    }
    if s[4] == '-' {
        if s[5] == 'W' {
            if len > 8 && s[8] == '-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && s[10].is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if s[4] == 'W' {
        let mut idx = 7;
        while idx < len && s[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

/// `parse_isoformat_date`: calendar dates and ISO week dates.
fn parse_date(s: &[char], len: usize) -> Option<(i32, u32, u32)> {
    let mut pos = 0;
    let year = digits(s, &mut pos, 4)?;
    let uses_separator = at(s, pos) == '-';
    if uses_separator {
        pos += 1;
    }
    if at(s, pos) == 'W' {
        pos += 1;
        let week = digits(s, &mut pos, 2)?;
        let weekday = if pos < len {
            if uses_separator {
                if at(s, pos) != '-' {
                    return None;
                }
                pos += 1;
            }
            digits(s, &mut pos, 1)?
        } else {
            1
        };
        return iso_to_ymd(year, week, weekday);
    }
    let month = digits(s, &mut pos, 2)?;
    if uses_separator {
        if at(s, pos) != '-' {
            return None;
        }
        pos += 1;
    }
    let day = digits(s, &mut pos, 2)?;
    Some((i32::try_from(year).ok()?, month, day))
}

/// `parse_isoformat_time`: the clock and an optional UTC offset.
fn parse_time(t: &[char]) -> Option<Clock> {
    if t.is_empty() {
        return None;
    }
    let end = t.len();
    let tz = t
        .iter()
        .position(|c| matches!(c, 'Z' | '+' | '-'))
        .unwrap_or(end);
    let (clock, more) = parse_hh_mm_ss_ff(t, 0, tz)?;
    let mut result = Clock {
        hour: clock[0],
        minute: clock[1],
        second: clock[2],
        microsecond: clock[3],
        offset_us: None,
    };
    if tz == end {
        return if more { None } else { Some(result) };
    }
    if t[tz] == 'Z' {
        if tz + 1 < end {
            return None;
        }
        result.offset_us = Some(0);
        return Some(result);
    }
    let sign: i64 = if t[tz] == '-' { -1 } else { 1 };
    let (offset, more) = parse_hh_mm_ss_ff(t, tz + 1, end)?;
    if more {
        return None;
    }
    let seconds = i64::from(offset[0]) * 3600 + i64::from(offset[1]) * 60 + i64::from(offset[2]);
    result.offset_us = Some(sign * (seconds * 1_000_000 + i64::from(offset[3])));
    Some(result)
}

/// `parse_hh_mm_ss_ff`: `HH[:?MM[:?SS[{.,}ffffff]]]` between `start` and `end`. The flag
/// says whether anything other than the end of the string follows.
fn parse_hh_mm_ss_ff(t: &[char], start: usize, end: usize) -> Option<([u32; 4], bool)> {
    let mut values = [0u32; 4];
    let mut pos = start;
    let mut has_separator = true;
    let mut index = 0;
    while index < 3 {
        values[index] = digits(t, &mut pos, 2)?;
        let c = at(t, pos);
        pos += 1;
        if index == 0 {
            has_separator = c == ':';
        }
        if c == '.' || c == ',' {
            if index < 2 || pos >= end {
                return None;
            }
            break;
        } else if pos >= end {
            return Some((values, c != '\0'));
        } else if has_separator && c == ':' {
            if index == 2 {
                return None;
            }
        } else if !has_separator {
            pos -= 1;
        } else {
            return None;
        }
        index += 1;
    }
    let to_parse = end.saturating_sub(pos).min(6);
    if to_parse == 0 {
        return None;
    }
    let mut fraction = digits(t, &mut pos, to_parse)?;
    const CORRECTION: [u32; 5] = [100_000, 10_000, 1_000, 100, 10];
    if to_parse < 6 {
        fraction *= CORRECTION[to_parse - 1];
    }
    values[3] = fraction;
    while at(t, pos).is_ascii_digit() {
        pos += 1;
    }
    Some((values, at(t, pos) != '\0'))
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `iso_to_ymd`: an ISO year, week and weekday to a calendar date.
fn iso_to_ymd(iso_year: u32, week: u32, weekday: u32) -> Option<(i32, u32, u32)> {
    let year = i32::try_from(iso_year).ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let january_first = NaiveDate::from_ymd_opt(year, 1, 1)?;
    // Monday is 0.
    let first_weekday = january_first.weekday().num_days_from_monday();
    if week == 0 || week >= 53 {
        let long_year = first_weekday == 3 || (first_weekday == 2 && is_leap(year));
        if week != 53 || !long_year {
            return None;
        }
    }
    if !(1..=7).contains(&weekday) {
        return None;
    }
    let first_day = january_first.num_days_from_ce();
    let mut week1_monday = first_day - i32::try_from(first_weekday).ok()?;
    if first_weekday > 3 {
        week1_monday += 7;
    }
    let ordinal = week1_monday + i32::try_from((week - 1) * 7 + weekday - 1).ok()?;
    let date = NaiveDate::from_num_days_from_ce_opt(ordinal)?;
    Some((date.year(), date.month(), date.day()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(text: &str) -> Option<String> {
        normalize_with_offset(text)
    }

    #[test]
    fn accepts_the_forms_python_accepts() {
        let cases = [
            ("2025-01-01T00:00:00Z", "2025-01-01T00:00:00+00:00"),
            ("2025-01-01 12:00+01", "2025-01-01T12:00:00+01:00"),
            (
                "2025-01-01T12:00:00.1234567+01:00",
                "2025-01-01T12:00:00+01:00",
            ),
            ("2025W011T12Z", "2024-12-30T12:00:00+00:00"),
            ("20250101T120000Z", "2025-01-01T12:00:00+00:00"),
            (
                "2025-01-01T12:00:00+00:00:30.5",
                "2025-01-01T12:00:00+00:00:30.500000",
            ),
            ("2025-01-01T12:00:00,5-05:30", "2025-01-01T12:00:00-05:30"),
            ("2025-01-01é12:00:00+00:00", "2025-01-01T12:00:00+00:00"),
            ("2025-01-01T12:00:00+00:99", "2025-01-01T12:00:00+01:39"),
            (
                "2025-01-01T12:00:00-00:00:00.5",
                "2025-01-01T12:00:00-00:00:00.500000",
            ),
            ("2025-12-31T24:00:00+00:00", "2026-01-01T00:00:00+00:00"),
            ("0001-W01-1T00:00+00:00", "0001-01-01T00:00:00+00:00"),
        ];
        for (input, expected) in cases {
            assert_eq!(norm(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn rejects_what_python_rejects() {
        for bad in [
            "2025-01-01T12:00:00",
            "2025-01-01",
            "2025-001T12:00Z",
            "2025-01-01T12:00:00+24:00",
            "2025-01-01T24:00:01+00:00",
            "2025-02-30T00:00Z",
            "2025-01-01T12:30.5Z",
            "2025-01-01T12:00:00:5Z",
            "2025-W53-1T00:00Z",
            "2025-01-01T12:00:00.Z",
            "short",
            "",
        ] {
            assert_eq!(norm(bad), None, "{bad}");
        }
        assert_eq!(
            norm("2026-W53-1T00:00Z").as_deref(),
            Some("2026-12-28T00:00:00+00:00")
        );
    }

    #[test]
    fn days_ago_stays_in_python_range() {
        assert!(utc_days_ago(0).is_some());
        assert!(utc_days_ago(10_000_000).is_none());
        assert_eq!(utc_now().len(), 25);
    }
}
