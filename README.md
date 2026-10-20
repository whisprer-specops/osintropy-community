# OSINTropy 5.0.0

OSINTropy is a local case notebook for public-source observations. It offers name searches across person and organisation directories, domain lookups, local imports, source provenance in SQLite, manual review, and JSON, CSV, HTML and GraphML exports. It makes no network requests during `demo`, imports, reports or tests.

Version 5.0.0 is OSINTropy rewritten in Rust. It is one self-contained program, `osintropy`, with the same commands, case files and reports as the Python 4.1.1 release. Copyright © 2026 whispr.dev, free software under GPL-3.0-only.

**An observation is a source hit, not a resolved identity.** People, organisations and domains have separate subject types. Identical names stay separate even within one type. Search hits for accounts, articles, cases or archived pages are unverified leads and cannot be linked as records. Source diversity is a Shannon entropy measurement of the *mix of sources*, never a probability that a claim is true.

**New here?** Start with the [quickstart guide](docs/quickstart/QUICKSTART.md): a plain-English tour with pictures, from installing to your first results. It's also a [PDF](docs/quickstart/osintropy-quickstart.pdf) to keep or print.

## What is where

| Folder | Contents |
| --- | --- |
| `docs/quickstart/` | The quickstart guide, in Markdown and as a PDF |
| `v5.0.0/` | This release: the Rust source (`src/`, `tests/`, `scripts/`, `Cargo.toml`, `Cargo.lock`) and the source catalogue, [SOURCES.md](v5.0.0/SOURCES.md) |
| `packaging/`, `.github/workflows/` | How releases are built, packed and published for Windows, macOS and Linux |

## Download

Ready-built programs for Windows, macOS and Linux are on the [Releases page](https://github.com/whisprer-specops/osintropy-community/releases/latest). These links always give the newest release:

| System | Installer | Plain archive |
| --- | --- | --- |
| Windows 10 or 11, 64-bit | [`osintropy-windows-x86_64-setup.exe`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-windows-x86_64-setup.exe) | [`osintropy-windows-x86_64.zip`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-windows-x86_64.zip) |
| macOS 11 or newer, Apple silicon and Intel | [`osintropy-macos-universal.run`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-macos-universal.run) | [`osintropy-macos-universal.tar.gz`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-macos-universal.tar.gz) |
| Linux, x86_64 | [`osintropy-linux-x86_64.run`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-linux-x86_64.run) | [`osintropy-linux-x86_64.tar.gz`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-linux-x86_64.tar.gz) |
| Linux, ARM64 | [`osintropy-linux-aarch64.run`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-linux-aarch64.run) | [`osintropy-linux-aarch64.tar.gz`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/osintropy-linux-aarch64.tar.gz) |

Each installer shows the licence and asks you to accept it, then asks which folder to install into. It changes nothing else: no registry entries, no PATH changes and no uninstaller. To remove OSINTropy, delete its folder. The plain archives hold the same files. Every release lists each file's SHA-256 in [`SHA256SUMS.txt`](https://github.com/whisprer-specops/osintropy-community/releases/latest/download/SHA256SUMS.txt).

- **Windows:** run the `-setup.exe`. It isn't code-signed yet, so Windows may say "Windows protected your PC". That's expected: click **More info**, then **Run anyway**.
- **macOS and Linux:** open a terminal and run the installer with `sh`, for example `sh ~/Downloads/osintropy-macos-universal.run`. On macOS, running it with `sh` avoids the warning about programs that aren't notarised yet. Page through the licence with Space, then type `y` to accept.

Then, in the folder you chose, `.\osintropy.exe --help` on Windows, or `./osintropy --help` on macOS and Linux, lists every option. To build it yourself instead, read on.

## Build on Windows 11

Install [Rust through rustup](https://rust-lang.org/tools/install/) and the Visual Studio 2022 Build Tools with **Desktop development with C++** and a Windows SDK. Use the MSVC Rust host. The source pins Rust **1.97.0** in `rust-toolchain.toml`, and rustup downloads it on the first build. The code keeps to Rust 1.87, the declared minimum. `Cargo.lock` pins every dependency. SQLite and the cryptography code are compiled from bundled sources, so nothing else needs installing.

In PowerShell, from the repository folder:

```powershell
cd .\v5.0.0
cargo build --locked --release
.\target\release\osintropy.exe --version
.\target\release\osintropy.exe --db .\target\demo\demo-case.db demo --out .\target\demo\demo.html
Invoke-Item .\target\demo\demo.html
```

Then install the command, so `osintropy` works in any folder:

```powershell
cargo install --locked --path .
osintropy --help
```

`cargo install` copies `osintropy.exe` into `%USERPROFILE%\.cargo\bin`, which rustup put on your `PATH`. Run the same command again after updating the source.


## Build on Linux or macOS

Install Rust through rustup and a C compiler: `build-essential` on Ubuntu, or the Xcode command line tools on macOS.

```bash
cd v5.0.0
cargo build --locked --release
./target/release/osintropy --db ./target/demo/demo-case.db demo --out ./target/demo/demo.html
cargo install --locked --path .
```

## The demo

`demo.html` works offline in any browser. The case holds ten invented observations spanning people, organisations and a domain. It shows two manual record links, two unverified leads, an unresolved same-name review pair, and a city variation in linked company records. Every source name begins with `synthetic_`. No API is contacted, no real person or company is described, and no credentials are needed. The demo writes into `target\demo`, which git ignores, so it stays apart from your cases. Pass the printed run ID to `show` or `export` to see the JSON, CSV or GraphML versions.

## Search public APIs

```powershell
osintropy search Ada Lovelace --out .\ada.json
osintropy search Avery Example --location "Sampleton, CA" --source npi
osintropy search "Ada" "Lovelace" --source openalex --source crossref
osintropy search-org "Example Holdings Ltd" --out .\company.json
osintropy domain example.org --source rdap --source wayback --source urlscan --out .\domain.json
osintropy sources
```

These store their runs in your default case file (see [Data and safety](#data-and-safety)). The defaults are `npi` and `wikidata` for people, `gleif` for organisations, and `rdap` for domains. Other sources run only when selected with `--source`; repeat it to query more than one. A search asks for at most the first 10–20 hits per source and stores only matches that pass the connector's checks. It does not page through whole databases.

| Command | Source | Observation and access |
| --- | --- | --- |
| `search` | `npi`, `wikidata` | Individual U.S. provider and Wikidata human records; no key |
| `search` | `companies_house_officers` | UK officer index records; `COMPANIES_HOUSE_API_KEY` |
| `search` | `openalex`, `orcid`, `openlibrary` | Scholarly author and catalogue records; OpenAlex key optional, ORCID public OAuth token or client credentials, Open Library no key |
| `search` | `crossref`, `bluesky`, `github`, `courtlistener`, `google_cse` | Article, public account, case-law and web search **leads**; CourtListener and Google require credentials; GitHub token optional; others no key |
| `search-org` | `gleif`, `companies_house_companies`, `opencorporates` | LEI and company register records; GLEIF no key, Companies House and OpenCorporates require tokens |
| `domain` | `rdap`, `wayback`, `urlscan` | Authoritative RDAP record and archived-page or scan **leads**; no key required, urlscan key optional |

For each connector's fields, matching rules, endpoint, credential variable and limitations, see [SOURCES.md](v5.0.0/SOURCES.md). Optional API services may impose account, quota or licence terms. Google CSE is available to existing API customers and has announced a January 1, 2027 end date. Set credentials in the environment, not in code or command arguments. For example:

```powershell
$env:GOOGLE_CSE_API_KEY = Read-Host 'Existing Google API key'
$env:GOOGLE_CSE_CX = Read-Host 'Existing search engine ID'
osintropy search Ada Lovelace --source google_cse
```

The program does not create credentials or include any secret. These variables last only for the current PowerShell window. OpenCorporates requires its `api_token` in the HTTPS query string by its published contract; OSINTropy does not echo request URLs or tokens in errors or reports.

Requests use HTTPS, a descriptive user agent, 12 seconds for each whole request, one-second per-host pacing, no automatic retries or redirects, and a 2 MB response limit. API failures are stored alongside partial results and cause exit code 3. A successful empty search is recorded as an empty completed run. No source is queried unless `search`, `search-org` or `domain` is invoked. `--location` applies to NPI only; the other person connectors ignore it.

To send searches through a proxy, set `HTTPS_PROXY` before searching. For a local Tor client:

```powershell
$env:HTTPS_PROXY = 'socks5h://127.0.0.1:9050'
osintropy search Ada Lovelace
```

`http://`, `socks4://`, `socks4a://`, `socks5://` and `socks5h://` addresses work. An `https://` address is used as an ordinary HTTP proxy, as 4.1.1 used it. With SOCKS5 the proxy also looks up the host names. Hosts listed in `NO_PROXY` are reached directly. Any other proxy address makes each search fail with exit code 3; OSINTropy never goes around a proxy you set. Windows' own proxy settings are not read.

## Import your own observations

JSON accepts either an array or an object with `query` and `records` (or `observations`):

```json
{
  "query": "Example case",
  "records": [
    {
      "source": "local_register",
      "source_id": "entry-17",
      "name": "Avery Example",
      "subject_type": "person",
      "source_url": "https://example.org/entry/17",
      "kind": "record",
      "fields": {"occupation": ["Engineer"], "city": ["Sampleton"]}
    }
  ]
}
```

The URL above illustrates the schema; replace it with a URL you actually verified, or omit `source_url` for offline material. `subject_type` is `person` (the default), `organization` or `domain`. `source_id` is stable within a source; if absent it is derived from the observation content. The file must be standard JSON, so `NaN` and `Infinity` are refused. CSV needs `source,name` columns and may include `source_id,source_url,kind,subject_type,collected_at,fields_json`; any other columns become fields. A CSV row is one observation. Files are capped at 20 MB and 10,000 observations per import.

```powershell
osintropy import-file .\my-observations.json --out .\imported.json
osintropy import-legacy .\old-people-search.db
```

The legacy importer reads the original `person_records` SQLite table without changing it. Old records were already merged by previous code, so each becomes **one unverified imported observation** with a provenance warning. It cannot reconstruct field-level evidence that the old schema discarded.

## Review and export

```powershell
osintropy list
osintropy show RUN_ID
osintropy inspect 1
osintropy link 1 2 --reason "Compared source identifiers and checked both records"
osintropy unlink 1 2
osintropy export RUN_ID --format html --out .\case.html
osintropy export RUN_ID --format graphml --out .\case.graphml
osintropy export RUN_ID --format csv --out .\case.csv
osintropy backup --out .\cases-backup.db
osintropy delete RUN_ID
osintropy prune --days 30
```

Use an actual run ID from `list` in place of `RUN_ID`. Observation IDs appear in `show` and the HTML cards. Linking requires a reason, matching subject types, and two records rather than unverified leads. A link is your manual assertion; OSINTropy never makes it from matching names. The report offers a review queue for exact shared names within the same subject type and shows differing field values in linked observations. Deleting a run also deletes its observations and their links. `prune` removes runs older than the given number of days. `backup` uses SQLite's backup API, so the copy is consistent, and closes both files before putting the copy in place. Exports are written atomically; CSV values that could trigger spreadsheet formulas are escaped.

The HTML report is self-contained and works offline. Its evidence map previews the first 80 observations; GraphML exports the complete source/observation graph. JSON contains the complete run, errors, reviewed links and analysis. CSV is a flat **export** of field values; for round-trip imports use JSON or the documented one-row-per-observation CSV format.

`osintropy help` lists every command, and `osintropy help export` (or `osintropy export --help`) explains one. `--db FILE` may go anywhere on the line. Exit codes are **0** for success, **2** for an error or a mistyped command, and **3** for a search in which a source failed; that run is still saved.

## Data and safety

The default case file is `%LOCALAPPDATA%\osintropy\cases.db` on Windows, or `${XDG_DATA_HOME:-~/.local/share}/osintropy/cases.db` on Linux and macOS. Use `--db` to keep a case elsewhere, such as on an encrypted drive:

```powershell
osintropy --db E:\cases\client-a.db search Avery Example
```

New case files and exports are readable only by their owner on Linux and macOS; on Windows, access follows the folder's permissions. Do not place case files in a public web folder, a git repository or a synced folder without considering the personal data they hold. This repository's `.gitignore` ignores `*.db` files for that reason.

This is an investigator's notebook, not identity verification, a background-check service or a risk classifier. Information in public directories can be stale or wrong. Follow source terms and data protection obligations; keep only data relevant to an authorised purpose. See [SECURITY.md](SECURITY.md) for the network and data details.

## Coming from 4.1.1

Commands, options, case files and reports are the same. 5.0.0 opens 4.1.1 case files as they are, and 4.1.1 can open files 5.0.0 writes. For the same run, the JSON, CSV, HTML and GraphML exports match 4.1.1's byte for byte, apart from the fixes listed in [MIGRATION.md](MIGRATION.md). Replace `py -3.14 -m osintropy` with `osintropy`.

## Check a build

From `v5.0.0`, run every check at once:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check.ps1
```

It checks formatting, runs the tests, runs the strict lints, builds the release program and makes the offline demo. On Linux or macOS, `bash scripts/check.sh` does the same. The tests use synthetic API responses and contact no outside service. Live responses cannot be guaranteed by an offline test: APIs may change their schemas or availability. See [CHANGELOG.md](CHANGELOG.md) for what changed and [CONTRIBUTING.md](CONTRIBUTING.md) before sending changes.

## License

Copyright © 2026 whispr.dev.

OSINTropy is free software: you can redistribute it and change it under the terms of the GNU General Public License, version 3 only (GPL-3.0-only). It comes with NO WARRANTY, to the extent permitted by law. The full terms are in `LICENSE`. Every code file opens with its copyright line and `SPDX-License-Identifier: GPL-3.0-only`.

The GPL covers the program. It gives no right to use the OSINTropy name, which stays with whispr.dev, as section 7 of the licence allows.
