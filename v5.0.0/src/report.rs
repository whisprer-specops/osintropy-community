// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Deterministic local JSON, CSV, HTML and GraphML exports, byte for byte as 4.1.1 wrote
//! them.

use std::fmt::Write as _;
use std::str::FromStr;

use crate::analysis::summarize;
use crate::models::{observation_id, sha256_hex, Observation, Run};
use crate::pyfmt::{float_repr, lstrip};
use crate::pyjson::{Json, Style};

pub const METHOD: &str =
    "Each source hit remains separate until two observation IDs are explicitly linked after review.";

/// The export formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Csv,
    Html,
    GraphMl,
}

impl Format {
    pub const NAMES: [&'static str; 4] = ["json", "csv", "html", "graphml"];
}

impl FromStr for Format {
    type Err = String;

    fn from_str(text: &str) -> Result<Format, String> {
        match text {
            "json" => Ok(Format::Json),
            "csv" => Ok(Format::Csv),
            "html" => Ok(Format::Html),
            "graphml" => Ok(Format::GraphMl),
            other => Err(format!("unknown export format: {other}")),
        }
    }
}

/// The run in the chosen format.
pub fn render(run: &Run, format: Format) -> String {
    match format {
        Format::Json => json_text(run),
        Format::Csv => csv_text(run),
        Format::Html => html_text(run),
        Format::GraphMl => graphml_text(run),
    }
}

/// The complete run, its analysis and the method note, keys sorted, two-space indent.
pub fn json_text(run: &Run) -> String {
    let mut document = match run.to_json() {
        Json::Object(entries) => entries,
        _ => Vec::new(),
    };
    document.push(("analysis".to_owned(), summarize(run).to_json()));
    document.push(("method".to_owned(), Json::str(METHOD)));
    let mut text = Json::Object(document).dumps(Style::REPORT);
    text.push('\n');
    text
}

/// Spreadsheet programs may run a cell that starts with `=`, `+`, `-` or `@` (even after
/// spaces) or with a tab or line break, so such cells get a leading apostrophe.
fn safe_cell(text: &str) -> String {
    let trimmed = lstrip(text);
    if trimmed.starts_with(['=', '+', '-', '@']) || text.starts_with(['\t', '\r', '\n']) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

fn csv_row(out: &mut String, cells: &[String]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&safe_cell(cell).replace('"', "\"\""));
        out.push('"');
    }
    out.push('\n');
}

/// One row per field value: a flat export, not an import format.
pub fn csv_text(run: &Run) -> String {
    let mut out = String::new();
    let header = [
        "run_id",
        "observation_id",
        "kind",
        "subject_type",
        "source",
        "source_id",
        "name",
        "field",
        "value",
        "source_url",
        "collected_at",
    ];
    csv_row(&mut out, &header.map(str::to_owned));
    for item in &run.observations {
        let empty = [(String::new(), vec![String::new()])];
        let pairs: Vec<(&String, &Vec<String>)> = if item.fields.is_empty() {
            empty.iter().map(|(k, v)| (k, v)).collect()
        } else {
            item.fields.iter().collect()
        };
        for (field, values) in pairs {
            for value in values {
                csv_row(
                    &mut out,
                    &[
                        run.id.clone(),
                        observation_id(item).to_string(),
                        item.kind.as_str().to_owned(),
                        item.subject_type.as_str().to_owned(),
                        item.source.clone(),
                        item.source_id.clone(),
                        item.name.clone(),
                        field.clone(),
                        value.clone(),
                        item.source_url.clone(),
                        item.collected_at.clone(),
                    ],
                );
            }
        }
    }
    out
}

/// ElementTree's text escaping.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// ElementTree's attribute escaping.
fn xml_attr(text: &str) -> String {
    xml_text(text)
        .replace('"', "&quot;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#09;")
}

fn source_node(source: &str) -> String {
    format!("source-{}", &sha256_hex(source.as_bytes())[..16])
}

/// Every source and observation as nodes, an edge from each observation to its source,
/// and an edge for each reviewed link inside the run.
pub fn graphml_text(run: &Run) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">",
    );
    for attr in ["label", "type", "subject_type"] {
        let _ = write!(
            out,
            "<key id=\"{attr}\" for=\"node\" attr.name=\"{attr}\" attr.type=\"string\" />"
        );
    }
    let _ = write!(
        out,
        "<graph id=\"{}\" edgedefault=\"undirected\"",
        xml_attr(&run.id)
    );
    if run.observations.is_empty() {
        // ElementTree writes an element with no children in its short form.
        out.push_str(" /></graphml>\n");
        return out;
    }
    out.push('>');
    let mut sources: Vec<&str> = run.observations.iter().map(|o| o.source.as_str()).collect();
    sources.sort_unstable();
    sources.dedup();
    for source in sources {
        let _ = write!(
            out,
            "<node id=\"{}\"><data key=\"label\">{}</data><data key=\"type\">source</data></node>",
            source_node(source),
            xml_text(source)
        );
    }
    for item in &run.observations {
        let id = observation_id(item);
        let _ = write!(
            out,
            "<node id=\"observation-{id}\"><data key=\"label\">{}</data><data key=\"type\">{}</data><data key=\"subject_type\">{}</data></node>",
            xml_text(&item.name),
            item.kind.as_str(),
            item.subject_type.as_str()
        );
        let _ = write!(
            out,
            "<edge source=\"observation-{id}\" target=\"{}\" />",
            source_node(&item.source)
        );
    }
    let ids: Vec<i64> = run.observations.iter().map(observation_id).collect();
    for link in &run.reviewed_links {
        if ids.contains(&link.a) && ids.contains(&link.b) {
            let _ = write!(
                out,
                "<edge source=\"observation-{}\" target=\"observation-{}\" />",
                link.a, link.b
            );
        }
    }
    out.push_str("</graph></graphml>\n");
    out
}

/// Python's `html.escape(text, quote=True)`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

fn first_chars(text: &str, count: usize) -> &str {
    match text.char_indices().nth(count) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// A bounded, self-contained preview of the first 80 observations; GraphML has them all.
fn svg_graph(run: &Run) -> String {
    let items: Vec<&Observation> = run.observations.iter().take(80).collect();
    if items.is_empty() {
        return String::new();
    }
    let mut sources: Vec<&str> = items.iter().map(|o| o.source.as_str()).collect();
    sources.sort_unstable();
    sources.dedup();
    let height = (72 + 48 * items.len()).max(170);
    let source_y = |name: &str| -> f64 {
        let index = sources.iter().position(|s| *s == name).unwrap_or(0);
        58.0 + ((height - 110) as f64 * (index as f64 + 0.5)) / sources.len() as f64
    };
    let item_y = |id: i64| -> Option<f64> {
        items
            .iter()
            .position(|o| observation_id(o) == id)
            .map(|index| (58 + index * 48) as f64)
    };
    let mut edges = String::new();
    for item in &items {
        let _ = write!(
            edges,
            "<line x1=\"220\" y1=\"{:.1}\" x2=\"590\" y2=\"{:.1}\" stroke=\"#3f7083\" stroke-width=\"1\"/>",
            source_y(&item.source),
            item_y(observation_id(item)).unwrap_or(0.0)
        );
    }
    for link in &run.reviewed_links {
        if let (Some(ya), Some(yb)) = (item_y(link.a), item_y(link.b)) {
            let _ = write!(
                edges,
                "<path d=\"M 605 {ya:.1} Q 550 {:.1} 605 {yb:.1}\" fill=\"none\" stroke=\"#67e8c7\" stroke-width=\"3\"/>",
                (ya + yb) / 2.0
            );
        }
    }
    let mut nodes = String::new();
    for source in &sources {
        let y = source_y(source);
        let _ = write!(
            nodes,
            "<circle cx=\"220\" cy=\"{y:.1}\" r=\"7\" fill=\"#8bd9ff\"/><text x=\"205\" y=\"{:.1}\" text-anchor=\"end\" fill=\"#edf4f7\">{}</text>",
            y + 5.0,
            escape(first_chars(source, 27))
        );
    }
    for item in &items {
        let id = observation_id(item);
        let y = item_y(id).unwrap_or(0.0);
        let label = format!("#{id} {}", first_chars(&item.name, 32));
        let _ = write!(
            nodes,
            "<circle cx=\"605\" cy=\"{y:.1}\" r=\"7\" fill=\"#67e8c7\"/><text x=\"620\" y=\"{:.1}\" fill=\"#edf4f7\">{}</text>",
            y + 5.0,
            escape(&label)
        );
    }
    let total = run.observations.len();
    let note = if total > 80 {
        format!(
            "<p>Showing the first 80 of {total} observations. GraphML contains all of them.</p>"
        )
    } else {
        String::new()
    };
    format!(
        "<section><h2>Evidence map</h2><p>Blue: source · Green: observation · Green curve: reviewed link</p><div style=\"overflow:auto\"><svg role=\"img\" aria-label=\"Evidence source and observation graph\" width=\"900\" height=\"{height}\" viewBox=\"0 0 900 {height}\">{edges}{nodes}</svg></div>{note}</section>"
    )
}

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A self-contained page that works offline, with a strict content security policy.
pub fn html_text(run: &Run) -> String {
    let analysis = summarize(run);
    let mut cards = String::new();
    for item in &run.observations {
        let mut fields = String::new();
        for (key, values) in &item.fields {
            let shown: Vec<String> = values.iter().map(|value| escape(value)).collect();
            let _ = write!(
                fields,
                "<tr><th>{}</th><td>{}</td></tr>",
                escape(key),
                shown.join("<br>")
            );
        }
        let link = if item.source_url.is_empty() {
            "Imported locally".to_owned()
        } else {
            format!(
                "<a rel=\"noopener noreferrer\" href=\"{}\">Source record</a>",
                escape(&item.source_url)
            )
        };
        let _ = write!(
            cards,
            "<article><header><span class=\"badge\">#{} · {} · {}</span><h2>{}</h2></header><p>{} · {} · {link}</p><table>{fields}</table></article>",
            observation_id(item),
            escape(item.subject_type.as_str()),
            escape(item.kind.as_str()),
            escape(&item.name),
            escape(&item.source),
            escape(&item.collected_at)
        );
    }
    if cards.is_empty() {
        cards.push_str("<section><p>No observations returned.</p></section>");
    }
    let mut links: String = run
        .reviewed_links
        .iter()
        .map(|link| {
            format!(
                "<li>#{} ↔ #{}: {}</li>",
                link.a,
                link.b,
                escape(&link.reason)
            )
        })
        .collect();
    if links.is_empty() {
        links.push_str("<li>None reviewed.</li>");
    }
    let mut errors: String = run
        .errors
        .iter()
        .map(|(source, message)| format!("<li>{}: {}</li>", escape(source), escape(message)))
        .collect();
    if errors.is_empty() {
        errors.push_str("<li>None.</li>");
    }
    let mut suggestions: String = analysis
        .possible_reviews
        .iter()
        .map(|group| format!("<li>{}</li>", id_list(group)))
        .collect();
    if suggestions.is_empty() {
        suggestions.push_str("<li>None.</li>");
    }
    let mut variations: String = analysis
        .field_variations
        .iter()
        .map(|variation| {
            format!(
                "<li>{} in {}: {}</li>",
                escape(&variation.field),
                id_list(&variation.observation_ids),
                escape(&variation.values.join(" / "))
            )
        })
        .collect();
    if variations.is_empty() {
        variations.push_str("<li>None.</li>");
    }
    let query = escape(&run.query);
    format!(
        r##"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'">
<title>OSINTropy · {query}</title>
<style>body{{font:16px/1.55 system-ui,sans-serif;background:#0b1522;color:#edf4f7;margin:0}}
main{{max-width:960px;margin:auto;padding:2.5rem 1.2rem 5rem}}h1{{font-size:clamp(2rem,5vw,3.5rem);margin:.2rem 0}}
.sub{{color:#a3bbcb}}.stats{{display:flex;flex-wrap:wrap;gap:.7rem;margin:1.8rem 0}}
.stats div,article,section{{background:#122338;border:1px solid #29455b;border-radius:12px;padding:1rem 1.3rem}}
.stats div{{min-width:120px}}.stats strong{{display:block;font-size:1.7rem;color:#67e8c7}}
article{{margin:1rem 0}}header{{display:flex;align-items:center;gap:1rem;flex-wrap:wrap}}h2{{margin:.3rem 0}}
.badge{{border:1px solid #59bea7;border-radius:99px;padding:.2rem .6rem;color:#67e8c7;font-size:.85rem}}
table{{border-collapse:collapse;width:100%;overflow-wrap:anywhere}}th,td{{text-align:left;vertical-align:top;padding:.5rem;border-bottom:1px solid #29455b}}
th{{width:25%;color:#adc4d2}}a{{color:#8bd9ff}}section{{margin:1rem 0}}li{{margin:.25rem 0}}
</style></head><body><main><p class="sub">OSINTROPY / CASE NOTEBOOK</p>
<h1>{query}</h1><p class="sub">Run {run_id} · {created_at} · {status}</p>
<div class="stats"><div><strong>{observation_count}</strong>observations</div>
<div><strong>{record_count}</strong>records</div><div><strong>{lead_count}</strong>unverified leads</div>
<div><strong>{group_count}</strong>reviewed groups</div></div>
<section><h2>How to read this</h2><p>Each card is a source observation. Matching names do not establish identity.
Links below were added manually after review. Source diversity ({bits} bits)
measures distribution, not accuracy or confidence.</p></section>
{svg}
<section><h2>Review queue</h2><p>These records share an exact name and subject type but remain separate:</p><ul>{suggestions}</ul>
<h2>Field variations in linked records</h2><ul>{variations}</ul></section>
{cards}
<section><h2>Reviewed links</h2><ul>{links}</ul></section>
<section><h2>Source errors</h2><ul>{errors}</ul></section></main></body></html>"##,
        run_id = escape(&run.id),
        created_at = escape(&run.created_at),
        status = escape(&run.status),
        observation_count = analysis.observation_count,
        record_count = analysis.record_count,
        lead_count = analysis.lead_count,
        group_count = analysis.reviewed_groups.len(),
        bits = float_repr(analysis.source_diversity_bits),
        svg = svg_graph(run),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_cells_are_neutralised() {
        assert_eq!(safe_cell("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(safe_cell("  -1"), "'  -1");
        assert_eq!(safe_cell("\u{1c}@x"), "'\u{1c}@x");
        assert_eq!(safe_cell("\tx"), "'\tx");
        assert_eq!(safe_cell("a=b"), "a=b");
    }

    #[test]
    fn escapes_match_python() {
        assert_eq!(
            escape("<a href='x'>&\"</a>"),
            "&lt;a href=&#x27;x&#x27;&gt;&amp;&quot;&lt;/a&gt;"
        );
        assert_eq!(xml_attr("a\"b\n<"), "a&quot;b&#10;&lt;");
        assert_eq!(first_chars("héllo", 2), "hé");
    }
}
