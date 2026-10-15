// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Domain registration and historical-web evidence. These sources ask registries and
//! archives about a domain; none of them contacts the target website.

use std::net::Ipv4Addr;

use serde_json::{json, Map, Value};

use super::{as_object, dedupe, list_member, shapes, str_of, strings, Context};
use crate::http::SourceError;
use crate::models::{sha256_hex, Invalid, Observation};
use crate::pyfmt::strip;
use crate::pyjson::{scalar_text, truthy};
use crate::urlparse::{quote, urlsplit};

type Found = Result<Vec<Observation>, SourceError>;

/// A domain name in the form the sources expect: international names converted to their
/// `xn--` form, lower case, no trailing dot. URLs, ports, wildcards and IP addresses are
/// refused.
pub fn canonical_domain(value: &str) -> Result<String, Invalid> {
    let value = strip(value).trim_end_matches('.');
    if value.contains("://") || value.contains(['/', '?', '#', '@', ':', '*', '\\']) {
        return Err(Invalid::new(
            "supply a domain name, without a URL, port, or wildcard",
        ));
    }
    let idna_error = || Invalid::new("invalid internationalized domain name");
    let name = if value.is_ascii() {
        value.to_ascii_lowercase()
    } else {
        // UTS #46, as browsers and the WHATWG URL standard convert names.
        idna::domain_to_ascii(value).map_err(|_| idna_error())?
    };
    // Python's IDNA codec refused empty labels and labels of 64 or more characters.
    let labels: Vec<&str> = name.split('.').collect();
    if let Some((last, rest)) = labels.split_last() {
        if rest
            .iter()
            .any(|label| label.is_empty() || label.len() >= 64)
            || last.len() >= 64
        {
            return Err(idna_error());
        }
    }
    if labels.len() < 2 || name.len() > 253 || !labels.iter().all(|label| shapes::ldh_label(label))
    {
        return Err(Invalid::new("invalid domain name"));
    }
    if name.parse::<Ipv4Addr>().is_ok() {
        return Err(Invalid::new("IP addresses are not domain names"));
    }
    Ok(name)
}

/// The host is the domain or one of its subdomains.
fn is_domain_host(host: &str, domain: &str) -> bool {
    let host = host.to_lowercase();
    host.trim_end_matches('.') == domain || host.ends_with(&format!(".{domain}"))
}

/// The registration record from the registry IANA names for the domain's suffix (the most
/// specific one). Status, registration, expiry and change dates, and name servers are kept;
/// contacts are not.
pub fn rdap(domain: &str, ctx: &mut Context<'_>) -> Found {
    let bootstrap = ctx
        .http
        .get_json("https://data.iana.org/rdap/dns.json", &[], &[])?;
    let services = list_member(&bootstrap, "services", false)
        .ok_or_else(|| SourceError::new("IANA RDAP bootstrap has no services list"))?;
    let mut best: Option<(usize, &str)> = None;
    for service in services {
        let Some(pair) = service.as_array().filter(|pair| pair.len() == 2) else {
            continue;
        };
        let (Some(suffixes), Some(endpoints)) = (pair[0].as_array(), pair[1].as_array()) else {
            continue;
        };
        for suffix in suffixes.iter().filter_map(Value::as_str) {
            let lower = suffix.to_lowercase();
            if domain != lower && !domain.ends_with(&format!(".{lower}")) {
                continue;
            }
            for base in endpoints.iter().filter_map(Value::as_str) {
                let length = suffix.chars().count();
                if base.starts_with("https://") && best.is_none_or(|(longest, _)| length > longest)
                {
                    best = Some((length, base));
                }
            }
        }
    }
    let Some((_, base)) = best else {
        return Err(SourceError::new(
            "IANA has no HTTPS RDAP service for this domain suffix",
        ));
    };
    let parsed = urlsplit(base).map_err(SourceError)?;
    let has_credentials = parsed.username().is_some_and(|user| !user.is_empty())
        || parsed.password().is_some_and(|pass| !pass.is_empty());
    if parsed.hostname().is_none()
        || has_credentials
        || !parsed.query.is_empty()
        || !parsed.fragment.is_empty()
    {
        return Err(SourceError::new("invalid IANA RDAP service URL"));
    }
    let url = format!(
        "{}/domain/{}",
        base.trim_end_matches('/'),
        quote(domain, "")
    );
    let data = ctx.http.get_json(&url, &[], &[])?;
    if data.get("errorCode").is_some_and(truthy) {
        return Err(SourceError::new("RDAP returned a domain lookup error"));
    }
    let answered = str_of(&data, "ldhName")
        .is_some_and(|name| name.to_lowercase().trim_end_matches('.') == domain);
    if !answered {
        return Err(SourceError::new(
            "RDAP response did not identify the requested domain",
        ));
    }
    let mut fields = Map::new();
    fields.insert("domain".into(), json!([domain]));
    if let Some(Value::Array(statuses)) = data.get("status") {
        let kept: Vec<String> = statuses
            .iter()
            .take(20)
            .filter_map(Value::as_str)
            .filter(|status| !strip(status).is_empty())
            .map(str::to_owned)
            .collect();
        fields.insert("status".into(), strings(kept));
    }
    if let Some(Value::Array(events)) = data.get("events") {
        for event in events.iter().take(30).filter_map(Value::as_object) {
            let Some(date) = str_of(event, "eventDate") else {
                continue;
            };
            let action = match event.get("eventAction") {
                None => String::new(),
                Some(value) => scalar_text(value).unwrap_or_default(),
            };
            let key = match action.to_lowercase().as_str() {
                "registration" => "registered",
                "expiration" => "expires",
                "last changed" => "changed",
                _ => continue,
            };
            fields.insert(key.into(), json!([date]));
        }
    }
    if let Some(Value::Array(servers)) = data.get("nameservers") {
        let names: Vec<String> = servers
            .iter()
            .take(30)
            .filter_map(Value::as_object)
            .filter_map(|server| str_of(server, "ldhName").map(str::to_owned))
            .collect();
        if !names.is_empty() {
            fields.insert("nameserver".into(), strings(dedupe(names)));
        }
    }
    Ok(vec![Observation::from_mapping(&json!({
        "source": "rdap",
        "source_id": domain,
        "source_url": url,
        "name": domain,
        "subject_type": "domain",
        "fields": fields,
        "kind": "record",
    }))?])
}

/// Archived pages of the domain and its subdomains (first 20 distinct captures). Leads: a
/// capture is historical page evidence, not ownership evidence.
pub fn wayback(domain: &str, ctx: &mut Context<'_>) -> Found {
    let rows = ctx.http.get_json_array(
        "https://web.archive.org/cdx/search/cdx",
        &[
            ("url", format!("{domain}/*")),
            ("output", "json".into()),
            ("fl", "timestamp,original,statuscode,mimetype".into()),
            ("filter", "statuscode:200".into()),
            ("collapse", "urlkey".into()),
            ("limit", "20".into()),
        ],
        &[],
    )?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let expected = json!(["timestamp", "original", "statuscode", "mimetype"]);
    if rows[0] != expected {
        return Err(SourceError::new("Wayback CDX returned unexpected columns"));
    }
    let mut found = Vec::new();
    for row in rows.iter().skip(1).take(20) {
        let Some(row) = row.as_array().filter(|row| row.len() == 4) else {
            continue;
        };
        let (Some(stamp), Some(original), Some(status)) =
            (row[0].as_str(), row[1].as_str(), row[2].as_str())
        else {
            continue;
        };
        if !shapes::wayback_stamp(stamp) || status != "200" {
            continue;
        }
        // An address Python's urlsplit refused is skipped rather than failing the source.
        let Ok(parsed) = urlsplit(original) else {
            continue;
        };
        let credentials = parsed.username().is_some_and(|user| !user.is_empty())
            || parsed.password().is_some_and(|pass| !pass.is_empty());
        let on_domain = parsed
            .hostname()
            .is_some_and(|host| is_domain_host(&host, domain));
        if !matches!(parsed.scheme.as_str(), "http" | "https") || credentials || !on_domain {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("archived_url".into(), json!([original]));
        fields.insert("capture_time".into(), json!([stamp]));
        if let Some(mime) = row[3].as_str().filter(|mime| !mime.is_empty()) {
            fields.insert("mime_type".into(), json!([mime]));
        }
        let fingerprint = format!("{stamp}\0{original}");
        found.push(Observation::from_mapping(&json!({
            "source": "wayback",
            "source_id": &sha256_hex(fingerprint.as_bytes())[..24],
            "source_url": format!("https://web.archive.org/web/{stamp}/{original}"),
            "name": domain,
            "subject_type": "domain",
            "kind": "lead",
            "fields": fields,
        }))?);
    }
    Ok(found)
}

/// Existing public urlscan.io scans whose page domain is the domain or a subdomain. Leads:
/// a scan does not establish ownership.
pub fn urlscan(domain: &str, ctx: &mut Context<'_>) -> Found {
    let headers: Vec<(&str, String)> = ctx
        .env
        .var("URLSCAN_API_KEY")
        .map(|key| vec![("api-key", key)])
        .unwrap_or_default();
    let data = ctx.http.get_json(
        "https://urlscan.io/api/v1/search/",
        &[("q", format!("domain:{domain}")), ("size", "20".into())],
        &headers,
    )?;
    let results = list_member(&data, "results", false)
        .ok_or_else(|| SourceError::new("urlscan search has no results list"))?;
    let mut found = Vec::new();
    for hit in results.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(identifier) = str_of(hit, "_id").filter(|id| shapes::scan_id(id)) else {
            continue;
        };
        let Some(page) = as_object(hit.get("page")) else {
            continue;
        };
        let Some(scanned) = str_of(page, "domain").filter(|host| is_domain_host(host, domain))
        else {
            continue;
        };
        let mut fields = Map::new();
        fields.insert("scanned_domain".into(), json!([scanned]));
        if let Some(url) = str_of(page, "url") {
            fields.insert("scanned_url".into(), json!([url]));
        }
        if let Some(time) = as_object(hit.get("task")).and_then(|task| str_of(task, "time")) {
            fields.insert("scan_time".into(), json!([time]));
        }
        found.push(Observation::from_mapping(&json!({
            "source": "urlscan",
            "source_id": identifier,
            "source_url": format!("https://urlscan.io/result/{identifier}/"),
            "name": domain,
            "subject_type": "domain",
            "kind": "lead",
            "fields": fields,
        }))?);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_domains() {
        assert_eq!(
            canonical_domain("BÜCHER.example.").unwrap(),
            "xn--bcher-kva.example"
        );
        assert_eq!(canonical_domain(" Example.ORG ").unwrap(), "example.org");
        for (bad, message) in [
            (
                "https://example.org",
                "supply a domain name, without a URL, port, or wildcard",
            ),
            (
                "example.org:443",
                "supply a domain name, without a URL, port, or wildcard",
            ),
            (
                "*.example.org",
                "supply a domain name, without a URL, port, or wildcard",
            ),
            ("127.0.0.1", "IP addresses are not domain names"),
            ("localhost", "invalid domain name"),
            ("a..b", "invalid internationalized domain name"),
            (".example.org", "invalid internationalized domain name"),
            ("_dmarc.example.org", "invalid domain name"),
            ("", "invalid domain name"),
        ] {
            assert_eq!(canonical_domain(bad).unwrap_err().0, message, "{bad}");
        }
        let long = format!("{}.com", "a".repeat(64));
        assert_eq!(
            canonical_domain(&long).unwrap_err().0,
            "invalid internationalized domain name"
        );
    }

    #[test]
    fn domain_hosts() {
        assert!(is_domain_host("Example.org.", "example.org"));
        assert!(is_domain_host("www.example.org", "example.org"));
        assert!(!is_domain_host("example.org.evil.test", "example.org"));
        assert!(!is_domain_host("www.example.org.", "example.org"));
    }
}
