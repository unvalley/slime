//! A small, deterministic kana-kanji conversion baseline backed by a reduced
//! Mozc OSS dictionary.

mod compact;

use bumpalo::{Bump, collections::String as BumpString};
use compact::CompactDictionary;
use std::num::NonZeroUsize;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryEntry {
    pub reading: String,
    pub surface: String,
    pub left_id: u16,
    pub right_id: u16,
    pub word_cost: i32,
}

impl DictionaryEntry {
    #[must_use]
    pub fn new(reading: impl Into<String>, surface: impl Into<String>, word_cost: i32) -> Self {
        Self {
            reading: reading.into(),
            surface: surface.into(),
            left_id: 0,
            right_id: 0,
            word_cost,
        }
    }

    #[must_use]
    pub fn with_pos(
        reading: impl Into<String>,
        surface: impl Into<String>,
        left_id: u16,
        right_id: u16,
        word_cost: i32,
    ) -> Self {
        Self {
            reading: reading.into(),
            surface: surface.into(),
            left_id,
            right_id,
            word_cost,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub surface: String,
    pub cost: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub reading: String,
    pub surface: String,
    pub cost: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversion {
    pub surface: String,
    pub segments: Vec<Segment>,
    pub cost: i32,
}

/// Assigns a final ordering cost to a complete conversion candidate.
///
/// The dictionary and connection matrix generate plausible paths first. A
/// statistical language model can implement this trait later without changing
/// the lattice search or the platform-facing candidate API. Lower costs rank
/// first.
pub trait CandidateRanker {
    fn ranking_cost(&self, reading: &str, conversion: &Conversion) -> i32;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CostOnlyRanker;

impl CandidateRanker for CostOnlyRanker {
    fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
        conversion.cost
    }
}

#[derive(Clone, Debug)]
pub struct DictionaryLayer {
    id: String,
    name: String,
    entries: Arc<[DictionaryEntry]>,
    max_reading_bytes: usize,
}

impl DictionaryLayer {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        mut entries: Vec<DictionaryEntry>,
    ) -> Self {
        sort_entries(&mut entries);
        let max_reading_bytes = entries
            .iter()
            .map(|entry| entry.reading.len())
            .max()
            .unwrap_or(0);
        Self {
            id: id.into(),
            name: name.into(),
            entries: entries.into(),
            max_reading_bytes,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Clone, Debug)]
pub struct Dictionary {
    bundled: Option<&'static CompactDictionary>,
    layers: Arc<[DictionaryLayer]>,
    uses_connection_costs: bool,
}

/// A borrowed view of one dictionary entry during lattice construction. The
/// entry's reading is always the query string itself, so only the surface and
/// costs are carried.
#[derive(Clone, Copy, Debug)]
struct EntryView<'a> {
    surface: &'a str,
    left_id: u16,
    right_id: u16,
    word_cost: i32,
}

impl Dictionary {
    #[must_use]
    pub fn new(entries: Vec<DictionaryEntry>) -> Self {
        let layer = DictionaryLayer::new("default", "Default", entries);
        Self {
            bundled: None,
            layers: vec![layer].into(),
            uses_connection_costs: false,
        }
    }

    #[must_use]
    pub fn bundled() -> Self {
        Self {
            bundled: Some(CompactDictionary::bundled()),
            layers: Vec::new().into(),
            uses_connection_costs: true,
        }
    }

    #[must_use]
    pub fn bundled_with_layers(additional_layers: Vec<DictionaryLayer>) -> Self {
        Self {
            bundled: Some(CompactDictionary::bundled()),
            layers: additional_layers.into(),
            uses_connection_costs: true,
        }
    }

    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.bundled.map_or(0, CompactDictionary::entry_count)
            + self
                .layers
                .iter()
                .map(DictionaryLayer::entry_count)
                .sum::<usize>()
    }

    #[must_use]
    pub fn layer_count(&self) -> usize {
        usize::from(self.bundled.is_some()) + self.layers.len()
    }

    /// Calls `callback` for every entry whose reading equals `reading`.
    fn for_each_exact<'s>(&'s self, reading: &str, mut callback: impl FnMut(EntryView<'s>)) {
        if let Some(compact) = self.bundled {
            compact.for_each_exact(reading, |entry| {
                callback(EntryView {
                    surface: entry.surface,
                    left_id: entry.left_id,
                    right_id: entry.right_id,
                    word_cost: entry.word_cost,
                });
            });
        }
        for layer in self.layers.iter() {
            for entry in exact_entries_in_layer(layer, reading) {
                callback(EntryView {
                    surface: &entry.surface,
                    left_id: entry.left_id,
                    right_id: entry.right_id,
                    word_cost: entry.word_cost,
                });
            }
        }
    }

    /// Calls `callback(prefix_bytes, entry)` for every entry whose reading is
    /// a prefix of `suffix`.
    fn for_each_prefix<'s>(&'s self, suffix: &str, mut callback: impl FnMut(usize, EntryView<'s>)) {
        if let Some(compact) = self.bundled {
            compact.for_each_prefix(suffix, |prefix_bytes, entry| {
                callback(
                    prefix_bytes,
                    EntryView {
                        surface: entry.surface,
                        left_id: entry.left_id,
                        right_id: entry.right_id,
                        word_cost: entry.word_cost,
                    },
                );
            });
        }

        if self.layers.is_empty() {
            return;
        }
        let maximum = self
            .layers
            .iter()
            .map(|layer| layer.max_reading_bytes)
            .max()
            .unwrap_or(0);
        for prefix_bytes in suffix
            .char_indices()
            .skip(1)
            .map(|(index, _)| index)
            .chain(std::iter::once(suffix.len()))
        {
            if prefix_bytes > maximum {
                break;
            }
            let prefix = &suffix[..prefix_bytes];
            for layer in self.layers.iter() {
                for entry in exact_entries_in_layer(layer, prefix) {
                    callback(
                        prefix_bytes,
                        EntryView {
                            surface: &entry.surface,
                            left_id: entry.left_id,
                            right_id: entry.right_id,
                            word_cost: entry.word_cost,
                        },
                    );
                }
            }
        }
    }

    #[must_use]
    pub fn candidates(&self, reading: &str) -> Vec<Candidate> {
        self.candidates_with_ranker(reading, DEFAULT_N_BEST, &CostOnlyRanker)
    }

    #[must_use]
    pub fn candidates_with_ranker(
        &self,
        reading: &str,
        limit: usize,
        ranker: &dyn CandidateRanker,
    ) -> Vec<Candidate> {
        let mut candidates = Vec::<Candidate>::new();
        let mut conversions = Vec::new();
        self.for_each_exact(reading, |entry| {
            let cost = if entry.surface == reading {
                LITERAL_CANDIDATE_COST
            } else {
                entry.word_cost
            };
            conversions.push(Conversion {
                surface: entry.surface.to_owned(),
                segments: vec![Segment {
                    reading: reading.to_owned(),
                    surface: entry.surface.to_owned(),
                    cost,
                }],
                cost,
            });
        });
        let n_best = self.convert_n_best(reading, limit);
        if let Some(best) = n_best.first() {
            let maximum_cost = best.cost.saturating_add(candidate_cost_window(reading));
            // When one strong word covers the whole reading, patchwork paths
            // like Git+は+部 for ぎっとはぶ read as noise; keep only paths
            // that are near ties, such as 今日+と alongside 京都.
            let multi_segment_maximum = if best.segments.len() == 1 {
                best.cost.saturating_add(MULTI_SEGMENT_COST_WINDOW)
            } else {
                maximum_cost
            };
            conversions.extend(n_best.into_iter().filter(|conversion| {
                let maximum = if conversion.segments.len() > 1 {
                    multi_segment_maximum
                } else {
                    maximum_cost
                };
                conversion.cost <= maximum
            }));
        }

        for conversion in conversions {
            let cost = if conversion.surface == reading {
                LITERAL_CANDIDATE_COST
            } else {
                ranker.ranking_cost(reading, &conversion)
            };
            if let Some(existing) = candidates
                .iter_mut()
                .find(|candidate| candidate.surface == conversion.surface)
            {
                existing.cost = existing.cost.min(cost);
            } else {
                candidates.push(Candidate {
                    surface: conversion.surface,
                    cost,
                });
            }
        }

        if !candidates
            .iter()
            .any(|candidate| candidate.surface == reading)
        {
            candidates.push(Candidate {
                surface: reading.to_owned(),
                cost: LITERAL_CANDIDATE_COST,
            });
        }

        candidates.sort_unstable_by_key(|candidate| candidate.cost);
        candidates
    }

    /// Returns complete conversion paths ordered by their lattice cost.
    ///
    /// Unlike [`Self::convert_best`], this keeps multiple paths which arrive at
    /// the same part-of-speech state. It is intentionally used only when the
    /// candidate window is requested; live conversion stays on the optimized
    /// one-best path.
    #[must_use]
    pub fn convert_n_best(&self, reading: &str, limit: usize) -> Vec<Conversion> {
        if reading.is_empty() || limit == 0 {
            return Vec::new();
        }
        if self.uses_connection_costs {
            self.convert_n_best_connected(reading, limit)
        } else {
            self.convert_n_best_heuristic(reading, limit)
        }
    }

    #[must_use]
    pub fn convert_best(&self, reading: &str) -> Option<Conversion> {
        if self.uses_connection_costs {
            return self.convert_best_connected(reading);
        }
        self.convert_best_heuristic(reading)
    }

    fn convert_best_heuristic(&self, reading: &str) -> Option<Conversion> {
        if reading.is_empty() {
            return None;
        }

        let mut best_cost = vec![i32::MAX; reading.len() + 1];
        let mut previous: Vec<Option<Predecessor>> = vec![None; reading.len() + 1];
        best_cost[0] = 0;

        for start in reading
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(reading.len()))
        {
            let path_cost = best_cost[start];
            if path_cost == i32::MAX || start == reading.len() {
                continue;
            }

            let suffix = &reading[start..];
            self.for_each_prefix(suffix, |relative_end, entry| {
                let prefix = &suffix[..relative_end];
                let is_literal = entry.surface == prefix;
                if is_literal && !is_grammar_literal(prefix) {
                    return;
                }

                let end = start + relative_end;
                let word_cost = if is_literal { 0 } else { entry.word_cost };
                let segment_cost = word_cost.saturating_add(SEGMENT_PENALTY);
                update_path(
                    &mut best_cost,
                    &mut previous,
                    start,
                    end,
                    path_cost.saturating_add(segment_cost),
                    prefix,
                    entry.surface,
                    segment_cost,
                );
            });

            let Some(character) = suffix.chars().next() else {
                continue;
            };
            let end = start + character.len_utf8();
            let literal = &reading[start..end];
            update_path(
                &mut best_cost,
                &mut previous,
                start,
                end,
                path_cost.saturating_add(UNKNOWN_COST),
                literal,
                literal,
                UNKNOWN_COST,
            );
        }

        let total_cost = best_cost[reading.len()];
        if total_cost == i32::MAX {
            return None;
        }

        let mut reversed = Vec::new();
        let mut cursor = reading.len();
        while cursor > 0 {
            let predecessor = previous[cursor].take()?;
            cursor = predecessor.start;
            reversed.push(Segment {
                reading: predecessor.reading,
                surface: predecessor.surface,
                cost: predecessor.segment_cost,
            });
        }
        reversed.reverse();

        let surface_capacity = reversed.iter().map(|segment| segment.surface.len()).sum();
        let mut surface = String::with_capacity(surface_capacity);
        for segment in &reversed {
            surface.push_str(&segment.surface);
        }

        Some(Conversion {
            surface,
            segments: reversed,
            cost: total_cost,
        })
    }

    fn convert_best_connected(&self, reading: &str) -> Option<Conversion> {
        if reading.is_empty() {
            return None;
        }

        let connection = ConnectionMatrix::bundled();
        let synthetic_arena = Bump::new();
        let synthetic_by_start = synthetic_entries_by_start(reading, &synthetic_arena);
        let mut lattice: Vec<Vec<LatticeNode<'_>>> =
            (0..=reading.len()).map(|_| Vec::new()).collect();
        let mut predecessor_cache = Vec::new();

        for start in reading
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(reading.len()))
        {
            if start == reading.len() || (start > 0 && lattice[start].is_empty()) {
                continue;
            }
            predecessor_cache.clear();

            let suffix = &reading[start..];
            self.for_each_prefix(suffix, |relative_end, entry| {
                let Some((predecessor_cost, predecessor)) = cached_connected_predecessor(
                    &lattice,
                    start,
                    entry.left_id,
                    connection,
                    &mut predecessor_cache,
                ) else {
                    return;
                };
                let total_cost = predecessor_cost.saturating_add(entry.word_cost);
                insert_lattice_node(
                    &mut lattice[start + relative_end],
                    LatticeNode {
                        start,
                        predecessor,
                        reading: &suffix[..relative_end],
                        surface: entry.surface,
                        segment_cost: entry.word_cost,
                        right_id: entry.right_id,
                        total_cost,
                    },
                );
            });

            for synthetic in &synthetic_by_start[start] {
                let Some((predecessor_cost, predecessor)) = cached_connected_predecessor(
                    &lattice,
                    start,
                    synthetic.left_id,
                    connection,
                    &mut predecessor_cache,
                ) else {
                    continue;
                };
                let total_cost = predecessor_cost.saturating_add(synthetic.cost);
                insert_lattice_node(
                    &mut lattice[synthetic.end],
                    LatticeNode {
                        start,
                        predecessor,
                        reading: &reading[start..synthetic.end],
                        surface: synthetic.surface,
                        segment_cost: synthetic.cost,
                        right_id: synthetic.right_id,
                        total_cost,
                    },
                );
            }

            let character = suffix.chars().next()?;
            let end = start + character.len_utf8();
            let literal = &reading[start..end];
            if let Some((predecessor_cost, predecessor)) = cached_connected_predecessor(
                &lattice,
                start,
                UNKNOWN_POS_ID,
                connection,
                &mut predecessor_cache,
            ) {
                let total_cost = predecessor_cost.saturating_add(UNKNOWN_COST);
                insert_lattice_node(
                    &mut lattice[end],
                    LatticeNode {
                        start,
                        predecessor,
                        reading: literal,
                        surface: literal,
                        segment_cost: UNKNOWN_COST,
                        right_id: UNKNOWN_POS_ID,
                        total_cost,
                    },
                );
            }
        }

        reconstruct_connected_conversion(&lattice, reading.len(), connection)
    }

    fn convert_n_best_connected(&self, reading: &str, limit: usize) -> Vec<Conversion> {
        let connection = ConnectionMatrix::bundled();
        let synthetic_arena = Bump::new();
        let synthetic_by_start = synthetic_entries_by_start(reading, &synthetic_arena);
        let mut arena = Vec::<NBestNode<'_>>::with_capacity(n_best_arena_capacity(reading, limit));
        let mut lattice: Vec<Vec<usize>> = (0..=reading.len()).map(|_| Vec::new()).collect();

        for start in reading
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(reading.len()))
        {
            if start == reading.len() || (start > 0 && lattice[start].is_empty()) {
                continue;
            }
            let predecessors = lattice[start].clone();
            let suffix = &reading[start..];

            self.for_each_prefix(suffix, |relative_end, entry| {
                insert_connected_word(
                    &mut arena,
                    &mut lattice[start + relative_end],
                    &predecessors,
                    connection,
                    start,
                    &suffix[..relative_end],
                    entry.surface,
                    (entry.left_id, entry.right_id),
                    entry.word_cost,
                    limit,
                );
            });

            for synthetic in &synthetic_by_start[start] {
                insert_connected_word(
                    &mut arena,
                    &mut lattice[synthetic.end],
                    &predecessors,
                    connection,
                    start,
                    &reading[start..synthetic.end],
                    synthetic.surface,
                    (synthetic.left_id, synthetic.right_id),
                    synthetic.cost,
                    limit,
                );
            }

            insert_connected_unknown(
                reading,
                start,
                &predecessors,
                &mut arena,
                &mut lattice,
                connection,
                limit,
            );
        }

        let mut completed: Vec<_> = lattice[reading.len()]
            .iter()
            .map(|&node| {
                (
                    node,
                    arena[node]
                        .total_cost
                        .saturating_add(connection.cost(arena[node].right_id, BOS_EOS_POS_ID)),
                )
            })
            .collect();
        completed.sort_unstable_by_key(|(_, cost)| *cost);
        reconstruct_n_best_conversions(&arena, &completed, limit)
    }

    fn convert_n_best_heuristic(&self, reading: &str, limit: usize) -> Vec<Conversion> {
        let mut arena = Vec::<NBestNode<'_>>::with_capacity(n_best_arena_capacity(reading, limit));
        let mut lattice: Vec<Vec<usize>> = (0..=reading.len()).map(|_| Vec::new()).collect();

        for start in reading
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(reading.len()))
        {
            if start == reading.len() || (start > 0 && lattice[start].is_empty()) {
                continue;
            }
            let predecessors = lattice[start].clone();
            let suffix = &reading[start..];

            self.for_each_prefix(suffix, |relative_end, entry| {
                let prefix = &suffix[..relative_end];
                let is_literal = entry.surface == prefix;
                if is_literal && !is_grammar_literal(prefix) {
                    return;
                }
                let segment_cost =
                    if is_literal { 0 } else { entry.word_cost }.saturating_add(SEGMENT_PENALTY);
                if start == 0 {
                    insert_n_best_node(
                        &mut arena,
                        &mut lattice[start + relative_end],
                        NBestNode {
                            start,
                            predecessor: None,
                            reading: prefix,
                            surface: entry.surface,
                            segment_cost,
                            right_id: 0,
                            total_cost: segment_cost,
                        },
                        limit,
                    );
                } else {
                    for &predecessor in &predecessors {
                        let total_cost = arena[predecessor].total_cost.saturating_add(segment_cost);
                        insert_n_best_node(
                            &mut arena,
                            &mut lattice[start + relative_end],
                            NBestNode {
                                start,
                                predecessor: Some(NodeIndex::new(predecessor)),
                                reading: prefix,
                                surface: entry.surface,
                                segment_cost,
                                right_id: 0,
                                total_cost,
                            },
                            limit,
                        );
                    }
                }
            });

            insert_heuristic_unknown(
                reading,
                start,
                &predecessors,
                &mut arena,
                &mut lattice,
                limit,
            );
        }

        let mut completed: Vec<_> = lattice[reading.len()]
            .iter()
            .map(|&node| (node, arena[node].total_cost))
            .collect();
        completed.sort_unstable_by_key(|(_, cost)| *cost);
        reconstruct_n_best_conversions(&arena, &completed, limit)
    }
}

fn exact_entries_in_layer<'a>(
    layer: &'a DictionaryLayer,
    reading: &str,
) -> std::slice::Iter<'a, DictionaryEntry> {
    if reading.len() > layer.max_reading_bytes {
        return layer.entries[0..0].iter();
    }
    let start = layer
        .entries
        .partition_point(|entry| entry.reading.as_str() < reading);
    let end = layer
        .entries
        .partition_point(|entry| entry.reading.as_str() <= reading);
    layer.entries[start..end].iter()
}

fn sort_entries(entries: &mut [DictionaryEntry]) {
    entries.sort_unstable_by(|left, right| {
        (
            &left.reading,
            left.word_cost,
            &left.surface,
            left.left_id,
            left.right_id,
        )
            .cmp(&(
                &right.reading,
                right.word_cost,
                &right.surface,
                right.left_id,
                right.right_id,
            ))
    });
}

fn reconstruct_connected_conversion(
    lattice: &[Vec<LatticeNode<'_>>],
    reading_length: usize,
    connection: ConnectionMatrix,
) -> Option<Conversion> {
    let (mut cursor, mut node_index, total_cost) = lattice[reading_length]
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (
                reading_length,
                index,
                node.total_cost
                    .saturating_add(connection.cost(node.right_id, BOS_EOS_POS_ID)),
            )
        })
        .min_by_key(|(_, _, cost)| *cost)?;

    let mut reversed = Vec::new();
    loop {
        let node = &lattice[cursor][node_index];
        reversed.push(Segment {
            reading: node.reading.to_owned(),
            surface: node.surface.to_owned(),
            cost: node.segment_cost,
        });
        let Some(predecessor) = node.predecessor else {
            break;
        };
        cursor = node.start;
        node_index = predecessor.get();
    }
    reversed.reverse();

    let surface = reversed
        .iter()
        .map(|segment| segment.surface.as_str())
        .collect();
    Some(Conversion {
        surface,
        segments: reversed,
        cost: total_cost,
    })
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::bundled()
    }
}

#[derive(Clone, Debug)]
struct Predecessor {
    start: usize,
    reading: String,
    surface: String,
    segment_cost: i32,
}

#[derive(Clone, Debug)]
struct LatticeNode<'a> {
    start: usize,
    predecessor: Option<NodeIndex>,
    reading: &'a str,
    surface: &'a str,
    segment_cost: i32,
    right_id: u16,
    total_cost: i32,
}

#[derive(Clone, Debug)]
struct NBestNode<'a> {
    start: usize,
    predecessor: Option<NodeIndex>,
    reading: &'a str,
    surface: &'a str,
    segment_cost: i32,
    right_id: u16,
    total_cost: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NodeIndex(NonZeroUsize);

impl NodeIndex {
    fn new(index: usize) -> Self {
        let encoded = index.checked_add(1).expect("node index overflow");
        Self(NonZeroUsize::new(encoded).expect("encoded node index is non-zero"))
    }

    fn get(self) -> usize {
        self.0.get() - 1
    }
}

const _: () = assert!(std::mem::size_of::<LatticeNode<'static>>() <= 64);
const _: () = assert!(std::mem::size_of::<NBestNode<'static>>() <= 64);

fn insert_connected_unknown<'a>(
    reading: &'a str,
    start: usize,
    predecessors: &[usize],
    arena: &mut Vec<NBestNode<'a>>,
    lattice: &mut [Vec<usize>],
    connection: ConnectionMatrix,
    limit: usize,
) {
    let Some(character) = reading[start..].chars().next() else {
        return;
    };
    let end = start + character.len_utf8();
    let literal = &reading[start..end];
    if start == 0 {
        let total_cost = connection
            .cost(BOS_EOS_POS_ID, UNKNOWN_POS_ID)
            .saturating_add(UNKNOWN_COST);
        insert_n_best_node(
            arena,
            &mut lattice[end],
            NBestNode {
                start,
                predecessor: None,
                reading: literal,
                surface: literal,
                segment_cost: UNKNOWN_COST,
                right_id: UNKNOWN_POS_ID,
                total_cost,
            },
            limit,
        );
        return;
    }

    let mut connection_cache = ConnectionCostCache::new(UNKNOWN_POS_ID);
    for &predecessor in predecessors {
        let previous = &arena[predecessor];
        let total_cost = previous
            .total_cost
            .saturating_add(connection_cache.cost(connection, previous.right_id))
            .saturating_add(UNKNOWN_COST);
        insert_n_best_node(
            arena,
            &mut lattice[end],
            NBestNode {
                start,
                predecessor: Some(NodeIndex::new(predecessor)),
                reading: literal,
                surface: literal,
                segment_cost: UNKNOWN_COST,
                right_id: UNKNOWN_POS_ID,
                total_cost,
            },
            limit,
        );
    }
}

fn insert_heuristic_unknown<'a>(
    reading: &'a str,
    start: usize,
    predecessors: &[usize],
    arena: &mut Vec<NBestNode<'a>>,
    lattice: &mut [Vec<usize>],
    limit: usize,
) {
    let Some(character) = reading[start..].chars().next() else {
        return;
    };
    let end = start + character.len_utf8();
    let literal = &reading[start..end];
    if start == 0 {
        insert_n_best_node(
            arena,
            &mut lattice[end],
            NBestNode {
                start,
                predecessor: None,
                reading: literal,
                surface: literal,
                segment_cost: UNKNOWN_COST,
                right_id: 0,
                total_cost: UNKNOWN_COST,
            },
            limit,
        );
        return;
    }

    for &predecessor in predecessors {
        let total_cost = arena[predecessor].total_cost.saturating_add(UNKNOWN_COST);
        insert_n_best_node(
            arena,
            &mut lattice[end],
            NBestNode {
                start,
                predecessor: Some(NodeIndex::new(predecessor)),
                reading: literal,
                surface: literal,
                segment_cost: UNKNOWN_COST,
                right_id: 0,
                total_cost,
            },
            limit,
        );
    }
}

/// Inserts one word (dictionary or synthetic) into the n-best lattice,
/// fanning out over every predecessor state at `start`.
#[allow(clippy::too_many_arguments)]
fn insert_connected_word<'a>(
    arena: &mut Vec<NBestNode<'a>>,
    states: &mut Vec<usize>,
    predecessors: &[usize],
    connection: ConnectionMatrix,
    start: usize,
    word_reading: &'a str,
    surface: &'a str,
    (left_id, right_id): (u16, u16),
    word_cost: i32,
    limit: usize,
) {
    if start == 0 {
        let total_cost = connection
            .cost(BOS_EOS_POS_ID, left_id)
            .saturating_add(word_cost);
        insert_n_best_node(
            arena,
            states,
            NBestNode {
                start,
                predecessor: None,
                reading: word_reading,
                surface,
                segment_cost: word_cost,
                right_id,
                total_cost,
            },
            limit,
        );
        return;
    }

    let mut connection_cache = ConnectionCostCache::new(left_id);
    for &predecessor in predecessors {
        let previous = &arena[predecessor];
        let total_cost = previous
            .total_cost
            .saturating_add(connection_cache.cost(connection, previous.right_id))
            .saturating_add(word_cost);
        insert_n_best_node(
            arena,
            states,
            NBestNode {
                start,
                predecessor: Some(NodeIndex::new(predecessor)),
                reading: word_reading,
                surface,
                segment_cost: word_cost,
                right_id,
                total_cost,
            },
            limit,
        );
    }
}

fn insert_n_best_node<'a>(
    arena: &mut Vec<NBestNode<'a>>,
    states: &mut Vec<usize>,
    candidate: NBestNode<'a>,
    limit_per_state: usize,
) {
    // Every target bucket is finalized before it becomes a predecessor. A
    // replacement can therefore reuse its arena slot without invalidating a
    // path which has already captured that index.
    let mut same_state_count = 0;
    let mut worst_same_state = None;
    let mut worst_global = None;
    for (position, &existing_index) in states.iter().enumerate() {
        let existing = &arena[existing_index];
        if existing.right_id == candidate.right_id
            && existing.start == candidate.start
            && existing.predecessor == candidate.predecessor
            && existing.reading == candidate.reading
            && existing.surface == candidate.surface
        {
            if candidate.total_cost < existing.total_cost {
                arena[existing_index] = candidate;
            }
            return;
        }

        if existing.right_id == candidate.right_id {
            same_state_count += 1;
            if worst_same_state.is_none_or(|(_, cost)| existing.total_cost >= cost) {
                worst_same_state = Some((position, existing.total_cost));
            }
        }
        if worst_global.is_none_or(|(_, cost)| existing.total_cost >= cost) {
            worst_global = Some((position, existing.total_cost));
        }
    }

    if same_state_count < limit_per_state {
        let beam_size = limit_per_state.saturating_mul(N_BEST_BEAM_FACTOR);
        if states.len() >= beam_size {
            let Some((worst_position, worst_cost)) = worst_global else {
                return;
            };
            if candidate.total_cost >= worst_cost {
                return;
            }
            let worst_index = states[worst_position];
            arena[worst_index] = candidate;
            return;
        }

        let index = arena.len();
        arena.push(candidate);
        states.push(index);
        return;
    }

    let Some((worst_position, worst_cost)) = worst_same_state else {
        return;
    };
    if candidate.total_cost < worst_cost {
        let worst_index = states[worst_position];
        arena[worst_index] = candidate;
    }
}

fn reconstruct_n_best_conversions(
    arena: &[NBestNode<'_>],
    completed: &[(usize, i32)],
    limit: usize,
) -> Vec<Conversion> {
    let mut conversions = Vec::with_capacity(limit);
    for &(last_node, total_cost) in completed {
        let mut reversed = Vec::new();
        let mut cursor = Some(last_node);
        while let Some(index) = cursor {
            let node = &arena[index];
            reversed.push(Segment {
                reading: node.reading.to_owned(),
                surface: node.surface.to_owned(),
                cost: node.segment_cost,
            });
            cursor = node.predecessor.map(NodeIndex::get);
        }
        reversed.reverse();
        let surface = reversed
            .iter()
            .map(|segment| segment.surface.as_str())
            .collect();
        if conversions
            .iter()
            .any(|conversion: &Conversion| conversion.surface == surface)
        {
            continue;
        }
        conversions.push(Conversion {
            surface,
            segments: reversed,
            cost: total_cost,
        });
        if conversions.len() == limit {
            break;
        }
    }
    conversions
}

fn best_connected_predecessor(
    lattice: &[Vec<LatticeNode<'_>>],
    start: usize,
    left_id: u16,
    connection: ConnectionMatrix,
) -> Option<(i32, Option<NodeIndex>)> {
    if start == 0 {
        return Some((connection.cost(BOS_EOS_POS_ID, left_id), None));
    }

    lattice[start]
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (
                node.total_cost
                    .saturating_add(connection.cost(node.right_id, left_id)),
                Some(NodeIndex::new(index)),
            )
        })
        .min_by_key(|(cost, _)| *cost)
}

fn cached_connected_predecessor(
    lattice: &[Vec<LatticeNode<'_>>],
    start: usize,
    left_id: u16,
    connection: ConnectionMatrix,
    cache: &mut Vec<(u16, i32, Option<NodeIndex>)>,
) -> Option<(i32, Option<NodeIndex>)> {
    if let Some((_, cost, predecessor)) = cache
        .iter()
        .find(|(cached_left_id, _, _)| *cached_left_id == left_id)
    {
        return Some((*cost, *predecessor));
    }

    let (cost, predecessor) = best_connected_predecessor(lattice, start, left_id, connection)?;
    cache.push((left_id, cost, predecessor));
    Some((cost, predecessor))
}

fn insert_lattice_node<'a>(nodes: &mut Vec<LatticeNode<'a>>, candidate: LatticeNode<'a>) {
    if let Some(existing) = nodes
        .iter_mut()
        .find(|node| node.right_id == candidate.right_id)
    {
        if candidate.total_cost < existing.total_cost {
            *existing = candidate;
        }
        return;
    }
    nodes.push(candidate);
}

#[derive(Clone, Copy, Debug)]
struct ConnectionMatrix {
    bytes: &'static [u8],
    size: usize,
    offsets_start: usize,
    modes_start: usize,
    entries_start: usize,
}

struct ConnectionCostCache {
    left_id: u16,
    right_ids: [u16; 16],
    costs: [i32; 16],
}

impl ConnectionCostCache {
    fn new(left_id: u16) -> Self {
        Self {
            left_id,
            right_ids: [u16::MAX; 16],
            costs: [0; 16],
        }
    }

    fn cost(&mut self, connection: ConnectionMatrix, right_id: u16) -> i32 {
        let slot = usize::from(right_id) & (self.right_ids.len() - 1);
        if self.right_ids[slot] != right_id {
            self.right_ids[slot] = right_id;
            self.costs[slot] = connection.cost(right_id, self.left_id);
        }
        self.costs[slot]
    }
}

impl ConnectionMatrix {
    fn bundled() -> Self {
        let bytes = include_bytes!("../data/mozc-connection.bin").as_slice();
        assert_eq!(&bytes[..4], b"UCN2", "connection matrix magic");
        let size = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
        let offsets_start = 8;
        let modes_start = offsets_start + (size + 1) * 4;
        let entries_start = modes_start + size * 2;
        Self {
            bytes,
            size,
            offsets_start,
            modes_start,
            entries_start,
        }
    }

    fn cost(self, right_id: u16, left_id: u16) -> i32 {
        let right = usize::from(right_id);
        let left = usize::from(left_id);
        if right >= self.size || left >= self.size {
            return INVALID_CONNECTION_COST;
        }

        let mut low = self.offset(right);
        let mut high = self.offset(right + 1);
        while low < high {
            let middle = low + (high - low) / 2;
            let entry_offset = self.entries_start + middle * 4;
            let entry_left = usize::from(u16::from_le_bytes([
                self.bytes[entry_offset],
                self.bytes[entry_offset + 1],
            ]));
            match entry_left.cmp(&left) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    return i32::from(u16::from_le_bytes([
                        self.bytes[entry_offset + 2],
                        self.bytes[entry_offset + 3],
                    ]));
                }
            }
        }

        let mode_offset = self.modes_start + right * 2;
        i32::from(u16::from_le_bytes([
            self.bytes[mode_offset],
            self.bytes[mode_offset + 1],
        ]))
    }

    fn offset(self, row: usize) -> usize {
        let offset = self.offsets_start + row * 4;
        u32::from_le_bytes([
            self.bytes[offset],
            self.bytes[offset + 1],
            self.bytes[offset + 2],
            self.bytes[offset + 3],
        ]) as usize
    }
}

#[allow(clippy::too_many_arguments)]
fn update_path(
    best_cost: &mut [i32],
    previous: &mut [Option<Predecessor>],
    start: usize,
    end: usize,
    total_cost: i32,
    reading: &str,
    surface: &str,
    segment_cost: i32,
) {
    if total_cost >= best_cost[end] {
        return;
    }

    best_cost[end] = total_cost;
    previous[end] = Some(Predecessor {
        start,
        reading: reading.to_owned(),
        surface: surface.to_owned(),
        segment_cost,
    });
}

const UNKNOWN_COST: i32 = 10_000;
const LITERAL_CANDIDATE_COST: i32 = i32::MAX;
const SEGMENT_PENALTY: i32 = 1_000;
const DEFAULT_N_BEST: usize = 10;
const N_BEST_BEAM_FACTOR: usize = 8;
const CANDIDATE_COST_PER_CHARACTER: i32 = 2_000;
const MINIMUM_CANDIDATE_COST_WINDOW: i32 = 6_000;
const MULTI_SEGMENT_COST_WINDOW: i32 = 2_500;
const INVALID_CONNECTION_COST: i32 = 30_000;
const BOS_EOS_POS_ID: u16 = 0;
const UNKNOWN_POS_ID: u16 = 1851;
const ARABIC_NUMBER_POS_ID: u16 = 2044;
const KANJI_NUMBER_POS_ID: u16 = 2051;
const NUMBER_VARIANT_STEP: i32 = 50;
const KATAKANA_RUN_MAX_CHARACTERS: usize = 12;

fn n_best_arena_capacity(reading: &str, limit: usize) -> usize {
    reading
        .chars()
        .count()
        .saturating_mul(limit.min(DEFAULT_N_BEST))
        .saturating_mul(N_BEST_BEAM_FACTOR)
}

fn katakana_run_base_cost() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("IME_KATAKANA_BASE", 1_000))
}

fn katakana_run_character_cost() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("IME_KATAKANA_PER_CHAR", 4_000))
}

fn number_cost() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("IME_NUMBER_COST", 2_000))
}

/// Evaluation-only override hook so cost sweeps do not need a rebuild; the
/// defaults are the tuned production values.
fn tuning_parameter(name: &str, default: i32) -> i32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn candidate_cost_window(reading: &str) -> i32 {
    let character_count = i32::try_from(reading.chars().count()).unwrap_or(i32::MAX);
    character_count
        .saturating_mul(CANDIDATE_COST_PER_CHARACTER)
        .max(MINIMUM_CANDIDATE_COST_WINDOW)
}

fn is_grammar_literal(reading: &str) -> bool {
    matches!(
        reading,
        "は" | "を"
            | "が"
            | "に"
            | "へ"
            | "と"
            | "で"
            | "の"
            | "も"
            | "や"
            | "か"
            | "ね"
            | "よ"
            | "する"
            | "ある"
            | "いる"
            | "なる"
            | "ない"
            | "たい"
            | "です"
            | "ます"
            | "ため"
            | "よう"
            | "こと"
            | "もの"
            | "これ"
            | "それ"
            | "ここ"
            | "そこ"
            | "ので"
            | "から"
            | "まで"
    )
}

/// A lattice node generated at runtime instead of coming from the dictionary:
/// composed numerals (せんきゅうひゃく → 1900) and katakana runs for unknown
/// foreign words. `end` is the absolute byte offset where the node stops.
#[derive(Clone, Debug)]
struct SyntheticEntry<'a> {
    end: usize,
    surface: &'a str,
    left_id: u16,
    right_id: u16,
    cost: i32,
}

fn synthetic_entries_by_start<'a>(reading: &str, arena: &'a Bump) -> Vec<Vec<SyntheticEntry<'a>>> {
    let mut by_start: Vec<Vec<SyntheticEntry>> = (0..=reading.len()).map(|_| Vec::new()).collect();
    for (start, _) in reading.char_indices() {
        push_number_entries(reading, start, arena, &mut by_start[start]);
        push_katakana_entries(reading, start, arena, &mut by_start[start]);
    }
    by_start
}

#[derive(Clone, Copy)]
enum NumberToken {
    Digit(u64),
    /// A sokuon digit form (いっ, はっ, ろっ) that is only a numeral when a
    /// positional unit follows: いっせん is 1000, but いった is 行った.
    SokuonDigit(u64),
    Small(u64),
    Big(u64),
}

/// Longest-match kana numeral tokens. Single-character readings that are
/// overwhelmingly grammatical (に, し, く, ご) never form a number on their
/// own; they only contribute inside longer sequences.
const NUMBER_TOKENS: &[(&str, NumberToken)] = &[
    ("きゅう", NumberToken::Digit(9)),
    ("ぜろ", NumberToken::Digit(0)),
    ("れい", NumberToken::Digit(0)),
    ("いち", NumberToken::Digit(1)),
    ("さん", NumberToken::Digit(3)),
    ("よん", NumberToken::Digit(4)),
    ("なな", NumberToken::Digit(7)),
    ("しち", NumberToken::Digit(7)),
    ("はち", NumberToken::Digit(8)),
    ("ろく", NumberToken::Digit(6)),
    ("いっ", NumberToken::SokuonDigit(1)),
    ("はっ", NumberToken::SokuonDigit(8)),
    ("ろっ", NumberToken::SokuonDigit(6)),
    ("じゅっ", NumberToken::Small(10)),
    ("じゅう", NumberToken::Small(10)),
    ("ひゃく", NumberToken::Small(100)),
    ("びゃく", NumberToken::Small(100)),
    ("ぴゃく", NumberToken::Small(100)),
    ("せん", NumberToken::Small(1_000)),
    ("ぜん", NumberToken::Small(1_000)),
    ("まん", NumberToken::Big(10_000)),
    ("おく", NumberToken::Big(100_000_000)),
    ("に", NumberToken::Digit(2)),
    ("し", NumberToken::Digit(4)),
    ("ご", NumberToken::Digit(5)),
    ("く", NumberToken::Digit(9)),
];

const RISKY_SINGLE_NUMBER_READINGS: &[&str] = &["に", "し", "ご", "く", "ぜん", "じゅっ"];

/// Parses kana numeral prefixes of `suffix`. Returns every token boundary at
/// which the consumed prefix forms a complete number, with its value.
fn parse_kana_number_prefixes(suffix: &str) -> Vec<(usize, u64)> {
    let mut results = Vec::new();
    let mut consumed = 0_usize;
    let mut token_count = 0_usize;
    let mut total = 0_u64;
    let mut section = 0_u64;
    let mut pending = 0_u64;
    let mut pending_digits = 0_u32;
    let mut last_small_unit = u64::MAX;
    let mut awaiting_unit = false;
    let mut first_token: &str = "";

    while consumed < suffix.len() {
        let rest = &suffix[consumed..];
        let Some(&(text, token)) = NUMBER_TOKENS
            .iter()
            .find(|(text, _)| rest.starts_with(text))
        else {
            break;
        };
        if awaiting_unit && !matches!(token, NumberToken::Small(_) | NumberToken::Big(_)) {
            break;
        }
        match token {
            NumberToken::Digit(value) => {
                if pending_digits >= 15 {
                    break;
                }
                pending = pending * 10 + value;
                pending_digits += 1;
            }
            NumberToken::SokuonDigit(value) => {
                if pending != 0 || pending_digits != 0 {
                    break;
                }
                pending = value;
                pending_digits = 1;
                awaiting_unit = true;
            }
            NumberToken::Small(unit) => {
                // Positional units must strictly descend within a section
                // (千→百→十); せんぜん or じゅうじゅう is not a numeral.
                if pending_digits > 1 || pending >= 10 || unit >= last_small_unit {
                    break;
                }
                section += pending.max(1) * unit;
                pending = 0;
                pending_digits = 0;
                last_small_unit = unit;
                awaiting_unit = false;
            }
            NumberToken::Big(unit) => {
                if section + pending == 0 {
                    break;
                }
                total += (section + pending) * unit;
                section = 0;
                pending = 0;
                pending_digits = 0;
                last_small_unit = u64::MAX;
                awaiting_unit = false;
            }
        }
        consumed += text.len();
        token_count += 1;
        if token_count == 1 {
            first_token = text;
        }

        let single_and_risky =
            token_count == 1 && RISKY_SINGLE_NUMBER_READINGS.contains(&first_token);
        if !single_and_risky && !awaiting_unit {
            results.push((consumed, total + section + pending));
        }
    }
    results
}

fn to_fullwidth_digits(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '0'..='9' => char::from_u32(u32::from(character) - u32::from('0') + u32::from('０'))
                .expect("valid fullwidth digit"),
            _ => character,
        })
        .collect()
}

fn kanji_numeral(mut value: u64) -> String {
    const DIGITS: [&str; 10] = ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    if value == 0 {
        return "〇".to_owned();
    }
    let mut groups = Vec::new();
    while value > 0 {
        groups.push(value % 10_000);
        value /= 10_000;
    }
    let mut result = String::new();
    for (index, &group) in groups.iter().enumerate().rev() {
        if group == 0 {
            continue;
        }
        let mut group_text = String::new();
        for (unit, unit_text) in [(1_000, "千"), (100, "百"), (10, "十"), (1, "")] {
            let digit = (group / unit) % 10;
            if digit == 0 {
                continue;
            }
            // 千万 reads as 一千万; the leading 一 is customary before 千 in
            // the 万-and-above groups but not in the lowest one (千円).
            if digit > 1 || unit == 1 || (unit == 1_000 && index > 0) {
                group_text.push_str(DIGITS[usize::try_from(digit).expect("digit fits usize")]);
            }
            group_text.push_str(unit_text);
        }
        result.push_str(&group_text);
        match index {
            0 => {}
            1 => result.push('万'),
            2 => result.push('億'),
            _ => result.push('兆'),
        }
    }
    result
}

/// Formats large values the way IMEs usually present them: arabic digits per
/// 万-group with kanji unit markers (10000000 → 1000万, 123450000 → 1億2345万).
fn mixed_numeral(value: u64) -> Option<String> {
    if value < 10_000 {
        return None;
    }
    let mut groups = Vec::new();
    let mut remainder = value;
    while remainder > 0 {
        groups.push(remainder % 10_000);
        remainder /= 10_000;
    }
    let mut result = String::new();
    for (index, &group) in groups.iter().enumerate().rev() {
        if group == 0 {
            continue;
        }
        result.push_str(&group.to_string());
        result.push_str(match index {
            0 => "",
            1 => "万",
            2 => "億",
            _ => "兆",
        });
    }
    Some(result)
}

fn push_number_entries<'a>(
    reading: &str,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    for (length, value) in parse_kana_number_prefixes(&reading[start..]) {
        let arabic = value.to_string();
        if let Some(mixed) = mixed_numeral(value) {
            out.push(SyntheticEntry {
                end: start + length,
                surface: arena.alloc_str(&mixed),
                left_id: ARABIC_NUMBER_POS_ID,
                right_id: ARABIC_NUMBER_POS_ID,
                cost: number_cost() - NUMBER_VARIANT_STEP,
            });
        }
        out.push(SyntheticEntry {
            end: start + length,
            surface: arena.alloc_str(&to_fullwidth_digits(&arabic)),
            left_id: ARABIC_NUMBER_POS_ID,
            right_id: ARABIC_NUMBER_POS_ID,
            cost: number_cost() + NUMBER_VARIANT_STEP,
        });
        out.push(SyntheticEntry {
            end: start + length,
            surface: arena.alloc_str(&kanji_numeral(value)),
            left_id: KANJI_NUMBER_POS_ID,
            right_id: KANJI_NUMBER_POS_ID,
            cost: number_cost() + 2 * NUMBER_VARIANT_STEP,
        });
        out.push(SyntheticEntry {
            end: start + length,
            surface: arena.alloc_str(&arabic),
            left_id: ARABIC_NUMBER_POS_ID,
            right_id: ARABIC_NUMBER_POS_ID,
            cost: number_cost(),
        });
    }
}

fn is_katakana_run_character(character: char) -> bool {
    matches!(character, 'ぁ'..='ゖ' | 'ー')
}

fn push_katakana_entries<'a>(
    reading: &str,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    let mut surface = BumpString::with_capacity_in(reading.len() - start, arena);
    let mut characters = 0_usize;
    for (offset, character) in reading[start..].char_indices() {
        if !is_katakana_run_character(character) || characters == KATAKANA_RUN_MAX_CHARACTERS {
            break;
        }
        surface.push(match character {
            'ぁ'..='ゖ' => {
                char::from_u32(u32::from(character) + 0x60).expect("valid katakana scalar")
            }
            other => other,
        });
        characters += 1;
        if characters >= 2 {
            out.push(SyntheticEntry {
                end: start + offset + character.len_utf8(),
                surface: arena.alloc_str(surface.as_str()),
                left_id: UNKNOWN_POS_ID,
                right_id: UNKNOWN_POS_ID,
                cost: katakana_run_base_cost()
                    + katakana_run_character_cost()
                        * i32::try_from(characters).expect("run length fits i32"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateRanker, ConnectionCostCache, ConnectionMatrix, Conversion, Dictionary,
        DictionaryEntry, DictionaryLayer, UNKNOWN_POS_ID,
    };

    struct PreferSurface<'a>(&'a str);

    impl CandidateRanker for PreferSurface<'_> {
        fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
            if conversion.surface == self.0 {
                i32::MIN
            } else {
                conversion.cost
            }
        }
    }

    #[test]
    fn connection_cost_cache_handles_direct_map_collisions() {
        let connection = ConnectionMatrix::bundled();
        let mut cache = ConnectionCostCache::new(100);

        for right_id in [0, 16, 0] {
            assert_eq!(
                cache.cost(connection, right_id),
                connection.cost(right_id, 100)
            );
        }
    }

    #[test]
    fn exact_candidates_are_ordered_by_cost() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates("にほん");

        assert_eq!(candidates[0].surface, "日本");
        assert_eq!(candidates[1].surface, "ニホン");
        assert_eq!(candidates[2].surface, "二本");
        assert_eq!(candidates.last().unwrap().surface, "にほん");
    }

    #[test]
    fn unconverted_reading_stays_after_long_conversion_paths() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates("わたしはにほん");

        assert_eq!(candidates[0].surface, "私は日本");
        assert_eq!(candidates.last().unwrap().surface, "わたしはにほん");
    }

    #[test]
    fn viterbi_selects_best_segmented_path() {
        let dictionary = Dictionary::bundled();
        let conversion = dictionary.convert_best("わたしはにほん").unwrap();

        assert_eq!(conversion.surface, "私は日本");
        assert_eq!(conversion.segments.len(), 3);
    }

    #[test]
    fn phrase_entry_resolves_semantically_ambiguous_noun() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.convert_best("はしでたべる").unwrap().surface,
            "箸で食べる"
        );
    }

    #[test]
    fn n_best_keeps_semantically_ambiguous_segmented_paths() {
        let dictionary = Dictionary::bundled();
        let conversions = dictionary.convert_n_best("はしでたべる", 10);
        let surfaces: Vec<_> = conversions
            .iter()
            .map(|conversion| conversion.surface.as_str())
            .collect();

        assert!(surfaces.contains(&"橋で食べる"), "surfaces: {surfaces:?}");
        assert!(surfaces.contains(&"箸で食べる"), "surfaces: {surfaces:?}");
    }

    #[test]
    fn candidate_ranker_can_reorder_complete_n_best_paths() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あ", "亜", 10),
            DictionaryEntry::new("あ", "阿", 20),
            DictionaryEntry::new("い", "伊", 10),
        ]);

        let candidates = dictionary.candidates_with_ranker("あい", 5, &PreferSurface("阿伊"));

        assert_eq!(candidates[0].surface, "阿伊");
    }

    #[test]
    fn unknown_input_converts_to_katakana_and_keeps_the_literal_reading() {
        let dictionary = Dictionary::bundled();
        let conversion = dictionary.convert_best("ゑゑ").unwrap();
        assert_eq!(conversion.surface, "ヱヱ");

        let candidates = dictionary.candidates("ゑゑ");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.surface == "ゑゑ")
        );
    }

    #[test]
    fn input_longer_than_every_dictionary_entry_still_converts_completely() {
        let dictionary = Dictionary::bundled();
        let reading = "ゑ".repeat(100);
        let conversion = dictionary.convert_best(&reading).unwrap();

        assert_eq!(conversion.surface, "ヱ".repeat(100));
        let reconstructed: String = conversion
            .segments
            .iter()
            .map(|segment| segment.reading.as_str())
            .collect();
        assert_eq!(reconstructed, reading);
    }

    #[test]
    fn kana_number_readings_compose_into_numerals() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates("せんきゅうひゃくきゅうじゅういちねん");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.surface == "1991年"),
            "candidates: {candidates:?}"
        );

        assert_eq!(super::kanji_numeral(1_991), "千九百九十一");
        assert_eq!(super::kanji_numeral(45), "四十五");
        assert_eq!(super::kanji_numeral(30_005), "三万五");
        assert_eq!(super::kanji_numeral(10_000_000), "一千万");
        assert_eq!(super::to_fullwidth_digits("45"), "４５");
        assert_eq!(super::mixed_numeral(10_000_000).as_deref(), Some("1000万"));
        assert_eq!(
            super::mixed_numeral(123_450_000).as_deref(),
            Some("1億2345万")
        );
        assert_eq!(super::mixed_numeral(1_991), None);
    }

    #[test]
    fn sokuon_digit_readings_compose_only_before_units() {
        let dictionary = Dictionary::bundled();
        for (reading, expected) in [
            ("いっせんまん", "1000万"),
            ("いっせんまん", "一千万"),
            ("はっぴゃく", "800"),
            ("ろっぴゃくえん", "600円"),
        ] {
            assert!(
                dictionary
                    .candidates(reading)
                    .iter()
                    .any(|candidate| candidate.surface == expected),
                "missing {expected} for {reading}"
            );
        }

        // いった must stay 行った; the sokuon form alone is not a numeral.
        let candidates = dictionary.candidates("いった");
        assert_eq!(candidates[0].surface, "行った");
        assert!(
            candidates
                .iter()
                .all(|candidate| !candidate.surface.contains('1'))
        );
    }

    #[test]
    fn segment_penalty_avoids_over_segmenting_a_reading() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あ", "亜", 10),
            DictionaryEntry::new("い", "伊", 10),
            DictionaryEntry::new("あい", "愛", 30),
        ]);

        assert_eq!(dictionary.convert_best("あい").unwrap().surface, "愛");
    }

    #[test]
    fn empty_input_has_no_conversion() {
        assert!(Dictionary::bundled().convert_best("").is_none());
    }

    #[test]
    fn bundled_dictionary_contains_a_practical_basic_vocabulary() {
        let dictionary = Dictionary::bundled();

        assert!(dictionary.entry_count() >= 170_000);
        for (reading, surface) in [
            ("かんじ", "漢字"),
            ("へんかん", "変換"),
            ("にゅうりょく", "入力"),
            ("どうさ", "動作"),
            ("こまる", "困る"),
            ("じしょ", "辞書"),
            ("かくじゅう", "拡充"),
            ("きごう", "記号"),
            ("ぜんかく", "全角"),
            ("こんぴゅーたー", "コンピューター"),
            ("きーぼーど", "キーボード"),
            ("でーたべーす", "データベース"),
        ] {
            assert!(
                dictionary
                    .candidates(reading)
                    .iter()
                    .any(|candidate| candidate.surface == surface),
                "missing candidate: {reading} -> {surface}"
            );
        }

        assert_eq!(dictionary.candidates("かんじ")[0].surface, "漢字");
    }

    #[test]
    fn additional_dictionary_layers_participate_in_exact_and_phrase_conversion() {
        let layer = DictionaryLayer::new(
            "technology",
            "技術用語",
            vec![DictionaryEntry::with_pos(
                "らすとげんご",
                "Rust言語",
                UNKNOWN_POS_ID,
                UNKNOWN_POS_ID,
                500,
            )],
        );
        let dictionary = Dictionary::bundled_with_layers(vec![layer]);

        assert_eq!(dictionary.layer_count(), 2);
        assert_eq!(dictionary.candidates("らすとげんご")[0].surface, "Rust言語");
        assert_eq!(
            dictionary
                .convert_best("らすとげんごをつかう")
                .unwrap()
                .surface,
            "Rust言語を使う"
        );
    }
}
