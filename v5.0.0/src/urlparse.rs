// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! URL splitting and percent-encoding with the rules of Python's `urllib.parse`.
//!
//! OSINTropy 4.x checked source URLs, archived pages and registry endpoints with
//! `urlsplit`, and built query strings with `urlencode`. These functions keep those
//! checks and encodings exactly, including Python's guards against bracket tricks and
//! hosts that change meaning under Unicode compatibility normalisation (NFKC).

use std::net::{Ipv4Addr, Ipv6Addr};

use icu_normalizer::ComposingNormalizerBorrowed;

/// The five parts `urlsplit` returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitUrl {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
    pub fragment: String,
}

/// Python's `urllib.parse.urlsplit(url)`. An `Err` carries Python's `ValueError` message.
pub fn urlsplit(url: &str) -> Result<SplitUrl, String> {
    // Leading C0 controls and spaces are stripped; tabs and line breaks are removed anywhere.
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut scheme = String::new();
    let mut rest = url.as_str();
    if let Some(colon) = url.find(':') {
        let candidate = &url[..colon];
        let starts_with_letter = candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if starts_with_letter
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            scheme = candidate.to_ascii_lowercase();
            rest = &url[colon + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_owned();
        rest = &after[end..];
        let opens = netloc.contains('[');
        let closes = netloc.contains(']');
        if opens != closes {
            return Err("Invalid IPv6 URL".to_owned());
        }
        if opens {
            check_bracketed_netloc(&netloc)?;
        }
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    check_netloc(&netloc)?;
    Ok(SplitUrl {
        scheme,
        netloc,
        path: path.to_owned(),
        query: query.to_owned(),
        fragment: fragment.to_owned(),
    })
}

impl SplitUrl {
    /// Python's `SplitResult.username`.
    pub fn username(&self) -> Option<&str> {
        self.netloc
            .rsplit_once('@')
            .map(|(info, _)| info.split_once(':').map_or(info, |(user, _)| user))
    }

    /// Python's `SplitResult.password`.
    pub fn password(&self) -> Option<&str> {
        self.netloc
            .rsplit_once('@')
            .and_then(|(info, _)| info.split_once(':').map(|(_, password)| password))
    }

    /// Python's `SplitResult.hostname`: lower-cased, without port or brackets.
    pub fn hostname(&self) -> Option<String> {
        let hostinfo = self
            .netloc
            .rsplit_once('@')
            .map_or(self.netloc.as_str(), |(_, host)| host);
        let host = match hostinfo.split_once('[') {
            Some((_, bracketed)) => bracketed.split_once(']').map_or(bracketed, |(h, _)| h),
            None => hostinfo.split_once(':').map_or(hostinfo, |(h, _)| h),
        };
        if host.is_empty() {
            return None;
        }
        // A scoped IPv6 address keeps the case of its zone.
        Some(match host.split_once('%') {
            Some((name, zone)) => format!("{}%{zone}", name.to_lowercase()),
            None => host.to_lowercase(),
        })
    }
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    let host_and_port = netloc.rsplit_once('@').map_or(netloc, |(_, host)| host);
    let hostname = match host_and_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err("Invalid IPv6 URL".to_owned());
            }
            let (hostname, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return Err("Invalid IPv6 URL".to_owned());
            }
            hostname
        }
        None => host_and_port
            .split_once(':')
            .map_or(host_and_port, |(h, _)| h),
    };
    check_bracketed_host(hostname)
}

fn check_bracketed_host(hostname: &str) -> Result<(), String> {
    if hostname.starts_with(['v', 'V']) {
        // IPvFuture: "v" HEXDIG+ "." something
        let body = &hostname[1..];
        let hex_len = body.bytes().take_while(u8::is_ascii_hexdigit).count();
        let valid = hex_len > 0
            && body[hex_len..]
                .strip_prefix('.')
                .is_some_and(|tail| !tail.is_empty());
        return if valid {
            Ok(())
        } else {
            Err("IPvFuture address is invalid".to_owned())
        };
    }
    if hostname.parse::<Ipv4Addr>().is_ok() {
        return Err("An IPv4 address cannot be in brackets".to_owned());
    }
    let address = match hostname.split_once('%') {
        Some((address, zone)) if !zone.is_empty() && !zone.contains('%') => address,
        Some(_) => "",
        None => hostname,
    };
    if address.parse::<Ipv6Addr>().is_ok() {
        Ok(())
    } else {
        Err(format!(
            "'{hostname}' does not appear to be an IPv4 or IPv6 address"
        ))
    }
}

/// Python's guard against host names such as "a\u{2100}b" that gain `/`, `?`, `#`, `@` or
/// `:` under NFKC, the normalisation IDNA applies.
fn check_netloc(netloc: &str) -> Result<(), String> {
    if netloc.is_ascii() {
        return Ok(());
    }
    let stripped: String = netloc
        .chars()
        .filter(|c| !matches!(c, '@' | ':' | '#' | '?'))
        .collect();
    let normalized = ComposingNormalizerBorrowed::new_nfkc().normalize(&stripped);
    if normalized == stripped {
        return Ok(());
    }
    if normalized.contains(['/', '?', '#', '@', ':']) {
        return Err(format!(
            "netloc '{netloc}' contains invalid characters under NFKC normalization"
        ));
    }
    Ok(())
}

/// Python's `urllib.parse.quote(text, safe)`: UTF-8 bytes, with letters, digits, `_.-~`
/// and the `safe` characters left as they are and everything else written as `%XX`.
pub fn quote(text: &str, safe: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'-' | b'~')
            || (byte.is_ascii() && safe.as_bytes().contains(&byte))
        {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Python's `urllib.parse.quote_plus(text)`: as `quote` with nothing extra safe, but
/// spaces become `+`.
pub fn quote_plus(text: &str) -> String {
    if text.contains(' ') {
        quote(text, " ").replace(' ', "+")
    } else {
        quote(text, "")
    }
}

/// Python's `urllib.parse.urlencode(params)` for string values, in the order given.
pub fn urlencode(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", quote_plus(key), quote_plus(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_like_python() {
        let parts = urlsplit("HTTPS://User:pw@Example.org:8443/a/b?x=1#frag").unwrap();
        assert_eq!(parts.scheme, "https");
        assert_eq!(parts.netloc, "User:pw@Example.org:8443");
        assert_eq!(parts.path, "/a/b");
        assert_eq!(parts.query, "x=1");
        assert_eq!(parts.fragment, "frag");
        assert_eq!(parts.username(), Some("User"));
        assert_eq!(parts.password(), Some("pw"));
        assert_eq!(parts.hostname().as_deref(), Some("example.org"));
        let bare = urlsplit("https://@host/").unwrap();
        assert_eq!(bare.username(), Some(""));
        assert_eq!(bare.password(), None);
        assert_eq!(urlsplit("https:example.org").unwrap().netloc, "");
        assert_eq!(urlsplit("1http://x").unwrap().scheme, "");
        assert_eq!(
            urlsplit("http://[::1]:80/").unwrap().hostname().as_deref(),
            Some("::1")
        );
    }

    #[test]
    fn rejects_what_python_rejects() {
        for bad in [
            "http://[::1/",
            "http://x]/",
            "http://a[::1]/",
            "http://[::1]x/",
            "http://[1.2.3.4]/",
            "http://[vZ.x]/",
            "http://[nothex]/",
            "http://a\u{2100}b/",
        ] {
            assert!(urlsplit(bad).is_err(), "{bad}");
        }
        assert!(urlsplit("http://[v1.x]/").is_ok());
        assert!(urlsplit("http://bücher.example/").is_ok());
    }

    #[test]
    fn encodes_like_python() {
        assert_eq!(
            quote("10.1000/xyz(1) é", "/"),
            "10.1000/xyz%281%29%20%C3%A9"
        );
        assert_eq!(
            quote_plus("\"Ada Lovelace\" in:fullname"),
            "%22Ada+Lovelace%22+in%3Afullname"
        );
        assert_eq!(quote_plus("a+b"), "a%2Bb");
        assert_eq!(
            urlencode(&[
                ("filter[entity.legalName]", "X Ltd".into()),
                ("page[size]", "20".into())
            ]),
            "filter%5Bentity.legalName%5D=X+Ltd&page%5Bsize%5D=20"
        );
    }
}
