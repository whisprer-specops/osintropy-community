# Source catalogue and evidence rules

OSINTropy queries published programmatic interfaces only when you explicitly select a search command. Each source returns a bounded first page. Its stable source ID and a source URL are stored with the observation. A record describes an entry in a source's directory, not a proven real-world identity. A lead is a weaker search result that cannot be linked as a record in the case notebook. An exact name is a local first-and-last or legal-name filter; middle names are permitted for people. Identical names may refer to different subjects.

Rows marked **Lead.** below are stored as unverified leads. Every other source is stored as a record. (Version 4.1.1 stored OpenAlex, ORCID, Open Library, GLEIF, Companies House company, OpenCorporates and RDAP hits as imports by mistake; 5.0.0 stores them as records.)

The source adapters were checked against published API contracts and tested with synthetic responses. A live query depends on the upstream service, your account and quota, and its current data. If a response breaks a contract, the run records the source error and retains results from other sources.

## People: `search FIRST LAST`

The default sources are `npi` and `wikidata`. Other sources require `--source NAME`; repeat that flag to combine sources. `--location "City, ST"` filters NPI practice locations only.

| Name | Credential | Stored data | Evidence rule and caveat |
| --- | --- | --- | --- |
| `npi` | None | NPI, individual provider name, public practice address and phone | Exact name, `NPI-1`, practice location only. NPPES may contain stale practice details. [NPPES API](https://npiregistry.cms.hhs.gov/api-page) |
| `wikidata` | None | QID, English description, occupation and employer QIDs | Exact English label and `instance of human`; a QID is not a personal identity assertion. [Wikibase API](https://www.mediawiki.org/wiki/Wikibase/API) |
| `companies_house_officers` | `COMPANIES_HOUSE_API_KEY` | Officer search ID, appointment count, service locality/region/country, partial birth month/year when returned | Exact person name, validated officer appointment path; full address lines are omitted. Search results can contain distinct officers with the same name. [Companies House search API](https://developer.company-information.service.gov.uk/), [officer search](https://developer-specs.company-information.service.gov.uk/companies-house-public-data-api/reference/search/search-officers) |
| `openalex` | Optional `OPENALEX_API_KEY` | OpenAlex author ID, work count, last known institutions, ORCID when present | Exact display name; author profiles are catalogue records and may need disambiguation. [OpenAlex author API](https://docs.openalex.org/api-entities/authors/search-authors) |
| `orcid` | `ORCID_ACCESS_TOKEN`, or both `ORCID_CLIENT_ID` and `ORCID_CLIENT_SECRET` | ORCID iD, public name and institutions | Public OAuth scope `/read-public`; exact name in expanded search. Private data is not requested. An ORCID search hit still needs review. A reply with no result list is an empty run. [ORCID public API](https://info.orcid.org/documentation/features/public-api/), [expanded search](https://info.orcid.org/documentation/integration-guide/searching-the-orcid-registry/) |
| `openlibrary` | None | Open Library author ID, catalogue name, work count and top work | Exact catalogue author name. Catalogue entries can be conflated or duplicated. [Author search API](https://openlibrary.org/dev/docs/api/search) |
| `crossref` | None; optional `OSINTROPY_CONTACT_EMAIL` | DOI, matching author string, publication title, publisher and year | **Lead.** A matching author string in publication metadata does not identify the person. [REST API](https://www.crossref.org/documentation/retrieve-metadata/rest-api/) |
| `bluesky` | None | DID, handle and display name | **Lead.** Exact public display name is weak identity evidence. The DID identifies the account. [Actor search](https://docs.bsky.app/docs/api/app-bsky-actor-search-actors) |
| `github` | Optional `GITHUB_TOKEN` | Username and requested full-name search context | **Lead.** GitHub user search does not return a verified full name in this response; results are candidates only. [Search users API](https://docs.github.com/en/rest/search/search#search-users) |
| `courtlistener` | `COURTLISTENER_API_TOKEN` | Opinion cluster ID, case name, court, filing date and docket number when returned | **Lead.** An exact-name phrase in a case-law search does not prove a subject's role or identity. [Search API](https://www.courtlistener.com/help/api/rest/), [API authentication](https://www.courtlistener.com/help/api/rest/) |
| `google_cse` | `GOOGLE_CSE_API_KEY` and `GOOGLE_CSE_CX` | Search page title, snippet and URL | **Lead.** Existing Custom Search JSON API customers only; Google announced discontinuation on January 1, 2027. [API overview](https://developers.google.com/custom-search/v1/overview) |

## Organisations: `search-org "LEGAL NAME"`

The default source is `gleif`. Names are compared after whitespace and case normalisation. No fuzzy result is automatically accepted. Multiple records with the same legal name remain separate.

| Name | Credential | Stored data | Evidence rule and caveat |
| --- | --- | --- | --- |
| `gleif` | None | LEI, status, jurisdiction, legal address city/region/country and direct parent LEI when returned | Exact legal name and valid LEI shape; LEI coverage is limited to registered entities. [GLEIF API](https://www.gleif.org/en/lei-data/gleif-api) |
| `companies_house_companies` | `COMPANIES_HOUSE_API_KEY` | UK company number, status, creation date, type and registered locality/region/country | Exact current title and valid company number; historical names are not automatically accepted. [Companies House search API](https://developer-specs.company-information.service.gov.uk/companies-house-public-data-api/reference/search/search-companies) |
| `opencorporates` | `OPENCORPORATES_API_TOKEN` | Jurisdiction and company number, status, incorporation date, registered address | Exact current name; source ID includes jurisdiction. The published API requires `api_token` as an HTTPS query parameter. Check your account's licence and quota. [API reference](https://api.opencorporates.com/documentation/API-Reference) |

## Domains: `domain example.org`

The default source is `rdap`. Input is a domain name, with no URL, wildcard, or port; IP addresses are refused. International names are converted to their `xn--` form with the UTS #46 rules browsers use, so `straße.de` is looked up as `xn--strae-oqa.de`. These connectors query registries and archives; they do not open the target website. Domain observations never enter a person or organisation name review group.

| Name | Credential | Stored data | Evidence rule and caveat |
| --- | --- | --- | --- |
| `rdap` | None | Domain name, status, registration/expiration/change events and nameservers | IANA bootstrap picks the most specific HTTPS registry endpoint. The returned `ldhName` must match the requested domain. Contact entities and vCards are omitted. Some suffixes have no HTTPS RDAP endpoint. [IANA bootstrap](https://www.iana.org/assignments/rdap-dns/rdap-dns.xhtml), [RDAP protocol](https://www.rfc-editor.org/rfc/rfc9083) |
| `wayback` | None | Archived URL, capture time and MIME type | **Lead.** CDX results are filtered to the domain or its subdomains. An archive capture is historical page evidence, not ownership evidence. [CDX server](https://github.com/internetarchive/wayback/tree/master/wayback-cdx-server) |
| `urlscan` | Optional `URLSCAN_API_KEY` | Scan ID, scanned domain and URL, scan time | **Lead.** Only results whose `page.domain` is the domain or a subdomain are kept; a scan does not establish ownership. [Search API](https://docs.urlscan.io/apis/urlscan-openapi/search) |

## Handling credentials and failures

Set credentials as environment variables in your shell. Do not put them in case files. The source URL stored in a report points to the public record or lead, never an authenticated API request. Errors report HTTP status or a short network or schema message without echoing request URLs, tokens, or response bodies. OpenCorporates' required query token can still appear in external request logs controlled by that service or your network provider.

The HTTP client permits HTTPS only, rejects redirects, limits bodies to 2 MB, allows 12 seconds for each whole request, and paces calls per host. It uses a proxy only when `HTTPS_PROXY` (or `https_proxy`) is set, and skips it for hosts listed in `NO_PROXY`. One source failure produces a `partial` run when another source supplied observations, or a `failed` run otherwise. Exit code 3 signals source errors; the saved run contains them. An empty, error-free result is a `complete` run with zero observations. No connector attempts bulk pagination or implicit follow-up on a result URL.

A reply without the list a connector expects is a source error. Within a good list, a hit that lacks a required value, or has one of the wrong shape, is skipped and the other hits are kept. An optional value of the wrong shape, such as a list where text belongs, is left out. A hit that a source returns twice in one search is stored once.
