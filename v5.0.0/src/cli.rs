// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! The command line. Commands never start hidden lookups: only `search`, `search-org` and
//! `domain` contact a network service, and only the sources asked for (or the defaults).
//!
//! Exit codes: 0 success, 2 an error (nothing was changed by a failed command), 3 a search
//! in which at least one source failed (the run, with its errors, is still saved).

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::demo::create_demo;
use crate::error::{Error, Result};
use crate::http::{Http, HttpClient};
use crate::importers::{load_file, load_legacy_db};
use crate::models::{clean_str, Observation};
use crate::paths;
use crate::pyfmt::parse_int;
use crate::pyjson::{Json, Style};
use crate::report::{render, Format};
use crate::sources::{
    self, domain::canonical_domain, Catalog, Context, Env, NameSource, PersonQuery, PersonSource,
    ProcessEnv,
};
use crate::storage::Store;

/// The GNU-style notice under the version line.
pub const NOTICE: &str = "\
Copyright (C) 2026 whispr.dev
License GPL-3.0-only: GNU General Public License, version 3 <https://www.gnu.org/licenses/gpl-3.0.html>
This is free software: you are free to change and redistribute it.
There is NO WARRANTY, to the extent permitted by law.";

/// The help screen.
pub const USAGE: &str = "\
osintropy - a local, provenance-first case notebook for public-source observations
Copyright (C) 2026 whispr.dev. Free software under GPL-3.0-only, with NO WARRANTY.

USAGE:
  osintropy [--db FILE] <command> [arguments]

  --db FILE   the case file. Default: %LOCALAPPDATA%\\osintropy\\cases.db on Windows,
              ${XDG_DATA_HOME:-~/.local/share}/osintropy/cases.db elsewhere.

SEARCH (the only commands that use the network):
  search FIRST LAST [--location \"City, ST\"] [--source NAME]... [--out FILE.json]
  search-org \"LEGAL NAME\" [--source NAME]... [--out FILE.json]
  domain example.org [--source NAME]... [--out FILE.json]
  sources                list every source, its default and its credentials

LOCAL DATA:
  import-file FILE.json|FILE.csv [--out FILE.json]
  import-legacy OLD.db [--out FILE.json]
  demo [--out FILE.html]  a synthetic case, made offline

REVIEW AND EXPORT:
  list
  show RUN_ID
  inspect OBSERVATION_ID
  link FIRST_ID SECOND_ID --reason \"what you checked\"
  unlink FIRST_ID SECOND_ID
  export RUN_ID --out FILE [--format json|csv|html|graphml]   (html by default)
  backup --out FILE.db
  delete RUN_ID
  prune --days N

  help [COMMAND] | --help | -h
  version | --version | -V

An observation is a source hit, not a resolved identity: matching names never link
records. Only `link` does, with a reason you give.
Exit codes: 0 success, 2 error, 3 a source failed (the run is still saved).";

const SOURCES_TEXT: &str = "\
PERSON SEARCH (first and last name)
npi         NPPES individual provider directory (default)
wikidata    Notable human entities with exact names (default)
google_cse  Search leads, existing API customers only; credentials required
companies_house_officers  UK officer records; COMPANIES_HOUSE_API_KEY
openalex    Scholarly author records; optional OPENALEX_API_KEY
orcid       Public researcher IDs; ORCID_ACCESS_TOKEN or client credentials
crossref    Publication author leads; no key
bluesky     Public profile leads; no key
github      Public user leads; optional GITHUB_TOKEN
courtlistener  US case-law leads; COURTLISTENER_API_TOKEN
openlibrary   Public catalog author records; no key
ORGANIZATION SEARCH (search-org)
gleif       Global LEI records (default); no key
companies_house_companies  UK companies; COMPANIES_HOUSE_API_KEY
opencorporates  Multi-jurisdiction company records; OPENCORPORATES_API_TOKEN
DOMAIN LOOKUP (domain)
rdap        Authoritative registration data (default); no key
wayback     Archived URL leads; no key
urlscan     Existing public scan leads; optional URLSCAN_API_KEY";

/// Everything a command run depends on from outside, so tests can stand in for it.
pub struct Runtime {
    pub env: Box<dyn Env>,
    /// Makes the HTTP client; called only by the three search commands.
    pub http: Box<dyn Fn() -> Box<dyn Http>>,
    pub people: Catalog<PersonSource>,
    pub organizations: Catalog<NameSource>,
    pub domains: Catalog<NameSource>,
}

impl Runtime {
    /// The real environment, network and source catalogues.
    pub fn standard() -> Runtime {
        Runtime {
            env: Box::new(ProcessEnv),
            http: Box::new(|| Box::new(HttpClient::new()) as Box<dyn Http>),
            people: sources::person_sources(),
            organizations: sources::organization_sources(),
            domains: sources::domain_sources(),
        }
    }
}

/// A command and its arguments, checked for shape but not yet run.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Search {
        first: String,
        last: String,
        location: String,
        sources: Vec<String>,
        out: Option<PathBuf>,
    },
    SearchOrg {
        name: String,
        sources: Vec<String>,
        out: Option<PathBuf>,
    },
    Domain {
        name: String,
        sources: Vec<String>,
        out: Option<PathBuf>,
    },
    ImportFile {
        file: PathBuf,
        out: Option<PathBuf>,
    },
    ImportLegacy {
        file: PathBuf,
        out: Option<PathBuf>,
    },
    Demo {
        out: Option<PathBuf>,
    },
    Sources,
    List,
    Show {
        run_id: String,
    },
    Inspect {
        observation_id: i64,
    },
    Export {
        run_id: String,
        format: Format,
        out: PathBuf,
    },
    Link {
        first: i64,
        second: i64,
        reason: String,
    },
    Unlink {
        first: i64,
        second: i64,
    },
    Delete {
        run_id: String,
    },
    Backup {
        out: PathBuf,
    },
    Prune {
        days: i64,
    },
}

/// What a command accepts: positional names, and options that take a value.
struct Spec {
    name: &'static str,
    positionals: &'static [&'static str],
    options: &'static [&'static str],
    synopsis: &'static str,
    about: &'static str,
}

const SPECS: &[Spec] = &[
    Spec {
        name: "search",
        positionals: &["first_name", "last_name"],
        options: &["--location", "--source", "--out"],
        synopsis: "search FIRST LAST [--location \"City, ST\"] [--source NAME]... [--out FILE.json]",
        about: "Query published person directories by first and last name. NPI and Wikidata run unless\n\
                --source names others; repeat --source to use several. --location filters NPI practice\n\
                locations only. --out also saves the run as a JSON report.",
    },
    Spec {
        name: "search-org",
        positionals: &["name"],
        options: &["--source", "--out"],
        synopsis: "search-org \"LEGAL NAME\" [--source NAME]... [--out FILE.json]",
        about: "Query legal-entity registers by exact legal name. GLEIF runs unless --source names others.",
    },
    Spec {
        name: "domain",
        positionals: &["name"],
        options: &["--source", "--out"],
        synopsis: "domain example.org [--source NAME]... [--out FILE.json]",
        about: "Look a domain up in registration and archive sources (RDAP by default). Give a bare\n\
                domain name: no URL, port or wildcard. The target website itself is never contacted.",
    },
    Spec {
        name: "import-file",
        positionals: &["file"],
        options: &["--out"],
        synopsis: "import-file FILE.json|FILE.csv [--out FILE.json]",
        about: "Import your own observations from JSON or CSV (at most 20 MB and 10,000 observations).",
    },
    Spec {
        name: "import-legacy",
        positionals: &["file"],
        options: &["--out"],
        synopsis: "import-legacy OLD.db [--out FILE.json]",
        about: "Import an old person_records SQLite database without changing it. Each record becomes one\n\
                unverified imported observation with a provenance warning.",
    },
    Spec {
        name: "demo",
        positionals: &[],
        options: &["--out"],
        synopsis: "demo [--out FILE.html]",
        about: "Store a synthetic case through the real pipeline, entirely offline, and optionally write\n\
                its HTML report.",
    },
    Spec {
        name: "sources",
        positionals: &[],
        options: &[],
        synopsis: "sources",
        about: "List the published API connectors, their defaults and their credentials.",
    },
    Spec {
        name: "list",
        positionals: &[],
        options: &[],
        synopsis: "list",
        about: "List stored runs, newest first: ID, time, status, observation count and query.",
    },
    Spec {
        name: "show",
        positionals: &["run_id"],
        options: &[],
        synopsis: "show RUN_ID",
        about: "Print a run's JSON report.",
    },
    Spec {
        name: "inspect",
        positionals: &["observation_id"],
        options: &[],
        synopsis: "inspect OBSERVATION_ID",
        about: "Print one observation and the ID of its run.",
    },
    Spec {
        name: "export",
        positionals: &["run_id"],
        options: &["--format", "--out"],
        synopsis: "export RUN_ID --out FILE [--format json|csv|html|graphml]",
        about: "Write a report (HTML by default) or graph. Files are replaced atomically.",
    },
    Spec {
        name: "link",
        positionals: &["first", "second"],
        options: &["--reason"],
        synopsis: "link FIRST_ID SECOND_ID --reason \"what you checked\"",
        about: "Record that you reviewed two observations and judge them the same subject. Both must be\n\
                records (not leads) of the same subject type; the reason is required.",
    },
    Spec {
        name: "unlink",
        positionals: &["first", "second"],
        options: &[],
        synopsis: "unlink FIRST_ID SECOND_ID",
        about: "Remove a reviewed link.",
    },
    Spec {
        name: "delete",
        positionals: &["run_id"],
        options: &[],
        synopsis: "delete RUN_ID",
        about: "Delete a run, its observations and their links.",
    },
    Spec {
        name: "backup",
        positionals: &[],
        options: &["--out"],
        synopsis: "backup --out FILE.db",
        about: "Make a consistent copy of the case file with SQLite's backup API.",
    },
    Spec {
        name: "prune",
        positionals: &[],
        options: &["--days"],
        synopsis: "prune --days N",
        about: "Delete runs older than N days.",
    },
];

/// A mistake in how the command was typed (exit code 2).
#[derive(Debug)]
struct Usage {
    command: Option<&'static str>,
    message: String,
}

fn usage(command: Option<&'static str>, message: impl Into<String>) -> Usage {
    Usage {
        command,
        message: message.into(),
    }
}

/// Global options and the command words that follow them.
struct Parsed {
    db: Option<PathBuf>,
    words: Vec<String>,
}

/// Pull `--db` out from anywhere before a `--`, and keep everything else in order.
fn take_global(args: &[String]) -> std::result::Result<Parsed, Usage> {
    let mut db = None;
    let mut words = Vec::new();
    let mut index = 0;
    let mut literal = false;
    while index < args.len() {
        let arg = &args[index];
        if literal {
            words.push(arg.clone());
        } else if arg == "--" {
            literal = true;
            words.push(arg.clone());
        } else if arg == "--db" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| usage(None, "argument --db: expected one argument"))?;
            db = Some(PathBuf::from(value));
            index += 1;
        } else if let Some(value) = arg.strip_prefix("--db=") {
            db = Some(PathBuf::from(value));
        } else {
            words.push(arg.clone());
        }
        index += 1;
    }
    Ok(Parsed { db, words })
}

/// A command's positional words and its options with their values.
type SplitArgs = (Vec<String>, Vec<(&'static str, String)>);

/// Options and positionals for one command, in any order, `--` ending options.
fn split_args(spec: &Spec, words: &[String]) -> std::result::Result<SplitArgs, Usage> {
    let mut positionals = Vec::new();
    let mut options = Vec::new();
    let mut index = 0;
    let mut literal = false;
    while index < words.len() {
        let word = &words[index];
        index += 1;
        if literal {
            positionals.push(word.clone());
            continue;
        }
        if word == "--" {
            literal = true;
            continue;
        }
        let looks_like_option =
            word.starts_with('-') && word.len() > 1 && parse_int(word).is_none();
        if !looks_like_option {
            positionals.push(word.clone());
            continue;
        }
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (word.as_str(), None),
        };
        let Some(option) = spec.options.iter().find(|option| **option == name) else {
            return Err(usage(
                Some(spec.name),
                format!("unrecognized arguments: {word}"),
            ));
        };
        let value = match inline {
            Some(value) => value,
            None => {
                let value = words.get(index).ok_or_else(|| {
                    usage(
                        Some(spec.name),
                        format!("argument {option}: expected one argument"),
                    )
                })?;
                index += 1;
                value.clone()
            }
        };
        options.push((*option, value));
    }
    if positionals.len() > spec.positionals.len() {
        let extra = positionals[spec.positionals.len()..].join(" ");
        return Err(usage(
            Some(spec.name),
            format!("unrecognized arguments: {extra}"),
        ));
    }
    if positionals.len() < spec.positionals.len() {
        let missing = spec.positionals[positionals.len()..].join(", ");
        return Err(usage(
            Some(spec.name),
            format!("the following arguments are required: {missing}"),
        ));
    }
    Ok((positionals, options))
}

/// The last value given for an option, as argparse keeps it.
fn last(options: &[(&'static str, String)], name: &str) -> Option<String> {
    options
        .iter()
        .rev()
        .find(|(option, _)| *option == name)
        .map(|(_, value)| value.clone())
}

fn every(options: &[(&'static str, String)], name: &str) -> Vec<String> {
    options
        .iter()
        .filter(|(option, _)| *option == name)
        .map(|(_, value)| value.clone())
        .collect()
}

fn checked_sources<F>(
    command: &'static str,
    chosen: Vec<String>,
    catalog: &Catalog<F>,
) -> std::result::Result<Vec<String>, Usage>
where
    F: Copy,
{
    for name in &chosen {
        if catalog.get(name).is_none() {
            let choices = catalog
                .names()
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(usage(
                Some(command),
                format!("argument --source: invalid choice: '{name}' (choose from {choices})"),
            ));
        }
    }
    Ok(chosen)
}

fn int_argument(command: &'static str, label: &str, text: &str) -> std::result::Result<i64, Usage> {
    parse_int(text).ok_or_else(|| {
        usage(
            Some(command),
            format!("argument {label}: invalid int value: '{text}'"),
        )
    })
}

fn parse_command(words: &[String], rt: &Runtime) -> std::result::Result<Command, Usage> {
    let Some(first) = words.first() else {
        return Err(usage(None, "the following arguments are required: command"));
    };
    let Some(spec) = SPECS.iter().find(|spec| spec.name == first.as_str()) else {
        let choices = SPECS
            .iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(usage(
            None,
            format!("argument command: invalid choice: '{first}' (choose from {choices})"),
        ));
    };
    let (positionals, options) = split_args(spec, &words[1..])?;
    let out = last(&options, "--out").map(PathBuf::from);
    let name = spec.name;
    Ok(match name {
        "search" => Command::Search {
            first: positionals[0].clone(),
            last: positionals[1].clone(),
            location: last(&options, "--location").unwrap_or_default(),
            sources: checked_sources(name, every(&options, "--source"), &rt.people)?,
            out,
        },
        "search-org" => Command::SearchOrg {
            name: positionals[0].clone(),
            sources: checked_sources(name, every(&options, "--source"), &rt.organizations)?,
            out,
        },
        "domain" => Command::Domain {
            name: positionals[0].clone(),
            sources: checked_sources(name, every(&options, "--source"), &rt.domains)?,
            out,
        },
        "import-file" => Command::ImportFile {
            file: PathBuf::from(&positionals[0]),
            out,
        },
        "import-legacy" => Command::ImportLegacy {
            file: PathBuf::from(&positionals[0]),
            out,
        },
        "demo" => Command::Demo { out },
        "sources" => Command::Sources,
        "list" => Command::List,
        "show" => Command::Show {
            run_id: positionals[0].clone(),
        },
        "inspect" => Command::Inspect {
            observation_id: int_argument(name, "observation_id", &positionals[0])?,
        },
        "export" => {
            let format = match last(&options, "--format") {
                None => Format::Html,
                Some(text) => text.parse().map_err(|_| {
                    let choices = Format::NAMES
                        .iter()
                        .map(|name| format!("'{name}'"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    usage(
                        Some(name),
                        format!(
                            "argument --format: invalid choice: '{text}' (choose from {choices})"
                        ),
                    )
                })?,
            };
            Command::Export {
                run_id: positionals[0].clone(),
                format,
                out: out.ok_or_else(|| {
                    usage(Some(name), "the following arguments are required: --out")
                })?,
            }
        }
        "link" => Command::Link {
            first: int_argument(name, "first", &positionals[0])?,
            second: int_argument(name, "second", &positionals[1])?,
            reason: last(&options, "--reason").ok_or_else(|| {
                usage(Some(name), "the following arguments are required: --reason")
            })?,
        },
        "unlink" => Command::Unlink {
            first: int_argument(name, "first", &positionals[0])?,
            second: int_argument(name, "second", &positionals[1])?,
        },
        "delete" => Command::Delete {
            run_id: positionals[0].clone(),
        },
        "backup" => Command::Backup {
            out: out
                .ok_or_else(|| usage(Some(name), "the following arguments are required: --out"))?,
        },
        "prune" => {
            let days = last(&options, "--days")
                .ok_or_else(|| usage(Some(name), "the following arguments are required: --days"))?;
            Command::Prune {
                days: int_argument(name, "--days", &days)?,
            }
        }
        _ => return Err(usage(None, format!("unknown command {name}"))),
    })
}

/// Writes that tolerate a closed pipe (`osintropy list | more` quitting early).
struct Output<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

impl Output<'_> {
    fn line(&mut self, text: &str) -> io::Result<()> {
        ignore_broken_pipe(writeln!(self.out, "{text}"))
    }

    fn raw(&mut self, text: &str) -> io::Result<()> {
        ignore_broken_pipe(self.out.write_all(text.as_bytes()))
    }

    fn error_line(&mut self, text: &str) {
        let _ = writeln!(self.err, "{text}");
    }
}

fn ignore_broken_pipe(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other,
    }
}

/// Run the command line (arguments without the program name) and return the exit code.
pub fn run(args: &[String], rt: &Runtime, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut output = Output { out, err };
    let code = match args.first().map(String::as_str) {
        None => {
            output.error_line(USAGE);
            2
        }
        Some("help" | "--help" | "-h") => help(args.get(1).map(String::as_str), &mut output),
        Some("version" | "--version" | "-V") if args.len() == 1 => {
            let text = format!("osintropy {}\n{NOTICE}", env!("CARGO_PKG_VERSION"));
            if output.line(&text).is_ok() {
                0
            } else {
                2
            }
        }
        _ => dispatch(args, rt, &mut output),
    };
    let _ = output.out.flush();
    code
}

fn help(topic: Option<&str>, output: &mut Output<'_>) -> u8 {
    let text = match topic {
        None => USAGE.to_owned(),
        Some(name) => match SPECS.iter().find(|spec| spec.name == name) {
            Some(spec) => format!(
                "usage: osintropy [--db FILE] {}\n\n{}",
                spec.synopsis, spec.about
            ),
            None => {
                output.error_line(&format!("osintropy: error: no command called '{name}'"));
                return 2;
            }
        },
    };
    if output.line(&text).is_ok() {
        0
    } else {
        2
    }
}

fn dispatch(args: &[String], rt: &Runtime, output: &mut Output<'_>) -> u8 {
    let parsed = match take_global(args) {
        Ok(parsed) => parsed,
        Err(problem) => return report_usage(&problem, output),
    };
    if let Some(first) = parsed.words.first() {
        if matches!(first.as_str(), "help" | "--help" | "-h") {
            return help(parsed.words.get(1).map(String::as_str), output);
        }
    }
    // `osintropy search --help` and friends.
    if let Some(command) = parsed.words.first() {
        let asks_help = parsed.words[1..]
            .iter()
            .take_while(|word| *word != "--")
            .any(|word| word == "--help" || word == "-h");
        if asks_help && SPECS.iter().any(|spec| spec.name == command.as_str()) {
            return help(Some(command), output);
        }
    }
    let command = match parse_command(&parsed.words, rt) {
        Ok(command) => command,
        Err(problem) => return report_usage(&problem, output),
    };
    match execute(command, parsed.db, rt, output) {
        Ok(code) => code,
        Err(problem) => {
            output.error_line(&format!("Error: {problem}"));
            2
        }
    }
}

fn report_usage(problem: &Usage, output: &mut Output<'_>) -> u8 {
    let synopsis = problem
        .command
        .and_then(|name| SPECS.iter().find(|spec| spec.name == name))
        .map_or("[--db FILE] <command> [arguments]", |spec| spec.synopsis);
    let prefix = problem.command.map_or_else(
        || "osintropy".to_owned(),
        |name| format!("osintropy {name}"),
    );
    output.error_line(&format!(
        "usage: osintropy {synopsis}\n{prefix}: error: {}\nRun `osintropy help` for every command.",
        problem.message
    ));
    2
}

fn open_store(db: Option<PathBuf>) -> Result<Store> {
    let path = match db {
        Some(path) => path,
        None => paths::default_db().ok_or_else(|| {
            Error::invalid("cannot find your home folder for the default case file; use --db FILE")
        })?,
    };
    Store::open(&path)
}

fn io_error(err: io::Error) -> Error {
    Error::Io(err)
}

fn execute(
    command: Command,
    db: Option<PathBuf>,
    rt: &Runtime,
    output: &mut Output<'_>,
) -> Result<u8> {
    if command == Command::Sources {
        output.line(SOURCES_TEXT).map_err(io_error)?;
        return Ok(0);
    }
    let store = open_store(db)?;
    match command {
        Command::Search {
            first,
            last,
            location,
            sources,
            out,
        } => {
            let first = clean_str(&first, 100)?;
            let last = clean_str(&last, 100)?;
            if first.is_empty() || last.is_empty() {
                return Err(Error::invalid("first and last name are required"));
            }
            let location = clean_str(&location, 100)?;
            let query = [&first, &last, &location]
                .iter()
                .filter(|part| !part.is_empty())
                .map(|part| part.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let person = PersonQuery {
                first,
                last,
                location,
            };
            let chosen = pick(&sources, &rt.people);
            let mut http = (rt.http)();
            let mut observations = Vec::new();
            let mut errors = BTreeMap::new();
            for name in chosen {
                if let Some(connector) = rt.people.get(&name) {
                    let mut ctx = Context {
                        http: http.as_mut(),
                        env: rt.env.as_ref(),
                    };
                    match connector(&person, &mut ctx) {
                        Ok(found) => observations.extend(found),
                        Err(problem) => {
                            errors.insert(name, problem.0);
                        }
                    }
                }
            }
            finish_search(&store, &query, observations, errors, out.as_deref(), output)
        }
        Command::SearchOrg { name, sources, out } => {
            let name = clean_str(&name, 250)?;
            if name.is_empty() {
                return Err(Error::invalid("organization name is required"));
            }
            let query = format!("Organization: {name}");
            run_named(
                &store,
                &query,
                &name,
                &sources,
                &rt.organizations,
                rt,
                out.as_deref(),
                output,
            )
        }
        Command::Domain { name, sources, out } => {
            let name = canonical_domain(&clean_str(&name, 253)?)?;
            let query = format!("Domain: {name}");
            run_named(
                &store,
                &query,
                &name,
                &sources,
                &rt.domains,
                rt,
                out.as_deref(),
                output,
            )
        }
        Command::ImportFile { file, out } => {
            let (query, observations) = load_file(&file)?;
            let run_id = store.add_run(&query, &observations, &BTreeMap::new())?;
            finish(&store, &run_id, out.as_deref(), Format::Json, output)?;
            Ok(0)
        }
        Command::ImportLegacy { file, out } => {
            let stem = file
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let observations = load_legacy_db(&file)?;
            let run_id = store.add_run(
                &format!("Legacy import: {stem}"),
                &observations,
                &BTreeMap::new(),
            )?;
            finish(&store, &run_id, out.as_deref(), Format::Json, output)?;
            Ok(0)
        }
        Command::Demo { out } => {
            let run_id = create_demo(&store)?;
            finish(&store, &run_id, out.as_deref(), Format::Html, output)?;
            Ok(0)
        }
        Command::List => {
            for run in store.list_runs()? {
                output
                    .line(&format!(
                        "{}  {}  {:<8}  {:>4}  {}",
                        run.id, run.created_at, run.status, run.count, run.query
                    ))
                    .map_err(io_error)?;
            }
            Ok(0)
        }
        Command::Show { run_id } => {
            let run = store.get_run(&run_id)?;
            output.raw(&render(&run, Format::Json)).map_err(io_error)?;
            Ok(0)
        }
        Command::Inspect { observation_id } => {
            let (run_id, item) = store.get_observation(observation_id)?;
            let mut entries = vec![("run_id".to_owned(), Json::str(run_id))];
            if let Json::Object(rest) = item.to_json() {
                entries.extend(rest);
            }
            output
                .line(&Json::Object(entries).dumps(Style::INSPECT))
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Export {
            run_id,
            format,
            out,
        } => {
            let run = store.get_run(&run_id)?;
            let written =
                paths::save(&out, &render(&run, format)).map_err(|err| Error::io_at(&out, err))?;
            output
                .line(&written.display().to_string())
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Link {
            first,
            second,
            reason,
        } => {
            store.link(first, second, &reason)?;
            output
                .line(&format!("Linked #{first} and #{second}"))
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Unlink { first, second } => {
            let removed = store.unlink(first, second)?;
            output
                .line(&format!("Removed {removed} link(s)"))
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Delete { run_id } => {
            let deleted = store.delete_run(&run_id)?;
            output
                .line(&format!("Deleted {deleted} run(s)"))
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Backup { out } => {
            let written = store.backup(&out)?;
            output
                .line(&written.display().to_string())
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Prune { days } => {
            if days < 0 {
                return Err(Error::invalid("days cannot be negative"));
            }
            let before = crate::isotime::utc_days_ago(days)
                .ok_or_else(|| Error::invalid("days reaches back before year 1"))?;
            let deleted = store.prune(&before)?;
            output
                .line(&format!("Deleted {deleted} run(s)"))
                .map_err(io_error)?;
            Ok(0)
        }
        Command::Sources => Ok(0),
    }
}

/// The sources asked for, or the defaults, each once, in the order given.
fn pick<F: Copy>(chosen: &[String], catalog: &Catalog<F>) -> Vec<String> {
    let names: Vec<String> = if chosen.is_empty() {
        catalog
            .defaults
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    } else {
        chosen.to_vec()
    };
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_named(
    store: &Store,
    query: &str,
    name: &str,
    sources: &[String],
    catalog: &Catalog<NameSource>,
    rt: &Runtime,
    out: Option<&Path>,
    output: &mut Output<'_>,
) -> Result<u8> {
    let mut http = (rt.http)();
    let mut observations = Vec::new();
    let mut errors = BTreeMap::new();
    for source in pick(sources, catalog) {
        if let Some(connector) = catalog.get(&source) {
            let mut ctx = Context {
                http: http.as_mut(),
                env: rt.env.as_ref(),
            };
            match connector(name, &mut ctx) {
                Ok(found) => observations.extend(found),
                Err(problem) => {
                    errors.insert(source, problem.0);
                }
            }
        }
    }
    finish_search(store, query, observations, errors, out, output)
}

/// Save a search. A source that repeats a hit has the repeat dropped, so one duplicated
/// reply cannot sink the whole run on the database's uniqueness rule.
fn finish_search(
    store: &Store,
    query: &str,
    observations: Vec<Observation>,
    errors: BTreeMap<String, String>,
    out: Option<&Path>,
    output: &mut Output<'_>,
) -> Result<u8> {
    let mut seen = HashSet::new();
    let observations: Vec<Observation> = observations
        .into_iter()
        .filter(|item| seen.insert((item.source.clone(), item.source_id.clone())))
        .collect();
    let failed = !errors.is_empty();
    let run_id = store.add_run(query, &observations, &errors)?;
    finish(store, &run_id, out, Format::Json, output)?;
    Ok(if failed { 3 } else { 0 })
}

/// The summary line, any source errors (on stderr), and the optional report.
fn finish(
    store: &Store,
    run_id: &str,
    out: Option<&Path>,
    format: Format,
    output: &mut Output<'_>,
) -> Result<()> {
    let run = store.get_run(run_id)?;
    output
        .line(&format!(
            "Run: {run_id} | {} observations | {}",
            run.observations.len(),
            run.status
        ))
        .map_err(io_error)?;
    for (source, problem) in &run.errors {
        output.error_line(&format!("  {source}: {problem}"));
    }
    if let Some(path) = out {
        let written =
            paths::save(path, &render(&run, format)).map_err(|err| Error::io_at(path, err))?;
        output
            .line(&format!("Report: {}", written.display()))
            .map_err(io_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_like_argparse() {
        let rt = Runtime::standard();
        let parsed = take_global(&words(
            "search --db x.db Ada Lovelace --source orcid --source npi",
        ))
        .unwrap();
        assert_eq!(parsed.db, Some(PathBuf::from("x.db")));
        match parse_command(&parsed.words, &rt).unwrap() {
            Command::Search {
                first,
                last,
                sources,
                ..
            } => {
                assert_eq!((first.as_str(), last.as_str()), ("Ada", "Lovelace"));
                assert_eq!(sources, ["orcid", "npi"]);
            }
            other => panic!("{other:?}"),
        }
        let link = parse_command(&words("link -3 2 --reason=checked"), &rt).unwrap();
        assert_eq!(
            link,
            Command::Link {
                first: -3,
                second: 2,
                reason: "checked".into()
            }
        );
        let prune = parse_command(&words("prune --days -1"), &rt).unwrap();
        assert_eq!(prune, Command::Prune { days: -1 });
        for bad in [
            "search Ada",
            "search Ada Lovelace Byron",
            "search Ada Lovelace --source nope",
            "search Ada Lovelace --sauce npi",
            "export abc",
            "export abc --out x --format pdf",
            "link 1 2",
            "inspect one",
            "frobnicate",
            "show",
            "prune",
        ] {
            assert!(parse_command(&words(bad), &rt).is_err(), "{bad}");
        }
    }

    #[test]
    fn sources_run_once_in_order() {
        let catalog = sources::person_sources();
        assert_eq!(pick(&[], &catalog), ["npi", "wikidata"]);
        let chosen = words("orcid npi orcid");
        assert_eq!(pick(&chosen, &catalog), ["orcid", "npi"]);
    }
}
