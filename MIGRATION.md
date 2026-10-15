# Migration

## From 4.1.1 (Python) to 5.0.0 (Rust)

5.0.0 is the same notebook written in Rust. The commands, options, case-file schema, import formats, report formats, credential variables, default case-file location and exit codes are unchanged. 5.0.0 opens 4.1.1 case files as they are, and 4.1.1 can open files that 5.0.0 writes. As before any upgrade, back up a valuable case first.

To switch, build and install 5.0.0 as the README describes, then type `osintropy` where you typed `py -3.14 -m osintropy`. If the 4.1.1 wheel is installed, `py -m pip uninstall osintropy` removes its command. The Python release stays in `v4.1.1/` for reference; nothing in 5.0.0 runs it.

### Where the Python code went

| 4.1.1, in `v4.1.1/osintropy/` | 5.0.0, in `v5.0.0/src/` |
| --- | --- |
| `cli.py`, `__main__.py` | `cli.rs`, `main.rs` |
| `models.py` | `models.rs` |
| `storage.py` | `storage.rs` |
| `analysis.py` | `analysis.rs` |
| `report.py` | `report.rs` |
| `importers.py` | `importers.rs`, `csvread.rs` |
| `demo.py` | `demo.rs` |
| `source_common.py` | `http.rs`, `sources/mod.rs` |
| `sources.py` | `sources/person.rs` (NPI, Wikidata, Google CSE), `sources/mod.rs` (the catalogues) |
| `person_sources.py` | `sources/person.rs` |
| `organization_sources.py` | `sources/organization.rs` |
| `domain_sources.py` | `sources/domain.rs` |
| Python's own JSON, number printing, case folding, `urllib.parse`, `datetime.fromisoformat` and `csv` rules | `pyjson.rs`, `pyfmt.rs`, `casefold_data.rs`, `urlparse.rs`, `isotime.rs`, `csvread.rs` |
| `v4.1.1/tests/test_pipeline.py` | `v5.0.0/tests/pipeline_contracts.rs`, `v5.0.0/tests/cli_contracts.rs` |
| `v4.1.1/tests/test_expanded_sources.py` | `v5.0.0/tests/source_contracts.rs` |

### What is deliberately different

Search results:

- **Seven sources now store records.** OpenAlex, ORCID, Open Library, GLEIF, Companies House company, OpenCorporates and RDAP hits are stored as `record`, as the source catalogue always described them. 4.1.1 stored them as `import`. Reports label them `record`; nothing else changes, because imports could already be linked.
- **An odd reply no longer sinks a source.** A hit that lacks a required value, or has one of the wrong shape, is skipped and the source's other hits are kept. 4.1.1 could fail the whole source or stop with a Python traceback: for example on a Crossref date that was not a list of numbers, a Wikidata label that was not text, an NPI address that was not an object, or one unusual Wayback URL. An optional value of the wrong shape, such as a list where text belongs, is left out rather than stored as Python's printout of it.
- **A repeated hit is stored once.** When a source returns the same record twice in one search, 5.0.0 keeps one copy. 4.1.1 stopped with a database error and saved nothing from the search.
- **ORCID with no matches is an empty run.** ORCID answers such a search with `"expanded-result": null`, which 4.1.1 recorded as a source error.
- **Identifiers must use the digits 0 to 9.** Python's `isdigit` and its regular-expression `\d` also accept other scripts' digits, such as Arabic-Indic ones, so 4.1.1 could store a malformed NPI number, Wikidata ID, ORCID iD or similar.
- **International domain names follow UTS #46**, as browsers do: `straße.de` is looked up as `xn--strae-oqa.de`. 4.1.1 used Python's IDNA 2003 codec, which turned it into `strasse.de`, a different domain. A name UTS #46 rejects is refused with "invalid internationalized domain name".
- **A one-source run has a source diversity of 0.0.** 4.1.1 printed -0.0 in its JSON and HTML reports.

Imports:

- **JSON imports must be standard JSON.** `NaN`, `Infinity` and `-Infinity` are refused, where Python accepted them. The message for a broken file is worded differently but still gives the line and column.
- **Malformed legacy rows are refused.** A `person_records` row that is not a well-formed old record stops `import-legacy` with "invalid legacy record". 4.1.1 could show a raw parser message, stop with a traceback, or import a garbled name.
- **Times are read as Python 3.14 reads them**, the version the 4.1.1 README used. Under Python 3.13 or older, 4.1.1 rejected the time `24:00`, which now means midnight at the start of the next day, and accepted a few malformed times that 3.14 refuses.

The command line:

- **`--db` may go anywhere on the line**, not only before the command.
- **New:** `help`, `help COMMAND`, `COMMAND --help`, and `version`, `--version` or `-V`. Running `osintropy` alone prints the whole help and exits with code 2.
- **Gone:** abbreviated options, such as `--sou` for `--source`. Type options in full.
- **Clearer messages.** "run X not found" no longer wraps the ID in stray quotes. A file error names the path and the system's reason, for example `missing.json: No such file or directory (os error 2)`. Network errors say what failed: `DNS lookup failed`, `connection failed`, `proxy failed`, `timeout`, `redirect refused`, `malformed response` or `connection interrupted`.
- **Printed paths are as you typed them**, with `~` expanded. 4.1.1 printed Python's tidied form, which on Windows turned `/` into `\`.

The network:

- **Proxies.** Like 4.1.1, 5.0.0 reads `HTTPS_PROXY` (or `https_proxy`) and `NO_PROXY` (or `no_proxy`), and treats an `http://` or `https://` address as an HTTP proxy. It adds SOCKS proxies, including `socks5h://` for Tor; 4.1.1 spoke HTTP to a SOCKS port, which fails. An address with any other scheme makes the search fail rather than go direct. With no variable set, 4.1.1 also used the Windows proxy settings; 5.0.0 does not read them.
- **Certificates.** HTTPS certificates are checked against the Mozilla root list built into the program, not the Windows certificate store. A network that inspects HTTPS with its own root certificate is refused.
- **Timeout.** The 12-second limit covers each whole request. In 4.1.1 it applied to each network read, so a slow trickle could run longer.
- **Compression.** Replies may arrive gzip-compressed; the 2 MB limit applies to the unpacked body.
- **User agent.** Requests identify as `OSINTropy/5.0.0 (+https://github.com/whisprer-specops/osintropy-community)`.

Text:

- **Case folding** for exact-name matching uses Python 3.13's Unicode 15.1 tables, with Rust's own tables for characters added since.

### How the port was checked

During the port, each part was compared with the 4.1.1 code on generated inputs. These covered about 200,000 floating-point numbers, every Unicode code point for case folding, 80,000 URLs, 60,000 import records, 40,000 text values, 30,000 percent-encodings, 20,000 CSV files and 5,100 recorded connector replies. A 61-step session of every command produced byte-identical exports. Apart from the wording of error messages, the only differences found are the ones listed above.

