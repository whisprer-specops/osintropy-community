// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Descriptive measures with no invented certainty about identity or truth.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::models::{normalized_name, observation_id, Kind, Run, SubjectType};
use crate::pyfmt::{float_sum, round_to};
use crate::pyjson::Json;

pub const INTERPRETATION: &str =
    "Source diversity measures distribution, not truth, identity, or confidence.";

/// A field whose values differ inside a group of linked observations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variation {
    pub observation_ids: Vec<i64>,
    pub field: String,
    pub values: Vec<String>,
}

/// What 4.1.1's `summarize()` reported about a run.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub observation_count: usize,
    pub record_count: usize,
    pub lead_count: usize,
    /// Observations per source, sorted by source name.
    pub source_counts: BTreeMap<String, usize>,
    /// Shannon entropy of the source mix, in bits, rounded to 4 places.
    pub source_diversity_bits: f64,
    pub reviewed_groups: Vec<Vec<i64>>,
    pub possible_reviews: Vec<Vec<i64>>,
    pub field_variations: Vec<Variation>,
    pub unreviewed_ids: Vec<i64>,
}

/// Union-find with path halving, as in 4.1.1.
struct Groups {
    parent: HashMap<i64, i64>,
}

impl Groups {
    fn find(&mut self, mut node: i64) -> i64 {
        loop {
            let parent = self.parent[&node];
            if parent == node {
                return node;
            }
            let grandparent = self.parent[&parent];
            self.parent.insert(node, grandparent);
            node = grandparent;
        }
    }
}

/// Summarise a run: counts, source diversity, reviewed groups, and the review queue of
/// exact same-name records that are not yet linked.
pub fn summarize(run: &Run) -> Summary {
    let items = &run.observations;
    let total = items.len();

    // Sources in order of first appearance, as Python's Counter keeps them; the sum's
    // order matters to the last bit.
    let mut order: Vec<&str> = Vec::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in items {
        let count = counts.entry(item.source.as_str()).or_insert(0);
        if *count == 0 {
            order.push(item.source.as_str());
        }
        *count += 1;
    }
    let source_diversity_bits = if total == 0 {
        0.0
    } else {
        let terms: Vec<f64> = order
            .iter()
            .map(|source| {
                let share = counts[source] as f64 / total as f64;
                share * share.log2()
            })
            .collect();
        // Adding 0.0 turns the -0.0 of a single-source run into 0.0.
        round_to(-float_sum(&terms), 4) + 0.0
    };

    let ids: Vec<i64> = items.iter().map(observation_id).collect();
    let mut groups = Groups {
        parent: ids.iter().map(|&id| (id, id)).collect(),
    };
    for link in &run.reviewed_links {
        if groups.parent.contains_key(&link.a) && groups.parent.contains_key(&link.b) {
            let root_a = groups.find(link.a);
            let root_b = groups.find(link.b);
            groups.parent.insert(root_a, root_b);
        }
    }
    // Groups in order of their first member, members in observation order.
    let mut group_order: Vec<i64> = Vec::new();
    let mut members: HashMap<i64, Vec<i64>> = HashMap::new();
    for &id in &ids {
        let root = groups.find(id);
        let list = members.entry(root).or_default();
        if list.is_empty() {
            group_order.push(root);
        }
        list.push(id);
    }

    let mut name_order: Vec<(SubjectType, String)> = Vec::new();
    let mut by_name: HashMap<(SubjectType, String), Vec<i64>> = HashMap::new();
    for item in items.iter().filter(|item| item.kind != Kind::Lead) {
        let key = (item.subject_type, normalized_name(&item.name));
        let list = by_name.entry(key.clone()).or_default();
        if list.is_empty() {
            name_order.push(key);
        }
        list.push(observation_id(item));
    }
    let mut possible_reviews: Vec<Vec<i64>> = Vec::new();
    for key in &name_order {
        let same_name = &by_name[key];
        if same_name.len() < 2 {
            continue;
        }
        let roots: BTreeSet<i64> = same_name.iter().map(|&id| groups.find(id)).collect();
        if roots.len() > 1 {
            let mut sorted = same_name.clone();
            sorted.sort_unstable();
            possible_reviews.push(sorted);
        }
    }
    possible_reviews.sort();

    let by_id: HashMap<i64, usize> = ids
        .iter()
        .enumerate()
        .map(|(index, &id)| (id, index))
        .collect();
    let mut field_variations: Vec<Variation> = Vec::new();
    let mut reviewed_groups: Vec<Vec<i64>> = Vec::new();
    for root in &group_order {
        let group = &members[root];
        if group.len() < 2 {
            continue;
        }
        let mut sorted_group = group.clone();
        sorted_group.sort_unstable();
        reviewed_groups.push(sorted_group.clone());
        let mut values: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for id in group {
            for (field, list) in &items[by_id[id]].fields {
                values
                    .entry(field.as_str())
                    .or_default()
                    .extend(list.iter().map(String::as_str));
            }
        }
        for (field, distinct) in values {
            if distinct.len() > 1 {
                field_variations.push(Variation {
                    observation_ids: sorted_group.clone(),
                    field: field.to_owned(),
                    values: distinct.into_iter().map(str::to_owned).collect(),
                });
            }
        }
    }
    field_variations
        .sort_by(|x, y| (&x.observation_ids, &x.field).cmp(&(&y.observation_ids, &y.field)));

    let mut unreviewed_ids: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|&id| {
            let root = groups.find(id);
            members[&root].len() == 1
        })
        .collect();
    unreviewed_ids.sort_unstable();

    Summary {
        observation_count: total,
        record_count: items.iter().filter(|item| item.kind != Kind::Lead).count(),
        lead_count: items.iter().filter(|item| item.kind == Kind::Lead).count(),
        source_counts: counts
            .into_iter()
            .map(|(source, count)| (source.to_owned(), count))
            .collect(),
        source_diversity_bits,
        reviewed_groups,
        possible_reviews,
        field_variations,
        unreviewed_ids,
    }
}

fn id_list(ids: &[i64]) -> Json {
    Json::Array(ids.iter().copied().map(Json::Int).collect())
}

impl Summary {
    /// The summary as 4.1.1's reports laid it out.
    pub fn to_json(&self) -> Json {
        Json::object([
            (
                "observation_count",
                Json::Int(self.observation_count as i64),
            ),
            ("record_count", Json::Int(self.record_count as i64)),
            ("lead_count", Json::Int(self.lead_count as i64)),
            (
                "source_counts",
                Json::Object(
                    self.source_counts
                        .iter()
                        .map(|(source, count)| (source.clone(), Json::Int(*count as i64)))
                        .collect(),
                ),
            ),
            (
                "source_diversity_bits",
                Json::Float(self.source_diversity_bits),
            ),
            (
                "reviewed_groups",
                Json::Array(self.reviewed_groups.iter().map(|g| id_list(g)).collect()),
            ),
            (
                "possible_reviews",
                Json::Array(self.possible_reviews.iter().map(|g| id_list(g)).collect()),
            ),
            (
                "field_variations_in_reviewed_groups",
                Json::Array(
                    self.field_variations
                        .iter()
                        .map(|variation| {
                            Json::object([
                                ("observation_ids", id_list(&variation.observation_ids)),
                                ("field", Json::str(&variation.field)),
                                ("values", Json::strings(&variation.values)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("unreviewed_ids", id_list(&self.unreviewed_ids)),
            ("interpretation", Json::str(INTERPRETATION)),
        ])
    }
}
