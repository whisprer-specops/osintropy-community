// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Legal-entity registers, kept separate from people and from one another. Names must
//! match exactly after whitespace and case normalisation; nothing fuzzy is accepted.

use serde_json::{json, Map, Value};

use super::person::companies_key;
use super::{as_object, list_member, object_or_empty, optional, shapes, str_of, Context};
use crate::http::SourceError;
use crate::models::{normalized_name, Observation};
use crate::urlparse::quote;

type Found = Result<Vec<Observation>, SourceError>;

/// UK companies whose current name matches exactly; historical names are not accepted.
pub fn companies_house_companies(name: &str, ctx: &mut Context<'_>) -> Found {
    let headers = companies_key(ctx)?;
    let data = ctx.http.get_json(
        "https://api.company-information.service.gov.uk/search/companies",
        &[("q", name.to_owned()), ("items_per_page", "20".into())],
        &headers,
    )?;
    let items = list_member(&data, "items", true)
        .ok_or_else(|| SourceError::new("Companies House company search has no items list"))?;
    let wanted = normalized_name(name);
    let mut found = Vec::new();
    for hit in items.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(title) = str_of(hit, "title") else {
            continue;
        };
        let Some(number) = str_of(hit, "company_number") else {
            continue;
        };
        if normalized_name(title) != wanted || !shapes::upper_alnum(number, 2, 12) {
            continue;
        }
        let mut fields = Map::new();
        fields.insert("company_number".into(), json!([number]));
        for (key, target) in [
            ("company_status", "status"),
            ("date_of_creation", "incorporated"),
            ("company_type", "company_type"),
        ] {
            optional(&mut fields, target, hit.get(key));
        }
        if let Some(address) = object_or_empty(hit.get("address")) {
            for key in ["locality", "region", "country"] {
                optional(&mut fields, &format!("registered_{key}"), address.get(key));
            }
        }
        found.push(Observation::from_mapping(&json!({
            "source": "companies_house_companies",
            "source_id": number,
            "source_url": format!("https://find-and-update.company-information.service.gov.uk/company/{number}"),
            "name": title,
            "subject_type": "organization",
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}

/// Legal Entity Identifier records whose legal name matches exactly.
pub fn gleif(name: &str, ctx: &mut Context<'_>) -> Found {
    let data = ctx.http.get_json(
        "https://api.gleif.org/api/v1/lei-records",
        &[
            ("filter[entity.legalName]", name.to_owned()),
            ("page[size]", "20".into()),
        ],
        &[],
    )?;
    let records = list_member(&data, "data", false)
        .ok_or_else(|| SourceError::new("GLEIF search has no data list"))?;
    let wanted = normalized_name(name);
    let mut found = Vec::new();
    for hit in records.iter().take(20) {
        let Some(hit) = hit.as_object() else { continue };
        let Some(attributes) = as_object(hit.get("attributes")) else {
            continue;
        };
        let Some(entity) = object_or_empty(attributes.get("entity")) else {
            continue;
        };
        let legal_name = match entity.get("legalName") {
            Some(Value::Object(legal)) => legal.get("name"),
            Some(other) if crate::pyjson::truthy(other) => Some(other),
            _ => None,
        };
        let Some(Value::String(legal_name)) = legal_name else {
            continue;
        };
        if normalized_name(legal_name) != wanted {
            continue;
        }
        let Some(lei) = str_of(hit, "id").filter(|lei| shapes::upper_alnum(lei, 20, 20)) else {
            continue;
        };
        let mut fields = Map::new();
        fields.insert("lei".into(), json!([lei]));
        optional(
            &mut fields,
            "legal_jurisdiction",
            entity.get("jurisdiction"),
        );
        optional(&mut fields, "entity_status", entity.get("status"));
        if let Some(registration) = object_or_empty(attributes.get("registration")) {
            optional(
                &mut fields,
                "registration_status",
                registration.get("status"),
            );
        }
        if let Some(address) = object_or_empty(entity.get("legalAddress")) {
            for key in ["city", "region", "country"] {
                optional(&mut fields, &format!("legal_{key}"), address.get(key));
            }
        }
        if let Some(relationships) = object_or_empty(hit.get("relationships")) {
            if let Some(parent) = object_or_empty(relationships.get("direct-parent")) {
                if let Some(data) = as_object(parent.get("data")) {
                    optional(&mut fields, "direct_parent_lei", data.get("id"));
                }
            }
        }
        found.push(Observation::from_mapping(&json!({
            "source": "gleif",
            "source_id": lei,
            "source_url": format!("https://search.gleif.org/#/record/{lei}"),
            "name": legal_name,
            "subject_type": "organization",
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}

/// Company records across jurisdictions; the source ID carries the jurisdiction, so equal
/// company numbers in different places stay apart. The API requires the token in the
/// query string.
pub fn opencorporates(name: &str, ctx: &mut Context<'_>) -> Found {
    let token = ctx
        .env
        .var("OPENCORPORATES_API_TOKEN")
        .ok_or_else(|| SourceError::new("set OPENCORPORATES_API_TOKEN to use OpenCorporates"))?;
    let data = ctx.http.get_json(
        "https://api.opencorporates.com/v0.4/companies/search",
        &[
            ("q", name.to_owned()),
            ("per_page", "20".into()),
            ("api_token", token),
        ],
        &[],
    )?;
    let companies = as_object(data.get("results"))
        .and_then(|results| list_member(results, "companies", false))
        .ok_or_else(|| SourceError::new("OpenCorporates search has no companies list"))?;
    let wanted = normalized_name(name);
    let mut found = Vec::new();
    for wrapper in companies.iter().take(20) {
        let Some(hit) = wrapper
            .as_object()
            .and_then(|w| as_object(w.get("company")))
        else {
            continue;
        };
        let Some(company_name) = str_of(hit, "name") else {
            continue;
        };
        if normalized_name(company_name) != wanted {
            continue;
        }
        let Some(jurisdiction) =
            str_of(hit, "jurisdiction_code").filter(|code| shapes::jurisdiction(code))
        else {
            continue;
        };
        let Some(number) = str_of(hit, "company_number")
            .filter(|number| !number.is_empty() && number.chars().count() <= 100)
        else {
            continue;
        };
        let mut fields = Map::new();
        fields.insert("jurisdiction".into(), json!([jurisdiction]));
        fields.insert("company_number".into(), json!([number]));
        for (key, target) in [
            ("current_status", "status"),
            ("incorporation_date", "incorporated"),
            ("registered_address_in_full", "registered_address"),
        ] {
            optional(&mut fields, target, hit.get(key));
        }
        found.push(Observation::from_mapping(&json!({
            "source": "opencorporates",
            "source_id": format!("{jurisdiction}/{number}"),
            "source_url": format!("https://opencorporates.com/companies/{jurisdiction}/{}", quote(number, "")),
            "name": company_name,
            "subject_type": "organization",
            "fields": fields,
            "kind": "record",
        }))?);
    }
    Ok(found)
}
