// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Connector contracts on synthetic API responses, ported from 4.1.1's
//! `tests/test_pipeline.py` and `tests/test_expanded_sources.py`, plus the 5.0 fixes.
//! Nothing here touches the network.

mod common;

use common::{named, person};
use osintropy::models::{Kind, SubjectType};
use osintropy::sources::domain::{canonical_domain, rdap, urlscan, wayback};
use osintropy::sources::organization::{companies_house_companies, gleif, opencorporates};
use osintropy::sources::person::{
    bluesky, companies_house_officers, courtlistener, crossref, github, google_cse, npi, openalex,
    openlibrary, orcid, wikidata,
};
use serde_json::json;

fn ids(found: &[osintropy::models::Observation]) -> Vec<&str> {
    found.iter().map(|item| item.source_id.as_str()).collect()
}

#[test]
fn npi_accepts_exact_individual_and_only_practice_address() {
    let response = json!({"results": [
        {"enumeration_type": "NPI-1", "number": 1234567890,
         "basic": {"first_name": "Avery", "middle_name": "Q", "last_name": "Example"},
         "addresses": [
             {"address_purpose": "MAILING", "address_1": "Private Road", "city": "Sampleton"},
             {"address_purpose": "LOCATION", "address_1": "1 Practice St", "city": "Sampleton", "state": "CA", "telephone_number": "555-0100"}]},
        {"enumeration_type": "NPI-2", "number": 2_222_222_222_u64,
         "basic": {"first_name": "Avery", "last_name": "Example"}},
        {"enumeration_type": "NPI-1", "number": 3_333_333_333_u64,
         "basic": {"first_name": "Avery", "last_name": "Different"}},
    ]});
    let (found, calls) = person(
        npi,
        "Avery",
        "Example",
        "Sampleton, CA",
        &[],
        vec![response],
    );
    let found = found.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].fields["npi"], ["1234567890"]);
    assert!(found[0].fields["practice_address"][0].contains("1 Practice St"));
    assert!(!format!("{:?}", found[0]).contains("Private Road"));
    assert_eq!(found[0].kind, Kind::Record);
    assert_eq!(calls[0].param("state"), Some("CA"));
    assert_eq!(calls[0].param("city"), Some("Sampleton"));
    let (failed, _) = person(
        npi,
        "Avery",
        "Example",
        "",
        &[],
        vec![json!({"Errors": [{"description": "limit"}]})],
    );
    assert!(failed.is_err());
}

#[test]
fn wikidata_requires_exact_human_entity() {
    let search = json!({"search": [{"id": "Q42", "label": "Avery Example"},
                                   {"id": "Q43", "label": "Avery Example"},
                                   {"id": "Q44", "label": "Avery Examples"}]});
    let entities = json!({"entities": {
        "Q42": {"labels": {"en": {"value": "Avery Example"}},
                "descriptions": {"en": {"value": "fictional engineer"}},
                "claims": {"P31": [{"mainsnak": {"datavalue": {"value": {"id": "Q5"}}}}]}},
        "Q43": {"labels": {"en": {"value": "Avery Example"}},
                "claims": {"P31": [{"mainsnak": {"datavalue": {"value": {"id": "Q11424"}}}}]}}
    }});
    let (found, calls) = person(
        wikidata,
        "Avery",
        "Example",
        "",
        &[],
        vec![search, entities],
    );
    let found = found.unwrap();
    assert_eq!(ids(&found), ["Q42"]);
    assert_eq!(found[0].fields["description"], ["fictional engineer"]);
    assert_eq!(calls[1].param("ids"), Some("Q42|Q43"));
}

#[test]
fn google_result_is_only_a_lead_and_requires_credentials() {
    let (refused, calls) = person(google_cse, "Avery", "Example", "", &[], vec![]);
    assert!(refused.is_err());
    assert!(calls.is_empty(), "no request is made without credentials");
    let (found, calls) = person(
        google_cse,
        "Avery",
        "Example",
        "",
        &[("GOOGLE_CSE_API_KEY", "test"), ("GOOGLE_CSE_CX", "test")],
        vec![
            json!({"items": [{"link": "https://example.org/a", "title": "Avery Example", "snippet": "Maybe"},
                              {"link": "ftp://example.org/b"}]}),
        ],
    );
    let found = found.unwrap();
    assert_eq!(found.len(), 1, "a hit that fails validation is skipped");
    assert_eq!(found[0].kind, Kind::Lead);
    assert_eq!(calls[0].param("q"), Some("\"Avery Example\""));
}

#[test]
fn companies_house_officer_keeps_service_region_and_stable_id() {
    let response = json!({"items": [
        {"title": "EXAMPLE, Avery Q", "links": {"self": "/officers/abc_123/appointments"},
         "appointment_count": 2, "date_of_birth": {"month": 5, "year": 1980},
         "address": {"locality": "Cardiff", "address_line_1": "Service address"}},
        {"title": "EXAMPLES, Avery", "links": {"self": "/officers/no/appointments"}},
        {"title": "EXAMPLE, Avery", "links": {"self": "https://evil.example/officers/x"}},
    ]});
    let (found, calls) = person(
        companies_house_officers,
        "Avery",
        "Example",
        "",
        &[("COMPANIES_HOUSE_API_KEY", "secret")],
        vec![response],
    );
    let found = found.unwrap();
    assert_eq!(ids(&found), ["abc_123"]);
    assert_eq!(found[0].fields["service_locality"], ["Cardiff"]);
    assert_eq!(found[0].fields["birth_month"], ["5"]);
    assert!(!found[0].fields.contains_key("address_line_1"));
    // Basic credentials: the key as user name and an empty password.
    assert_eq!(calls[0].header("Authorization"), Some("Basic c2VjcmV0Og=="));
    let (refused, _) = person(
        companies_house_officers,
        "Avery",
        "Example",
        "",
        &[],
        vec![],
    );
    assert_eq!(
        refused.unwrap_err().0,
        "set COMPANIES_HOUSE_API_KEY to use Companies House"
    );
}

#[test]
fn openalex_and_orcid_require_exact_name_and_keep_identifiers() {
    let alex = json!({"results": [
        {"id": "https://openalex.org/A123", "display_name": "Avery Q Example",
         "orcid": "https://orcid.org/0000-0001-2345-678X", "works_count": 9,
         "last_known_institutions": [{"display_name": "Cardiff University"}]},
        {"id": "https://openalex.org/A124", "display_name": "Alex Example"},
    ]});
    let (found, _) = person(openalex, "Avery", "Example", "", &[], vec![alex]);
    let found = found.unwrap();
    assert_eq!(ids(&found), ["A123"]);
    assert_eq!(found[0].fields["institution"], ["Cardiff University"]);
    assert_eq!(
        found[0].fields["orcid"],
        ["https://orcid.org/0000-0001-2345-678X"]
    );
    let token = json!({"access_token": "short-lived-test-token"});
    let rows = json!({"expanded-result": [
        {"orcid-id": "0000-0001-2345-678X", "given-names": "Avery Q",
         "family-names": "Example", "institution-name": ["Cardiff University"]},
        {"orcid-id": "0000-0001-2345-679X", "given-names": "Avery", "family-names": "Examples"},
    ]});
    let (found, calls) = person(
        orcid,
        "Avery",
        "Example",
        "",
        &[("ORCID_CLIENT_ID", "id"), ("ORCID_CLIENT_SECRET", "secret")],
        vec![token, rows],
    );
    let found = found.unwrap();
    assert_eq!(ids(&found), ["0000-0001-2345-678X"]);
    assert_eq!(calls[0].url, "https://orcid.org/oauth/token");
    assert_eq!(calls[0].param("scope"), Some("/read-public"));
    assert_eq!(
        calls[1].header("Authorization"),
        Some("Bearer short-lived-test-token")
    );
    assert_eq!(
        calls[1].param("q"),
        Some("given-names:\"Avery\" AND family-name:\"Example\"")
    );
    assert!(!format!("{found:?}").contains("short-lived-test-token"));
}

#[test]
fn orcid_quotes_names_so_they_cannot_change_the_query() {
    let (_, calls) = person(
        orcid,
        "Av\"ery",
        "Ex\\ample",
        "",
        &[("ORCID_ACCESS_TOKEN", "t")],
        vec![json!({"expanded-result": []})],
    );
    assert_eq!(
        calls[0].param("q"),
        Some(r#"given-names:"Av\"ery" AND family-name:"Ex\\ample""#)
    );
}

#[test]
fn crossref_articles_are_leads_and_openlibrary_authors_are_records() {
    let works = json!({"status": "ok", "message": {"items": [
        {"DOI": "10.5555/alpha", "author": [{"given": "Avery Q", "family": "Example"}],
         "title": ["Useful paper"], "published": {"date-parts": [[2024, 3, 1]]}},
        {"DOI": "10.5555/beta", "author": [{"given": "Avery", "family": "Examples"}]},
    ]}});
    let (found, _) = person(crossref, "Avery", "Example", "", &[], vec![works]);
    let found = found.unwrap();
    assert_eq!(ids(&found), ["10.5555/alpha"]);
    assert_eq!(found[0].kind, Kind::Lead);
    assert_eq!(found[0].fields["publication_year"], ["2024"]);
    assert_eq!(found[0].source_url, "https://doi.org/10.5555/alpha");
    let catalog = json!({"docs": [
        {"key": "OL321A", "name": "Avery Example", "work_count": 7},
        {"key": "OL322A", "name": "Avery Examples"},
    ]});
    let (found, _) = person(openlibrary, "Avery", "Example", "", &[], vec![catalog]);
    let found = found.unwrap();
    assert_eq!(ids(&found), ["OL321A"]);
    assert_eq!(
        found[0].kind,
        Kind::Record,
        "5.0 stores catalog authors as records"
    );
}

#[test]
fn public_profiles_and_cases_are_only_leads() {
    let sky = json!({"actors": [{"did": "did:plc:abcdef234567", "handle": "avery.bsky.social",
                                 "displayName": "Avery Example"},
                                {"did": "did:plc:invalid?", "displayName": "Avery Example"}]});
    let (found, _) = person(bluesky, "Avery", "Example", "", &[], vec![sky]);
    let found = found.unwrap();
    assert_eq!(
        found.iter().map(|item| item.kind).collect::<Vec<_>>(),
        [Kind::Lead]
    );
    assert_eq!(
        found[0].source_url,
        "https://bsky.app/profile/did:plc:abcdef234567"
    );
    let git = json!({"items": [{"login": "avery-example"}, {"login": "bad/username"}]});
    let (found, calls) = person(
        github,
        "Avery",
        "Example",
        "",
        &[("GITHUB_TOKEN", "gh")],
        vec![git],
    );
    assert_eq!(ids(&found.unwrap()), ["avery-example"]);
    assert_eq!(calls[0].param("q"), Some("\"Avery Example\" in:fullname"));
    assert_eq!(calls[0].header("Authorization"), Some("Bearer gh"));
    assert_eq!(
        calls[0].header("Accept"),
        Some("application/vnd.github+json")
    );
    let cases = json!({"results": [
        {"cluster_id": 234, "absolute_url": "/opinion/234/avery-v-example/",
         "caseName": "Avery v. Example", "court": "A court"},
        {"cluster_id": 235, "absolute_url": "https://other.example/case"},
    ]});
    let (found, calls) = person(
        courtlistener,
        "Avery",
        "Example",
        "",
        &[("COURTLISTENER_API_TOKEN", "test")],
        vec![cases],
    );
    let found = found.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::Lead);
    assert_eq!(
        found[0].source_url,
        "https://www.courtlistener.com/opinion/234/avery-v-example/"
    );
    assert_eq!(calls[0].header("Authorization"), Some("Token test"));
}

#[test]
fn companies_house_and_gleif_keep_distinct_organization_ids() {
    let companies = json!({"items": [
        {"title": "AVERY EXAMPLE LTD", "company_number": "01234567",
         "company_status": "active", "address": {"locality": "Cardiff"}},
        {"title": "AVERY EXAMPLE LIMITED", "company_number": "01234568"},
    ]});
    let (found, _) = named(
        companies_house_companies,
        "Avery Example Ltd",
        &[("COMPANIES_HOUSE_API_KEY", "secret")],
        vec![companies],
    );
    let found = found.unwrap();
    assert_eq!(ids(&found), ["01234567"]);
    assert_eq!(found[0].subject_type, SubjectType::Organization);
    assert_eq!(found[0].kind, Kind::Record);
    let lei = "529900T8BM49AURSDO55";
    let records = json!({"data": [
        {"id": lei, "attributes": {"entity": {"legalName": {"name": "Avery Example Ltd"},
          "legalAddress": {"city": "Cardiff", "country": "GB"}, "jurisdiction": "GB"},
          "registration": {"status": "ISSUED"}}},
        {"id": "NOT_A_LEI", "attributes": {"entity": {"legalName": {"name": "Avery Example Ltd"}}}},
    ]});
    let (found, calls) = named(gleif, "Avery Example Ltd", &[], vec![records]);
    let found = found.unwrap();
    assert_eq!(ids(&found), [lei]);
    assert_eq!(found[0].fields["legal_country"], ["GB"]);
    assert_eq!(found[0].fields["registration_status"], ["ISSUED"]);
    assert_eq!(
        calls[0].param("filter[entity.legalName]"),
        Some("Avery Example Ltd")
    );
}

#[test]
fn opencorporates_requires_key_and_namespaced_company_number() {
    let (refused, calls) = named(opencorporates, "Avery Ltd", &[], vec![]);
    assert!(refused.is_err());
    assert!(calls.is_empty());
    let response = json!({"results": {"companies": [
        {"company": {"name": "Avery Ltd", "jurisdiction_code": "gb",
                     "company_number": "123", "current_status": "Active"}},
        {"company": {"name": "Avery Ltd", "jurisdiction_code": "us_ca",
                     "company_number": "123"}},
    ]}});
    let (found, calls) = named(
        opencorporates,
        "Avery Ltd",
        &[("OPENCORPORATES_API_TOKEN", "test")],
        vec![response],
    );
    let found = found.unwrap();
    assert_eq!(ids(&found), ["gb/123", "us_ca/123"]);
    assert_eq!(calls[0].param("api_token"), Some("test"));
    assert!(
        calls[0].headers.is_empty(),
        "the token travels only in the query"
    );
    assert!(!format!("{found:?}").contains("\"test\""));
}

#[test]
fn domain_validation_and_authoritative_rdap_bootstrap() {
    assert_eq!(
        canonical_domain("BÜCHER.example.").unwrap(),
        "xn--bcher-kva.example"
    );
    for bad in [
        "https://example.org",
        "example.org:443",
        "*.example.org",
        "127.0.0.1",
    ] {
        assert!(canonical_domain(bad).is_err(), "{bad}");
    }
    let bootstrap = json!({"services": [
        [["uk"], ["https://rdap.example/uk/"]],
        [["co.uk"], ["https://rdap.example/co.uk/"]],
    ]});
    let answer = json!({"ldhName": "example.co.uk", "status": ["active"],
         "events": [{"eventAction": "registration", "eventDate": "2020-01-01T00:00:00Z"}],
         "nameservers": [{"ldhName": "ns1.example.net"}],
         "entities": [{"vcardArray": ["vcard", ["private contact"]]}]});
    let (found, calls) = named(rdap, "example.co.uk", &[], vec![bootstrap, answer]);
    let found = found.unwrap();
    assert_eq!(
        calls[1].url,
        "https://rdap.example/co.uk/domain/example.co.uk"
    );
    assert_eq!(found[0].subject_type, SubjectType::Domain);
    assert_eq!(
        found[0].kind,
        Kind::Record,
        "5.0 stores RDAP answers as records"
    );
    assert_eq!(found[0].fields["registered"], ["2020-01-01T00:00:00Z"]);
    assert!(!format!("{found:?}").contains("private contact"));
}

#[test]
fn wayback_and_urlscan_only_return_requested_domain_leads() {
    let archive = json!([
        ["timestamp", "original", "statuscode", "mimetype"],
        [
            "20250101000000",
            "https://example.org/about",
            "200",
            "text/html"
        ],
        [
            "20250102000000",
            "https://evil.example/about",
            "200",
            "text/html"
        ],
    ]);
    let (found, _) = named(wayback, "example.org", &[], vec![archive]);
    let found = found.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::Lead);
    assert_eq!(
        found[0].source_url,
        "https://web.archive.org/web/20250101000000/https://example.org/about"
    );
    let scans = json!({"results": [
        {"_id": "12345678-1234-1234-1234-123456789abc", "page": {"domain": "example.org"},
         "task": {"time": "2025-01-01T00:00:00Z"}},
        {"_id": "12345678-1234-1234-1234-123456789abd", "page": {"domain": "example.org.evil.test"}},
    ]});
    let (found, calls) = named(
        urlscan,
        "example.org",
        &[("URLSCAN_API_KEY", "k")],
        vec![scans],
    );
    assert_eq!(found.unwrap().len(), 1);
    assert_eq!(calls[0].header("api-key"), Some("k"));
}

// The 5.0 fixes, each pinned by a test.

#[test]
fn orcid_null_result_is_an_empty_search() {
    let (found, _) = person(
        orcid,
        "Avery",
        "Example",
        "",
        &[("ORCID_ACCESS_TOKEN", "t")],
        vec![json!({"expanded-result": null, "num-found": 0})],
    );
    assert!(found.unwrap().is_empty());
    let (broken, _) = person(
        orcid,
        "Avery",
        "Example",
        "",
        &[("ORCID_ACCESS_TOKEN", "t")],
        vec![json!({"expanded-result": "nonsense"})],
    );
    assert_eq!(
        broken.unwrap_err().0,
        "ORCID expanded search has no results list"
    );
}

#[test]
fn one_odd_hit_no_longer_sinks_a_whole_source() {
    let works = json!({"status": "ok", "message": {"items": [
        {"DOI": "10.5555/alpha", "author": [{"given": "Avery", "family": "Example"}],
         "published": {"date-parts": 5}},
    ]}});
    let (found, _) = person(crossref, "Avery", "Example", "", &[], vec![works]);
    let found = found.unwrap();
    assert_eq!(ids(&found), ["10.5555/alpha"]);
    assert!(!found[0].fields.contains_key("publication_year"));
    let archive = json!([
        ["timestamp", "original", "statuscode", "mimetype"],
        ["20250101000000", "https://[::1/odd", "200", "text/html"],
        [
            "20250102000000",
            "https://example.org/kept",
            "200",
            "text/html"
        ],
    ]);
    let (found, _) = named(wayback, "example.org", &[], vec![archive]);
    assert_eq!(found.unwrap().len(), 1);
    let entity = json!({"entities": {"Q42": {"labels": ["not", "an", "object"],
        "claims": {"P31": [{"mainsnak": {"datavalue": {"value": {"id": "Q5"}}}}]}}}});
    let (found, _) = person(
        wikidata,
        "Avery",
        "Example",
        "",
        &[],
        vec![
            json!({"search": [{"id": "Q42", "label": "Avery Example"}]}),
            entity,
        ],
    );
    assert!(found.unwrap().is_empty());
}

#[test]
fn registry_records_are_records() {
    let (found, _) = person(
        orcid,
        "Avery",
        "Example",
        "",
        &[("ORCID_ACCESS_TOKEN", "t")],
        vec![
            json!({"expanded-result": [{"orcid-id": "0000-0001-2345-678X",
            "given-names": "Avery", "family-names": "Example"}]}),
        ],
    );
    assert_eq!(found.unwrap()[0].kind, Kind::Record);
    let (found, _) = named(
        opencorporates,
        "Avery Ltd",
        &[("OPENCORPORATES_API_TOKEN", "t")],
        vec![
            json!({"results": {"companies": [{"company": {"name": "Avery Ltd",
            "jurisdiction_code": "gb", "company_number": "1"}}]}}),
        ],
    );
    assert_eq!(found.unwrap()[0].kind, Kind::Record);
}

#[test]
fn http_failures_are_source_errors_not_crashes() {
    let (failed, _) = person(npi, "Avery", "Example", "", &[], vec![json!([1, 2])]);
    assert_eq!(failed.unwrap_err().0, "expected a JSON object");
    let (failed, _) = named(wayback, "example.org", &[], vec![json!({"x": 1})]);
    assert_eq!(failed.unwrap_err().0, "expected a JSON array");
    let (failed, _) = named(rdap, "example.test", &[], vec![json!({"services": []})]);
    assert_eq!(
        failed.unwrap_err().0,
        "IANA has no HTTPS RDAP service for this domain suffix"
    );
}
