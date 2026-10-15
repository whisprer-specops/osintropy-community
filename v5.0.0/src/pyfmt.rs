// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Text and number rules that OSINTropy 4.x took from Python, reproduced exactly.
//!
//! Names are compared, cleaned and printed the way 4.1.1 did it, so a case file written by
//! either version reads the same in both. Each function names the Python behaviour it copies.

use crate::casefold_data::EXCEPTIONS;

/// Python's `str.isspace()`: Unicode `White_Space`, plus the four ASCII information
/// separators (U+001C to U+001F), which Python also treats as whitespace.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `" ".join(text.split())`: runs of whitespace become one space, ends trimmed.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split(is_space).filter(|word| !word.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Python's `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// Python's `str.lstrip()`.
pub fn lstrip(text: &str) -> &str {
    text.trim_start_matches(is_space)
}

/// Python's `str.casefold()`: full Unicode case folding, so "Strauß" and "STRAUSS" agree.
pub fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match EXCEPTIONS.binary_search_by(|(key, _)| key.cmp(&c)) {
            Ok(index) => out.push_str(EXCEPTIONS[index].1),
            Err(_) => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// Python's `" ".join(name.casefold().split())`: the form names are compared in.
pub fn normalized_name(name: &str) -> String {
    collapse_whitespace(&casefold(name))
}

/// Python's `repr(float)`, which is also what `str()` and `json.dumps` print.
///
/// The shortest digits that read back as the same number, in fixed notation unless the
/// decimal point sits more than 16 places right or 4 places left of the first digit.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    let Some((digits, exponent)) = shortest_digits(value.abs()) else {
        return format!("{value:e}");
    };
    // The value is 0.DIGITS x 10^point.
    let point = exponent + 1;
    let mut out = String::with_capacity(digits.len() + 8);
    if value < 0.0 {
        out.push('-');
    }
    if point <= -4 || point > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let shown = point - 1;
        out.push('e');
        out.push(if shown < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", shown.unsigned_abs()));
    } else if point <= 0 {
        out.push_str("0.");
        for _ in 0..point.unsigned_abs() {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let whole = point.unsigned_abs() as usize;
        if digits.len() <= whole {
            out.push_str(&digits);
            for _ in digits.len()..whole {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..whole]);
            out.push('.');
            out.push_str(&digits[whole..]);
        }
    }
    out
}

/// The shortest digits that read back as `value` (positive and finite), and the decimal
/// exponent of the first digit, chosen as Python's repr chooses them.
fn shortest_digits(value: f64) -> Option<(String, i32)> {
    let (digits, exponent) = split_scientific(&format!("{value:e}"))?;
    // When the exact value lies exactly halfway between two shortest candidates, Python
    // keeps the even last digit and Rust's `{:e}` rounds up. Such ties need about 16
    // digits, where neighbouring doubles are coarser than the last decimal place.
    let last = digits.as_bytes().last().copied()?;
    if digits.len() < 15 || last % 2 == 0 {
        return Some((digits, exponent));
    }
    let mut lower = digits.clone();
    lower.pop();
    lower.push(char::from(last - 1));
    let midpoint = format!("{lower}5");
    // 800 places is enough to write any double exactly.
    let (exact, exact_exponent) = split_scientific(&format!("{value:.800e}"))?;
    let is_tie = exact_exponent == exponent
        && exact.starts_with(&midpoint)
        && exact[midpoint.len()..].bytes().all(|b| b == b'0');
    if is_tie {
        let trimmed = lower.trim_end_matches('0');
        let lower = if trimmed.is_empty() { "0" } else { trimmed };
        return Some((lower.to_owned(), exponent));
    }
    Some((digits, exponent))
}

/// `"1.2345e-7"` to `("12345", -7)`.
fn split_scientific(text: &str) -> Option<(String, i32)> {
    let (mantissa, exponent) = text.split_once('e')?;
    let exponent = exponent.parse::<i32>().ok()?;
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some((digits, exponent))
}

/// Python's `round(value, digits)` for a float: correctly rounded, ties to even.
pub fn round_to(value: f64, digits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    // Rust's fixed-precision formatting rounds the exact binary value, ties to even,
    // exactly as Python's round() does.
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// Python's built-in `sum()` over floats (CPython 3.12 and later): compensated
/// (Neumaier) summation, so the result matches 4.1.1 to the last bit.
pub fn float_sum(values: &[f64]) -> f64 {
    let mut iter = values.iter().copied();
    let Some(first) = iter.next() else {
        return 0.0;
    };
    // sum() starts from the integer 0, and 0 + x is a float equal to x (with -0.0 becoming 0.0).
    let mut total = 0.0 + first;
    let mut compensation = 0.0_f64;
    for x in iter {
        let t = total + x;
        if total.abs() >= x.abs() {
            compensation += (total - t) + x;
        } else {
            compensation += (x - t) + total;
        }
        total = t;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

/// Python's `int(text)` for the ASCII forms people type: surrounding whitespace, an optional
/// sign, and digits that may be grouped with single underscores (`1_000`).
pub fn parse_int(text: &str) -> Option<i64> {
    let text = strip(text);
    let (negative, body) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut value: i64 = 0;
    for byte in body.bytes() {
        if byte == b'_' {
            continue;
        }
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?;
        let digit = i64::from(byte - b'0');
        value = if negative {
            value.checked_sub(digit)?
        } else {
            value.checked_add(digit)?
        };
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_matches_python_including_information_separators() {
        assert_eq!(
            collapse_whitespace("  a\t\u{1c}b \u{a0} c\u{3000}"),
            "a b c"
        );
        assert_eq!(collapse_whitespace("\u{200b}"), "\u{200b}");
        assert_eq!(strip("\u{1f} x \u{85}"), "x");
        assert_eq!(lstrip("\u{1d} =x "), "=x ");
    }

    #[test]
    fn casefold_uses_full_folding() {
        assert_eq!(casefold("Strauß"), "strauss");
        assert_eq!(casefold("ẞ"), "ss");
        assert_eq!(casefold("ΣΑΣ ς"), "σασ σ");
        assert_eq!(casefold("ﬁne"), "fine");
        assert_eq!(casefold("İ"), "i\u{307}");
        assert_eq!(casefold("Ꭰꭰ"), "ᎠᎠ");
        assert_eq!(normalized_name("  AVERY   example "), "avery example");
    }

    #[test]
    fn exception_table_is_sorted() {
        assert!(EXCEPTIONS.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn float_repr_matches_python() {
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (2.5, "2.5"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (123456789.123, "123456789.123"),
            (1.2345678901234568e17, "1.2345678901234568e+17"),
            (-3.25, "-3.25"),
            (1e300, "1e+300"),
            (f64::INFINITY, "inf"),
            // Exact ties between two shortest forms keep the even digit, as Python does.
            (948_841_720_842_342.0 + 0.25, "948841720842342.2"),
            (72_653_724_452_805.0 + 0.125, "72653724452805.12"),
            (-770_301_928_153_519.0 - 0.25, "-770301928153519.2"),
        ];
        for (value, expected) in cases {
            assert_eq!(float_repr(value), expected, "{value:e}");
        }
    }

    #[test]
    fn round_and_sum_follow_python() {
        assert_eq!(round_to(0.125, 2), 0.12);
        assert_eq!(round_to(2.675, 2), 2.67);
        assert_eq!(round_to(1.00005, 4), 1.0001);
        assert_eq!(float_sum(&[]), 0.0);
        assert_eq!(float_sum(&[0.1; 10]), 1.0);
        assert!(float_sum(&[-0.0]).is_sign_positive());
    }

    #[test]
    fn parse_int_accepts_python_forms() {
        assert_eq!(parse_int(" 42 "), Some(42));
        assert_eq!(parse_int("+7"), Some(7));
        assert_eq!(parse_int("-3"), Some(-3));
        assert_eq!(parse_int("1_000"), Some(1000));
        for bad in [
            "",
            "-",
            "1__0",
            "_1",
            "1_",
            "0x5",
            "1.0",
            "9223372036854775808",
        ] {
            assert_eq!(parse_int(bad), None, "{bad}");
        }
        assert_eq!(parse_int("-9223372036854775808"), Some(i64::MIN));
    }
}
