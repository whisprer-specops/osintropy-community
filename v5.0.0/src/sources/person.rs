// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Published person and account searches. Search hits never resolve identities.

use base64::Engine as _;
use serde_json::{json, Map, Value};

use super::{
    as_object, dedupe, exact_name, list_member, name_for_lead, object_or_empty, optional, shapes,
    str_of, strings, Context, PersonQuery,
};
use crate::http::SourceError;
use crate::models::{normalized_name, sha256_hex, Observation};
use crate::pyfmt::{casefold, strip};
use crate::pyjson::{equals_zero, integer_text, scalar_text, truthy};
use crate::urlparse::quote;

type Found = Result<Vec<Observation>, SourceError>;

fn text_or_empty(value: Option<&Value>) -> String {
    value.and_then(scalar_text).unwrap_or_default()
}

/// Individual NPPES providers (NPI-1) with an exact name; only public practice locations
/// are kept, never mailing addresses. `--location "City, ST"` narrows by practice city.
pub fn npi(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let (first, last, location) = (&query.first, &query.last, &query.location);
    let mut params = vec![
        ("version", "2.1".to_owned()),
        ("enumeration_type", "NPI-1".to_owned()),
        ("first_name", first.clone()),
        ("last_name", last.clone()),
        ("limit", "20".to_owned()),
    ];
    let wanted_city = location.split(',').next().map(strip).unwrap_or_default();
    if !location.is_empty() {
        let parts: Vec<&str> = location.split(',').map(strip).collect();
        params.push(("city", parts[0].to_owned()));
        if parts.len() > 1 && parts[1].chars().count() == 2 {
            params.push(("state", parts[1].to_uppercase()));
        }
    }
    let data = ctx
        .http
        .get_json("https://npiregistry.cms.hhs.gov/api/", &params, &[])?;
    if data.get("Errors").is_some_and(truthy) || data.get("error").is_some_and(truthy) {
        return Err(SourceError::new(
            "NPI returned an API error; check parameters or query limits",
        ));
    }
    let results = list_member(&data, "results", true)
        .ok_or_else(|| SourceError::new("NPI response has no results list"))?;
    let mut found = Vec::new();
    for hit in results.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        if str_of(hit, "enumeration_type") != Some("NPI-1") {
            continue;
        }
        let Some(basic) = object_or_empty(hit.get("basic")) else {
            continue;
        };
        let name = ["first_name", "middle_name", "last_name"]
            .iter()
            .filter_map(|key| basic.get(*key))
            .filter(|value| truthy(value))
            .filter_map(scalar_text)
            .collect::<Vec<_>>()
            .join(" ");
        if !exact_name(first, last, &name) {
            continue;
        }
        let number = text_or_empty(hit.get("number"));
        if number.len() != 10 || !shapes::digits(&number) {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("npi".into(), json!([number]));
        let practice: Vec<&Map<String, Value>> = match hit.get("addresses") {
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_object)
                .filter(|address| {
                    let purpose = address
                        .get("address_purpose")
                        .map_or_else(String::new, |value| text_or_empty(Some(value)));
                    purpose.to_uppercase() == "LOCATION"
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut addresses = Vec::new();
        let mut phones = Vec::new();
        for address in practice {
            if !location.is_empty() {
                let city = match address.get("city") {
                    None => Some(String::new()),
                    Some(value) => scalar_text(value),
                };
                if city.map(|city| casefold(&city)) != Some(casefold(wanted_city)) {
                    continue;
                }
            }
            let formatted = ["address_1", "address_2", "city", "state", "postal_code"]
                .iter()
                .filter_map(|key| address.get(*key))
                .filter(|value| truthy(value))
                .filter_map(scalar_text)
                .map(|part| strip(&part).to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            if !formatted.is_empty() {
                addresses.push(formatted);
            }
            if let Some(phone) = address
                .get("telephone_number")
                .filter(|value| truthy(value))
            {
                if let Some(phone) = scalar_text(phone) {
                    phones.push(phone);
                }
            }
        }
        if !location.is_empty() && addresses.is_empty() {
            continue;
        }
        if !addresses.is_empty() {
            fields.insert("practice_address".into(), strings(dedupe(addresses)));
        }
        if !phones.is_empty() {
            fields.insert("practice_phone".into(), strings(dedupe(phones)));
        }
        found.push(Observation::from_mapping(&json!({
            "source": "npi",
            "source_id": number,
            "source_url": format!("https://npiregistry.cms.hhs.gov/provider-view/{number}"),
            "name": name,
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}

/// The `value.id` of each claim on a Wikidata property.
fn claim_ids(entity: &Map<String, Value>, property: &str) -> Vec<String> {
    let Some(claims) = object_or_empty(entity.get("claims")) else {
        return Vec::new();
    };
    let Some(Value::Array(claims)) = claims.get(property) else {
        return Vec::new();
    };
    claims
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|claim| {
            let snak = object_or_empty(claim.get("mainsnak"))?;
            let data = object_or_empty(snak.get("datavalue"))?;
            let value = as_object(data.get("value"))?;
            str_of(value, "id").map(str::to_owned)
        })
        .collect()
}

/// `((outer.get(key) or {}).get("en") or {}).get("value")` for labels and descriptions.
fn english<'a>(entity: &'a Map<String, Value>, key: &str) -> Option<Option<&'a Value>> {
    let group = object_or_empty(entity.get(key))?;
    let language = object_or_empty(group.get("en"))?;
    Some(language.get("value"))
}

/// Wikidata items that are instances of human (Q5) with an exact English label.
pub fn wikidata(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let url = "https://www.wikidata.org/w/api.php";
    let name = format!("{} {}", query.first, query.last);
    let search = ctx.http.get_json(
        url,
        &[
            ("action", "wbsearchentities".into()),
            ("search", name.clone()),
            ("language", "en".into()),
            ("format", "json".into()),
            ("limit", "10".into()),
            ("type", "item".into()),
        ],
        &[],
    )?;
    if search.get("error").is_some_and(truthy) || search.get("success").is_some_and(equals_zero) {
        return Err(SourceError::new("Wikidata search returned an API error"));
    }
    let hits = list_member(&search, "search", true)
        .ok_or_else(|| SourceError::new("Wikidata response has no search list"))?;
    let wanted = normalized_name(&name);
    let ids: Vec<String> = hits
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|hit| {
            let id = str_of(hit, "id")?;
            let numbered = id.strip_prefix('Q').is_some_and(shapes::digits);
            let label = hit.get("label").map_or(Some(String::new()), scalar_text)?;
            (numbered && normalized_name(&label) == wanted).then(|| id.to_owned())
        })
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let entity_data = ctx.http.get_json(
        url,
        &[
            ("action", "wbgetentities".into()),
            ("ids", ids.join("|")),
            ("props", "labels|descriptions|claims".into()),
            ("languages", "en".into()),
            ("format", "json".into()),
        ],
        &[],
    )?;
    if entity_data.get("error").is_some_and(truthy)
        || entity_data.get("success").is_some_and(equals_zero)
    {
        return Err(SourceError::new(
            "Wikidata entity lookup returned an API error",
        ));
    }
    let entities = as_object(entity_data.get("entities"))
        .ok_or_else(|| SourceError::new("Wikidata response has no entities object"))?;
    let mut found = Vec::new();
    for qid in &ids {
        let Some(entity) = as_object(entities.get(qid)) else {
            continue;
        };
        if !claim_ids(entity, "P31").iter().any(|id| id == "Q5") {
            continue;
        }
        let Some(label) = english(entity, "labels") else {
            continue;
        };
        let label = label
            .cloned()
            .unwrap_or_else(|| Value::String(name.clone()));
        if scalar_text(&label).map(|text| normalized_name(&text)) != Some(wanted.clone()) {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("wikidata_id".into(), json!([qid]));
        if let Some(Some(description)) = english(entity, "descriptions") {
            if truthy(description) {
                fields.insert(
                    "description".into(),
                    Value::Array(vec![description.clone()]),
                );
            }
        }
        for (property, field) in [("P106", "occupation_entity"), ("P108", "employer_entity")] {
            let values: Vec<String> = claim_ids(entity, property).into_iter().take(10).collect();
            if !values.is_empty() {
                fields.insert(field.into(), strings(values));
            }
        }
        found.push(Observation::from_mapping(&json!({
            "source": "wikidata",
            "source_id": qid,
            "source_url": format!("https://www.wikidata.org/wiki/{qid}"),
            "name": label,
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}

/// Google Custom Search results, for existing API customers only. A snippet is never taken
/// to describe the person, so every hit is a lead.
pub fn google_cse(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let (Some(key), Some(cx)) = (
        ctx.env.var("GOOGLE_CSE_API_KEY"),
        ctx.env.var("GOOGLE_CSE_CX"),
    ) else {
        return Err(SourceError::new(
            "set GOOGLE_CSE_API_KEY and GOOGLE_CSE_CX for an existing API account",
        ));
    };
    let mut q = format!("\"{} {}\"", query.first, query.last);
    if !query.location.is_empty() {
        q.push_str(&format!(" \"{}\"", query.location));
    }
    let data = ctx.http.get_json(
        "https://www.googleapis.com/customsearch/v1",
        &[("key", key), ("cx", cx), ("q", q), ("num", "10".into())],
        &[],
    )?;
    if data.get("error").is_some_and(truthy) {
        return Err(SourceError::new(
            "Google CSE returned an API error; check quota and account status",
        ));
    }
    let items = list_member(&data, "items", true)
        .ok_or_else(|| SourceError::new("Google CSE response has no items list"))?;
    let mut found = Vec::new();
    for hit in items.iter().take(10) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(link) = str_of(hit, "link") else {
            continue;
        };
        let source_id = sha256_hex(link.as_bytes())[..24].to_owned();
        let title = hit.get("title").cloned().unwrap_or_else(|| json!(""));
        let snippet = hit.get("snippet").cloned().unwrap_or_else(|| json!(""));
        // A hit that fails validation is skipped, not fatal, as in 4.1.1.
        if let Ok(observation) = Observation::from_mapping(&json!({
            "source": "google_cse",
            "source_id": source_id,
            "source_url": link,
            "name": name_for_lead(&query.first, &query.last),
            "kind": "lead",
            "fields": {"page_title": [title], "snippet": [snippet]},
        })) {
            found.push(observation);
        }
    }
    Ok(found)
}

/// The Companies House key as HTTP Basic credentials (key as user, empty password).
pub(crate) fn companies_key(ctx: &Context<'_>) -> Result<Vec<(&'static str, String)>, SourceError> {
    let key = ctx
        .env
        .var("COMPANIES_HOUSE_API_KEY")
        .ok_or_else(|| SourceError::new("set COMPANIES_HOUSE_API_KEY to use Companies House"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{key}:"));
    Ok(vec![("Authorization", format!("Basic {encoded}"))])
}

/// UK company officers with an exact name. Service locality, region and country are kept;
/// full address lines never are. Distinct officers can share a name.
pub fn companies_house_officers(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let headers = companies_key(ctx)?;
    let data = ctx.http.get_json(
        "https://api.company-information.service.gov.uk/search/officers",
        &[
            ("q", format!("{} {}", query.first, query.last)),
            ("items_per_page", "20".into()),
        ],
        &headers,
    )?;
    let items = list_member(&data, "items", true)
        .ok_or_else(|| SourceError::new("Companies House officer search has no items list"))?;
    let mut found = Vec::new();
    for hit in items.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(title) = str_of(hit, "title") else {
            continue;
        };
        let title = strip(title);
        let name = match title.split_once(',') {
            Some((family, given)) => format!("{} {}", strip(given), strip(family)),
            None => title.to_owned(),
        };
        if !exact_name(&query.first, &query.last, &name) {
            continue;
        }
        let path = as_object(hit.get("links")).and_then(|links| str_of(links, "self"));
        let Some(path) = path.filter(|path| shapes::officer_path(path)) else {
            continue;
        };
        let officer = path.split('/').nth(2).unwrap_or_default();
        let mut fields = Map::new();
        fields.insert("officer_record".into(), json!([officer]));
        optional(
            &mut fields,
            "appointment_count",
            hit.get("appointment_count"),
        );
        optional(&mut fields, "description", hit.get("description"));
        if let Some(address) = object_or_empty(hit.get("address")) {
            for key in ["locality", "region", "country"] {
                optional(&mut fields, &format!("service_{key}"), address.get(key));
            }
        }
        if let Some(born) = object_or_empty(hit.get("date_of_birth")) {
            optional(&mut fields, "birth_year", born.get("year"));
            optional(&mut fields, "birth_month", born.get("month"));
        }
        found.push(Observation::from_mapping(&json!({
            "source": "companies_house_officers",
            "source_id": officer,
            "source_url": format!("https://find-and-update.company-information.service.gov.uk{path}"),
            "name": name,
            "kind": "record",
            "fields": fields,
        }))?);
    }
    Ok(found)
}

/// OpenAlex author profiles with an exact display name: catalog records that may still
/// need telling apart.
pub fn openalex(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let mut params = vec![
        ("search", format!("{} {}", query.first, query.last)),
        ("per_page", "20".to_owned()),
    ];
    if let Some(key) = ctx.env.var("OPENALEX_API_KEY") {
        params.push(("api_key", key));
    }
    let data = ctx
        .http
        .get_json("https://api.openalex.org/authors", &params, &[])?;
    let results = match (data.get("error").is_some_and(truthy), data.get("results")) {
        (false, Some(Value::Array(results))) => results,
        _ => {
            return Err(SourceError::new(
                "OpenAlex author search failed or returned no results list",
            ))
        }
    };
    let mut found = Vec::new();
    for hit in results.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(name) = str_of(hit, "display_name") else {
            continue;
        };
        let identifier = match hit.get("id") {
            None => "",
            Some(Value::String(id)) => id.as_str(),
            Some(_) => continue,
        };
        if !exact_name(&query.first, &query.last, name) {
            continue;
        }
        let author = identifier
            .rsplit_once('/')
            .map_or(identifier, |(_, last)| last);
        if !shapes::openalex_author(author) {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("openalex_id".into(), json!([author]));
        if let Some(orcid) = str_of(hit, "orcid").filter(|url| shapes::orcid_url(url)) {
            fields.insert("orcid".into(), json!([orcid]));
        }
        optional(&mut fields, "works_count", hit.get("works_count"));
        if let Some(Value::Array(institutions)) = hit.get("last_known_institutions") {
            let names: Vec<String> = institutions
                .iter()
                .take(10)
                .filter_map(Value::as_object)
                .filter_map(|item| str_of(item, "display_name").map(str::to_owned))
                .collect();
            if !names.is_empty() {
                fields.insert("institution".into(), strings(dedupe(names)));
            }
        }
        found.push(Observation::from_mapping(&json!({
            "source": "openalex",
            "source_id": author,
            "source_url": format!("https://openalex.org/authors/{author}"),
            "name": name,
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}

/// Public ORCID iDs with an exact given and family name, using only the `/read-public`
/// scope. A search hit still needs review.
pub fn orcid(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let token = match ctx.env.var("ORCID_ACCESS_TOKEN") {
        Some(token) => token,
        None => {
            let (Some(client_id), Some(secret)) = (
                ctx.env.var("ORCID_CLIENT_ID"),
                ctx.env.var("ORCID_CLIENT_SECRET"),
            ) else {
                return Err(SourceError::new(
                    "set ORCID_ACCESS_TOKEN or both ORCID_CLIENT_ID and ORCID_CLIENT_SECRET",
                ));
            };
            let grant = ctx.http.post_form_json(
                "https://orcid.org/oauth/token",
                &[
                    ("client_id", client_id),
                    ("client_secret", secret),
                    ("grant_type", "client_credentials".into()),
                    ("scope", "/read-public".into()),
                ],
            )?;
            match str_of(&grant, "access_token") {
                Some(token) if !token.is_empty() => token.to_owned(),
                _ => return Err(SourceError::new("ORCID did not return a public-read token")),
            }
        }
    };
    // Solr syntax: quote the user's text so it cannot change the query's structure.
    let escape = |part: &str| part.replace('\\', "\\\\").replace('"', "\\\"");
    let q = format!(
        "given-names:\"{}\" AND family-name:\"{}\"",
        escape(&query.first),
        escape(&query.last)
    );
    let data = ctx.http.get_json(
        "https://pub.orcid.org/v3.0/expanded-search/",
        &[("q", q), ("rows", "20".into())],
        &[
            ("Authorization", format!("Bearer {token}")),
            ("Accept", "application/vnd.orcid+json".into()),
        ],
    )?;
    // ORCID sends `"expanded-result": null` when nothing matches: an empty result.
    let rows: &[Value] = match data.get("expanded-result") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(rows)) => rows,
        Some(_) => {
            return Err(SourceError::new(
                "ORCID expanded search has no results list",
            ))
        }
    };
    let mut found = Vec::new();
    for hit in rows.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(identifier) = str_of(hit, "orcid-id").filter(|id| shapes::orcid_id(id)) else {
            continue;
        };
        let (Some(given), Some(family)) = (str_of(hit, "given-names"), str_of(hit, "family-names"))
        else {
            continue;
        };
        let name = format!("{given} {family}");
        if !exact_name(&query.first, &query.last, &name) {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("orcid".into(), json!([identifier]));
        let institutions: Option<Vec<Value>> = match hit.get("institution-name") {
            Some(Value::String(one)) if !one.is_empty() => Some(vec![Value::String(one.clone())]),
            Some(Value::Array(list)) => Some(list.clone()),
            Some(other) if truthy(other) => None,
            _ => Some(Vec::new()),
        };
        if let Some(list) = institutions {
            let kept: Vec<String> = list
                .iter()
                .take(10)
                .filter_map(Value::as_str)
                .filter(|name| !strip(name).is_empty())
                .map(str::to_owned)
                .collect();
            fields.insert("institution".into(), strings(kept));
        }
        found.push(Observation::from_mapping(&json!({
            "source": "orcid",
            "source_id": identifier,
            "source_url": format!("https://orcid.org/{identifier}"),
            "name": name,
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}

/// Publications with a matching author string. Leads only: an author string in metadata
/// does not identify the person.
pub fn crossref(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let mut params = vec![
        ("query.author", format!("{} {}", query.first, query.last)),
        ("rows", "20".to_owned()),
        ("select", "DOI,title,author,published,publisher".to_owned()),
    ];
    if let Some(email) = ctx.env.var("OSINTROPY_CONTACT_EMAIL") {
        params.push(("mailto", email));
    }
    let data = ctx
        .http
        .get_json("https://api.crossref.org/works", &params, &[])?;
    let items = match (str_of(&data, "status"), as_object(data.get("message"))) {
        (Some("ok"), Some(message)) => list_member(message, "items", false),
        _ => None,
    }
    .ok_or_else(|| SourceError::new("Crossref returned an invalid works response"))?;
    let mut found = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for hit in items.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(Value::Array(authors)) = hit.get("author") else {
            continue;
        };
        let matched = authors
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|author| {
                let given = str_of(author, "given")?;
                let family = str_of(author, "family")?;
                Some(strip(&format!("{given} {family}")).to_owned())
            })
            .find(|name| exact_name(&query.first, &query.last, name));
        let doi = str_of(hit, "DOI");
        let (Some(matched), Some(doi)) = (matched, doi) else {
            continue;
        };
        if matched.is_empty() || !doi.starts_with("10.") || seen.iter().any(|known| known == doi) {
            continue;
        }
        seen.push(doi.to_owned());
        let mut fields = Map::new();
        fields.insert("doi".into(), json!([doi]));
        fields.insert("author_name".into(), json!([matched]));
        if let Some(Value::Array(titles)) = hit.get("title") {
            if let Some(Value::String(_)) = titles.first() {
                optional(&mut fields, "publication_title", titles.first());
            }
        }
        optional(&mut fields, "publisher", hit.get("publisher"));
        if let Some(published) = as_object(hit.get("published")) {
            if let Some(Value::Array(parts)) = published.get("date-parts") {
                if let Some(Value::Array(first)) = parts.first() {
                    if !first.is_empty() {
                        optional(&mut fields, "publication_year", first.first());
                    }
                }
            }
        }
        found.push(Observation::from_mapping(&json!({
            "source": "crossref",
            "source_id": doi,
            "source_url": format!("https://doi.org/{}", quote(doi, "/")),
            "name": name_for_lead(&query.first, &query.last),
            "kind": "lead",
            "fields": fields,
        }))?);
    }
    Ok(found)
}

/// Public Bluesky profiles with an exact display name. Leads only; the DID names the
/// account.
pub fn bluesky(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let data = ctx.http.get_json(
        "https://public.api.bsky.app/xrpc/app.bsky.actor.searchActors",
        &[
            ("q", format!("{} {}", query.first, query.last)),
            ("limit", "20".into()),
        ],
        &[],
    )?;
    let actors = list_member(&data, "actors", false)
        .ok_or_else(|| SourceError::new("Bluesky actor search has no actors list"))?;
    let mut found = Vec::new();
    for hit in actors.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(display) = str_of(hit, "displayName") else {
            continue;
        };
        if !exact_name(&query.first, &query.last, display) {
            continue;
        }
        let Some(did) = str_of(hit, "did").filter(|did| shapes::did(did)) else {
            continue;
        };
        let mut fields = Map::new();
        fields.insert("display_name".into(), json!([display]));
        fields.insert("did".into(), json!([did]));
        optional(&mut fields, "handle", hit.get("handle"));
        found.push(Observation::from_mapping(&json!({
            "source": "bluesky",
            "source_id": did,
            "source_url": format!("https://bsky.app/profile/{}", quote(did, ":")),
            "name": name_for_lead(&query.first, &query.last),
            "kind": "lead",
            "fields": fields,
        }))?);
    }
    Ok(found)
}

/// GitHub users from a full-name search. Leads only: the search response carries no
/// verified name.
pub fn github(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let mut headers = vec![("Accept", "application/vnd.github+json".to_owned())];
    if let Some(token) = ctx.env.var("GITHUB_TOKEN") {
        headers.push(("Authorization", format!("Bearer {token}")));
    }
    let data = ctx.http.get_json(
        "https://api.github.com/search/users",
        &[
            (
                "q",
                format!("\"{} {}\" in:fullname", query.first, query.last),
            ),
            ("per_page", "10".into()),
        ],
        &headers,
    )?;
    let items = list_member(&data, "items", false)
        .ok_or_else(|| SourceError::new("GitHub user search has no items list"))?;
    let mut found = Vec::new();
    for hit in items.iter().take(10) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(login) = str_of(hit, "login") else {
            continue;
        };
        if !shapes::github_login(login) {
            continue;
        }
        found.push(Observation::from_mapping(&json!({
            "source": "github",
            "source_id": login,
            "source_url": format!("https://github.com/{login}"),
            "name": name_for_lead(&query.first, &query.last),
            "kind": "lead",
            "fields": {
                "username": [login],
                "search_context": [format!("Full-name search for {} {}", query.first, query.last)],
            },
        }))?);
    }
    Ok(found)
}

/// US case-law opinions containing the exact name as a phrase. Leads only: a name in a
/// case says nothing of the person's role or identity.
pub fn courtlistener(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let token = ctx
        .env
        .var("COURTLISTENER_API_TOKEN")
        .ok_or_else(|| SourceError::new("set COURTLISTENER_API_TOKEN to use CourtListener"))?;
    let data = ctx.http.get_json(
        "https://www.courtlistener.com/api/rest/v4/search/",
        &[
            ("q", format!("\"{} {}\"", query.first, query.last)),
            ("type", "o".into()),
        ],
        &[("Authorization", format!("Token {token}"))],
    )?;
    let results = list_member(&data, "results", false)
        .ok_or_else(|| SourceError::new("CourtListener search has no results list"))?;
    let mut found = Vec::new();
    for hit in results.iter().take(10) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(identifier) = hit.get("cluster_id").and_then(integer_text) else {
            continue;
        };
        if identifier.starts_with('-') || identifier == "0" {
            continue;
        }
        let Some(path) = str_of(hit, "absolute_url") else {
            continue;
        };
        if !shapes::opinion_path(path) || !path.starts_with(&format!("/opinion/{identifier}/")) {
            continue;
        }
        let mut fields = Map::new();
        for (key, target) in [
            ("caseName", "case_name"),
            ("court", "court"),
            ("dateFiled", "date_filed"),
            ("docketNumber", "docket_number"),
        ] {
            optional(&mut fields, target, hit.get(key));
        }
        found.push(Observation::from_mapping(&json!({
            "source": "courtlistener",
            "source_id": identifier,
            "source_url": format!("https://www.courtlistener.com{path}"),
            "name": name_for_lead(&query.first, &query.last),
            "kind": "lead",
            "fields": fields,
        }))?);
    }
    Ok(found)
}

/// Open Library author entries with an exact catalog name. Catalog entries can be merged
/// or duplicated.
pub fn openlibrary(query: &PersonQuery, ctx: &mut Context<'_>) -> Found {
    let data = ctx.http.get_json(
        "https://openlibrary.org/search/authors.json",
        &[
            ("q", format!("{} {}", query.first, query.last)),
            ("limit", "20".into()),
        ],
        &[],
    )?;
    let docs = list_member(&data, "docs", false)
        .ok_or_else(|| SourceError::new("Open Library author search has no docs list"))?;
    let mut found = Vec::new();
    for hit in docs.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(name) = str_of(hit, "name") else {
            continue;
        };
        let key = match hit.get("key") {
            None => "",
            Some(Value::String(key)) => key.as_str(),
            Some(_) => continue,
        };
        if !exact_name(&query.first, &query.last, name) {
            continue;
        }
        let author = key.rsplit_once('/').map_or(key, |(_, last)| last);
        if !shapes::openlibrary_author(author) {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("openlibrary_id".into(), json!([author]));
        optional(&mut fields, "work_count", hit.get("work_count"));
        optional(&mut fields, "top_work", hit.get("top_work"));
        found.push(Observation::from_mapping(&json!({
            "source": "openlibrary",
            "source_id": author,
            "source_url": format!("https://openlibrary.org/authors/{author}"),
            "name": name,
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}
