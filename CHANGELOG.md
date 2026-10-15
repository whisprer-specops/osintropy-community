# Changelog

## 5.0.0 — 2026-10-13

- Rewrote OSINTropy in Rust, in `v5.0.0/`: one self-contained program, `osintropy`, built with `cargo build --locked --release` and installed with `cargo install --locked --path .`. Commands, options, case files and reports are those of 4.1.1, and either version opens the other's case files. For the same run, the JSON, CSV, HTML and GraphML exports match 4.1.1's byte for byte, apart from the fixes below.
- Stored OpenAlex, ORCID, Open Library, GLEIF, Companies House company, OpenCorporates and RDAP hits as records, as the source catalogue always said. 4.1.1 stored them as imports.
- Kept searches going when a reply is odd. A malformed hit is skipped instead of failing its source or stopping with a Python traceback. A hit a source returns twice is stored once, where 4.1.1 failed the whole search. ORCID's reply for no matches is an empty run, not an error.
- Reported a single-source run's source diversity as 0.0, where 4.1.1 printed -0.0.
- Added SOCKS proxies to the `HTTPS_PROXY` and `NO_PROXY` support, so searches can go through Tor with `socks5h://127.0.0.1:9050`. A proxy address the client cannot use makes the search fail; it never goes direct. Windows' own proxy settings are no longer read.
- Converted international domain names with UTS #46, the rules browsers use, instead of IDNA 2003.
- Added `help COMMAND`, `COMMAND --help`, and `version`, `--version` or `-V`. `--db` may go anywhere on the line. Network errors say what failed: DNS lookup, connection, proxy, timeout or redirect.
- Added 75 offline tests with synthetic data, and `scripts/check.ps1` and `scripts/check.sh` to run them with the formatting check, strict lints, a release build and the demo. Every code file opens with whispr.dev's copyright line and the GPL-3.0-only SPDX tag, and a test enforces it.
- [MIGRATION.md](MIGRATION.md) lists every deliberate difference from 4.1.1.
- Ready-built downloads for Windows, macOS (one program for Apple silicon and Intel) and Linux (x86_64 and ARM64), from the public repository [whisprer-specops/osintropy-community](https://github.com/whisprer-specops/osintropy-community). Each system has an installer that shows the licence and asks you to accept it, then asks for a folder and changes nothing else, beside a plain archive. GitHub Actions builds and checks every release on all three systems (`.github/workflows/release.yml`, with `packaging/`). Requests name that repository in their user agent.

## 4.1.1 — 2026-09-29

- Expanded the offline demo to cover all three subject types in one case: ten synthetic observations, two unverified leads, two manual links, an unlinked same-name review queue, and a visible field variation.
- Added end-to-end checks for the expanded demo, including an assertion that it never creates an HTTP client.

## 4.1.0 — 2026-09-29

- Added explicit `search-org` and `domain` commands with typed organization and domain observations. Existing case databases gain a `subject_type` column in place; historical observations remain people.
- Added eight person searches (Companies House officers, OpenAlex, ORCID, Crossref, Bluesky, GitHub, CourtListener, Open Library), three organization registries (GLEIF, Companies House, OpenCorporates), and three domain sources (RDAP, Wayback, urlscan). Only NPI, Wikidata, GLEIF, and RDAP run by default in their respective commands.
- Kept profile, publication, case, web, archive, and scan search results as unverified leads. Names and subject types do not create identity links. Record IDs remain source-specific.
- Added a source catalog with credentials, data boundaries, matching rules, and links to API documentation. Expanded synthetic contract tests for each connector, domain filtering, existing database migration, and CLI partial failures.
- Closed SQLite connections before replacing a backup and after reading a legacy database, fixing Windows file-handle errors during backup and temporary directory cleanup.

## 4.0.0 — 2026-09-29

- Replaced the inconsistent runtime with a standard-library case notebook.
- Added exact-name NPPES and Wikidata connectors and an explicit existing-customer Google CSE lead connector.
- Added observation provenance, SQLite transactions, manual reviewed links, local import, legacy migration, retention deletion, and atomic JSON/CSV/HTML/GraphML exports.
- Reframed entropy as source distribution, with no unsupported truth or identity score.
- Added synthetic end-to-end and mocked API contract tests.
- Preserved source prototypes and all reference material as non-executable history.

