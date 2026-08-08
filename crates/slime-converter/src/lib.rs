//! A small, deterministic kana-kanji conversion baseline backed by a compact
//! dictionary.

mod compact;
mod ranking;
mod symbol_candidates;

pub use ranking::{CandidateRanker, CostOnlyRanker};

use bumpalo::{Bump, collections::String as BumpString};
use compact::CompactDictionary;
use compact_str::CompactString;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryEntry {
    pub reading: CompactString,
    pub surface: CompactString,
    pub left_id: u16,
    pub right_id: u16,
    pub word_cost: i32,
}

const _: () = assert!(std::mem::size_of::<DictionaryEntry>() <= 64);

impl DictionaryEntry {
    #[must_use]
    pub fn new(
        reading: impl Into<CompactString>,
        surface: impl Into<CompactString>,
        word_cost: i32,
    ) -> Self {
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
        reading: impl Into<CompactString>,
        surface: impl Into<CompactString>,
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

#[derive(Clone, Debug)]
pub struct DictionaryLayer {
    id: CompactString,
    name: CompactString,
    entries: Arc<[DictionaryEntry]>,
    max_reading_bytes: usize,
}

impl DictionaryLayer {
    #[must_use]
    pub fn new(
        id: impl Into<CompactString>,
        name: impl Into<CompactString>,
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

#[derive(Clone, Debug)]
struct CompoundPath {
    surface: String,
    cost: i32,
    right_id: u16,
}

#[derive(Clone, Debug)]
struct FixedSegmentPath {
    surface: String,
    changed_segments: usize,
    relative_cost: i64,
}

const COMPOUND_MAX_SEGMENTS: usize = 6;
const COMPOUND_MAX_READING_CHARACTERS: usize = 16;
const COMPOUND_MAX_ENTRIES_PER_SEGMENT: usize = 8;
const COMPOUND_MAX_CANDIDATES: usize = 64;
const FIXED_SEGMENT_MAX_READING_CHARACTERS: usize = 128;
const FIXED_SEGMENT_MAX_SEGMENTS: usize = 64;
const FIXED_SEGMENT_MAX_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_MAX_CANDIDATES: usize = 128;
const FIXED_SEGMENT_MAX_STATES: usize = 256;

fn trim_compound_paths(paths: &mut Vec<CompoundPath>, limit: usize) {
    paths.sort_unstable_by(|left, right| {
        left.surface
            .cmp(&right.surface)
            .then_with(|| left.right_id.cmp(&right.right_id))
            .then_with(|| left.cost.cmp(&right.cost))
    });
    paths.dedup_by(|left, right| left.surface == right.surface && left.right_id == right.right_id);
    paths.sort_unstable_by(|left, right| {
        left.cost
            .cmp(&right.cost)
            .then_with(|| left.surface.cmp(&right.surface))
            .then_with(|| left.right_id.cmp(&right.right_id))
    });
    paths.truncate(limit);
}

fn trim_fixed_segment_paths(paths: &mut Vec<FixedSegmentPath>, limit: usize) {
    paths.sort_unstable_by(|left, right| {
        left.surface
            .cmp(&right.surface)
            .then(left.changed_segments.cmp(&right.changed_segments))
            .then(left.relative_cost.cmp(&right.relative_cost))
    });
    paths.dedup_by(|left, right| left.surface == right.surface);
    paths.sort_unstable_by(|left, right| {
        left.changed_segments
            .cmp(&right.changed_segments)
            .then(left.relative_cost.cmp(&right.relative_cost))
            .then(left.surface.cmp(&right.surface))
    });
    paths.truncate(limit);
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

    #[must_use]
    pub fn has_exact_reading(&self, reading: &str) -> bool {
        let mut found = false;
        self.for_each_exact(reading, |_| found = true);
        found
    }

    /// Returns low-cost two- to six-part compounds assembled from exact
    /// dictionary entries. This is a bounded recall path for explicit "more
    /// candidates" actions; it does not replace the normal N-best ordering.
    #[must_use]
    pub fn compound_candidates(
        &self,
        reading: &str,
        entries_per_segment: usize,
        limit: usize,
    ) -> Vec<Candidate> {
        let character_count = reading.chars().count();
        if entries_per_segment == 0
            || limit == 0
            || !(4..=COMPOUND_MAX_READING_CHARACTERS).contains(&character_count)
        {
            return Vec::new();
        }

        let entries_per_segment = entries_per_segment.min(COMPOUND_MAX_ENTRIES_PER_SEGMENT);
        let limit = limit.min(COMPOUND_MAX_CANDIDATES);
        let state_limit = limit
            .saturating_mul(entries_per_segment)
            .min(COMPOUND_MAX_CANDIDATES * COMPOUND_MAX_ENTRIES_PER_SEGMENT);
        let connection = self.uses_connection_costs.then(ConnectionMatrix::bundled);
        let mut boundaries = reading
            .char_indices()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        boundaries.push(reading.len());
        let final_position = boundaries.len() - 1;
        let mut states =
            vec![vec![Vec::<CompoundPath>::new(); boundaries.len()]; COMPOUND_MAX_SEGMENTS + 1];
        states[0][0].push(CompoundPath {
            surface: String::new(),
            cost: 0,
            right_id: BOS_EOS_POS_ID,
        });

        for segment_count in 0..COMPOUND_MAX_SEGMENTS {
            for start_position in 0..final_position {
                let preceding = states[segment_count][start_position].clone();
                if preceding.is_empty() {
                    continue;
                }
                for end_position in (start_position + 1)..=final_position {
                    let segment_reading =
                        &reading[boundaries[start_position]..boundaries[end_position]];
                    let entries = self.exact_compound_entries(segment_reading, entries_per_segment);
                    if entries.is_empty() {
                        continue;
                    }

                    let destination = &mut states[segment_count + 1][end_position];
                    for path in &preceding {
                        for entry in &entries {
                            let mut surface = String::with_capacity(
                                path.surface.len().saturating_add(entry.surface.len()),
                            );
                            surface.push_str(&path.surface);
                            surface.push_str(entry.surface);
                            let transition_cost = connection
                                .map_or(0, |matrix| matrix.cost(path.right_id, entry.left_id));
                            destination.push(CompoundPath {
                                surface,
                                cost: path
                                    .cost
                                    .saturating_add(transition_cost)
                                    .saturating_add(entry.word_cost),
                                right_id: entry.right_id,
                            });
                        }
                    }
                    trim_compound_paths(destination, state_limit);
                }
            }
        }

        let mut candidates = Vec::<Candidate>::new();
        for paths in states
            .iter()
            .take(COMPOUND_MAX_SEGMENTS + 1)
            .skip(2)
            .map(|segments| &segments[final_position])
        {
            for path in paths {
                if path.surface == reading {
                    continue;
                }
                let cost = path.cost.saturating_add(
                    connection.map_or(0, |matrix| matrix.cost(path.right_id, BOS_EOS_POS_ID)),
                );
                if let Some(existing) = candidates
                    .iter_mut()
                    .find(|candidate| candidate.surface == path.surface)
                {
                    existing.cost = existing.cost.min(cost);
                } else {
                    candidates.push(Candidate {
                        surface: path.surface.clone(),
                        cost,
                    });
                }
            }
        }
        candidates.sort_unstable_by(|left, right| {
            left.cost
                .cmp(&right.cost)
                .then_with(|| left.surface.cmp(&right.surface))
        });
        candidates.truncate(limit);
        candidates
    }

    /// Reports whether `surface` can be aligned to two to six exact dictionary
    /// entries over `reading`, without applying the product candidate beam.
    ///
    /// This offline diagnostic distinguishes missing component vocabulary from
    /// a known-component phrase that needs stronger phrase knowledge. It uses
    /// the same literal-segment eligibility as [`Self::compound_candidates`]
    /// but does not rank or return alternative surfaces.
    #[must_use]
    pub fn is_exact_compound_surface(&self, reading: &str, surface: &str) -> bool {
        let character_count = reading.chars().count();
        if surface.is_empty() || !(4..=COMPOUND_MAX_READING_CHARACTERS).contains(&character_count) {
            return false;
        }

        let mut boundaries = reading
            .char_indices()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        boundaries.push(reading.len());
        let final_position = boundaries.len() - 1;
        let mut states =
            vec![vec![Vec::<usize>::new(); boundaries.len()]; COMPOUND_MAX_SEGMENTS + 1];
        states[0][0].push(0);

        for segment_count in 0..COMPOUND_MAX_SEGMENTS {
            for start_position in 0..final_position {
                let surface_positions = states[segment_count][start_position].clone();
                if surface_positions.is_empty() {
                    continue;
                }
                for end_position in (start_position + 1)..=final_position {
                    let segment_reading =
                        &reading[boundaries[start_position]..boundaries[end_position]];
                    let entries = self.exact_compound_entries(segment_reading, usize::MAX);
                    if entries.is_empty() {
                        continue;
                    }
                    let destination = &mut states[segment_count + 1][end_position];
                    for &surface_position in &surface_positions {
                        let Some(remaining_surface) = surface.get(surface_position..) else {
                            continue;
                        };
                        for entry in &entries {
                            if remaining_surface.starts_with(entry.surface) {
                                let next_position = surface_position + entry.surface.len();
                                if !destination.contains(&next_position) {
                                    destination.push(next_position);
                                }
                            }
                        }
                    }
                }
            }
        }

        states
            .iter()
            .take(COMPOUND_MAX_SEGMENTS + 1)
            .skip(2)
            .any(|segments| segments[final_position].contains(&surface.len()))
    }

    /// Returns alternatives that preserve the best path's segment boundaries.
    ///
    /// This is a bounded recall path for explicit "more candidates" actions on
    /// long readings. It avoids a wider whole-reading N-best search by changing
    /// only the candidate surface inside each already-selected segment.
    #[must_use]
    pub fn fixed_segment_variants(
        &self,
        reading: &str,
        entries_per_segment: usize,
        limit: usize,
    ) -> Vec<String> {
        let character_count = reading.chars().count();
        if entries_per_segment == 0
            || limit == 0
            || character_count > FIXED_SEGMENT_MAX_READING_CHARACTERS
        {
            return Vec::new();
        }
        let Some(best) = self.convert_best(reading) else {
            return Vec::new();
        };
        if !(2..=FIXED_SEGMENT_MAX_SEGMENTS).contains(&best.segments.len()) {
            return Vec::new();
        }

        let entries_per_segment = entries_per_segment.min(FIXED_SEGMENT_MAX_ENTRIES_PER_SEGMENT);
        let limit = limit.min(FIXED_SEGMENT_MAX_CANDIDATES);
        let state_limit = limit
            .saturating_mul(entries_per_segment)
            .min(FIXED_SEGMENT_MAX_STATES);
        let unchanged_surface = best.surface;
        let mut states = vec![FixedSegmentPath {
            surface: String::new(),
            changed_segments: 0,
            relative_cost: 0,
        }];

        for segment in best.segments {
            let mut alternatives = self
                .candidates_with_limit(&segment.reading, entries_per_segment)
                .into_iter()
                .take(entries_per_segment)
                .filter(|candidate| candidate.surface != segment.reading)
                .map(|candidate| (candidate.surface, i64::from(candidate.cost)))
                .collect::<Vec<_>>();
            alternatives
                .sort_unstable_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
            alternatives.dedup_by(|left, right| left.0 == right.0);
            alternatives
                .sort_unstable_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
            let minimum_cost = alternatives
                .iter()
                .filter(|(surface, _)| surface != &segment.surface)
                .map(|(_, cost)| *cost)
                .min()
                .unwrap_or(0);
            if !alternatives
                .iter()
                .any(|(surface, _)| surface == &segment.surface)
            {
                alternatives.push((segment.surface.clone(), minimum_cost));
            }

            let mut next = Vec::with_capacity(states.len().saturating_mul(alternatives.len()));
            for state in &states {
                for (surface, cost) in &alternatives {
                    let changed = surface != &segment.surface;
                    let mut combined =
                        String::with_capacity(state.surface.len().saturating_add(surface.len()));
                    combined.push_str(&state.surface);
                    combined.push_str(surface);
                    next.push(FixedSegmentPath {
                        surface: combined,
                        changed_segments: state.changed_segments + usize::from(changed),
                        relative_cost: state.relative_cost.saturating_add(if changed {
                            cost.saturating_sub(minimum_cost)
                        } else {
                            0
                        }),
                    });
                }
            }
            trim_fixed_segment_paths(&mut next, state_limit);
            states = next;
        }

        states.retain(|state| state.surface != unchanged_surface);
        trim_fixed_segment_paths(&mut states, limit);
        states.into_iter().map(|state| state.surface).collect()
    }

    fn exact_compound_entries<'s>(&'s self, reading: &str, limit: usize) -> Vec<EntryView<'s>> {
        let mut entries = Vec::new();
        let mut literal_entries = Vec::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading {
                literal_entries.push(entry);
            } else {
                entries.push(entry);
            }
        });
        // A dictionary-backed kana-only segment can connect names, particles,
        // and content words in productive compounds. Use it only when that
        // segment has no converted surface, so literal variants cannot evict
        // useful converted entries from the per-segment bound.
        if entries.is_empty() {
            entries = literal_entries;
        }
        entries.sort_unstable_by(|left, right| {
            left.surface
                .cmp(right.surface)
                .then_with(|| left.left_id.cmp(&right.left_id))
                .then_with(|| left.right_id.cmp(&right.right_id))
                .then_with(|| left.word_cost.cmp(&right.word_cost))
        });
        entries.dedup_by(|left, right| {
            left.surface == right.surface
                && left.left_id == right.left_id
                && left.right_id == right.right_id
        });
        entries.sort_unstable_by(|left, right| {
            left.word_cost
                .cmp(&right.word_cost)
                .then_with(|| left.surface.cmp(right.surface))
                .then_with(|| left.left_id.cmp(&right.left_id))
                .then_with(|| left.right_id.cmp(&right.right_id))
        });
        entries.truncate(limit);
        entries
    }

    /// Returns exact dictionary readings for a committed surface, ordered by
    /// word cost. This lookup is used only for explicit reconversion.
    #[must_use]
    pub fn readings_for_surface(&self, surface: &str) -> Vec<String> {
        let mut readings = self
            .bundled
            .map_or_else(Vec::new, |compact| compact.readings_for_surface(surface));
        for layer in self.layers.iter() {
            for entry in layer
                .entries
                .iter()
                .filter(|entry| entry.surface == surface)
            {
                readings.push((entry.reading.to_string(), entry.word_cost));
            }
        }
        readings.sort_unstable_by(|left, right| (left.1, &left.0).cmp(&(right.1, &right.0)));
        let mut seen = HashSet::with_capacity(readings.len());
        readings.retain(|(reading, _)| seen.insert(reading.clone()));
        readings.into_iter().map(|(reading, _)| reading).collect()
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

    /// Returns cost-ranked candidates using an explicit N-best search width.
    ///
    /// Interactive callers can keep the default search on the initial key
    /// path, then request a wider search only after the user reaches the end
    /// of the visible candidates.
    #[must_use]
    pub fn candidates_with_limit(&self, reading: &str, limit: usize) -> Vec<Candidate> {
        self.candidates_with_ranker(reading, limit, &CostOnlyRanker)
    }

    #[must_use]
    pub fn candidates_with_ranker(
        &self,
        reading: &str,
        limit: usize,
        ranker: &dyn CandidateRanker,
    ) -> Vec<Candidate> {
        self.candidates_with_context_ranker(reading, "", limit, ranker)
    }

    #[must_use]
    pub fn candidates_with_context_ranker(
        &self,
        reading: &str,
        left_context: &str,
        limit: usize,
        ranker: &dyn CandidateRanker,
    ) -> Vec<Candidate> {
        let mut candidates = Vec::<Candidate>::new();
        let mut conversions = Vec::new();
        let connection = self.uses_connection_costs.then(ConnectionMatrix::bundled);
        self.for_each_exact(reading, |entry| {
            let cost = if entry.surface == reading {
                LITERAL_CANDIDATE_COST
            } else {
                // Score exact whole-reading entries with their BOS/EOS
                // connection costs so they compare on the same scale as the
                // multi-segment lattice paths they are merged with below.
                whole_reading_entry_cost(connection, &entry)
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
                ranker.ranking_cost_with_context(reading, left_context, &conversion)
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
        symbol_candidates::append_for_reading(reading, &mut candidates);
        candidates
    }

    /// Returns numeral surfaces generated from the complete reading.
    ///
    /// Candidate UIs use this bounded query to identify generated numeric
    /// alternatives without duplicating the converter's numeral grammar or
    /// inferring provenance from the rendered surface.
    #[must_use]
    pub fn generated_number_surfaces(&self, reading: &str) -> Vec<String> {
        if reading.is_empty() {
            return Vec::new();
        }
        let arena = Bump::new();
        let mut entries = Vec::new();
        push_digit_run_entry(reading, 0, &mut entries);
        push_number_entries(reading, 0, &arena, &mut entries);
        let mut surfaces = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.end == reading.len()
                && !surfaces
                    .iter()
                    .any(|surface: &String| surface == entry.surface)
            {
                surfaces.push(entry.surface.to_owned());
            }
        }
        surfaces
    }

    /// Returns complete conversion paths ordered by their lattice cost.
    ///
    /// Unlike [`Self::convert_best`], this keeps multiple paths which arrive at
    /// the same part-of-speech state. Callers on latency-sensitive paths should
    /// request only the small limit they need; candidate windows use a wider
    /// search than live confidence checks.
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
        let mut lattice: Vec<NBestBucket> = (0..=reading.len())
            .map(|_| NBestBucket::default())
            .collect();

        for start in reading
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(reading.len()))
        {
            if start == reading.len() || (start > 0 && lattice[start].states.is_empty()) {
                continue;
            }
            let predecessors = lattice[start].states.clone();
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
            .states
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
        let mut lattice: Vec<NBestBucket> = (0..=reading.len())
            .map(|_| NBestBucket::default())
            .collect();

        for start in reading
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(reading.len()))
        {
            if start == reading.len() || (start > 0 && lattice[start].states.is_empty()) {
                continue;
            }
            let predecessors = lattice[start].states.clone();
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
            .states
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

#[derive(Debug, Default)]
struct NBestBucket {
    states: Vec<usize>,
    worst_total_cost: Option<i32>,
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
    lattice: &mut [NBestBucket],
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
    lattice: &mut [NBestBucket],
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
    states: &mut NBestBucket,
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
    bucket: &mut NBestBucket,
    candidate: NBestNode<'a>,
    limit_per_state: usize,
) {
    // Every target bucket is finalized before it becomes a predecessor. A
    // replacement can therefore reuse its arena slot without invalidating a
    // path which has already captured that index.
    let beam_size = limit_per_state.saturating_mul(N_BEST_BEAM_FACTOR);
    if bucket.states.len() >= beam_size
        && bucket
            .worst_total_cost
            .is_some_and(|worst_cost| candidate.total_cost >= worst_cost)
    {
        return;
    }

    let mut same_state_count = 0;
    let mut worst_same_state = None;
    let mut worst_global = None;
    for (position, &existing_index) in bucket.states.iter().enumerate() {
        let existing = &arena[existing_index];
        if existing.right_id == candidate.right_id
            && existing.start == candidate.start
            && existing.predecessor == candidate.predecessor
            && existing.reading == candidate.reading
            && existing.surface == candidate.surface
        {
            if candidate.total_cost < existing.total_cost {
                let replaced_worst = bucket.worst_total_cost == Some(existing.total_cost);
                arena[existing_index] = candidate;
                if replaced_worst {
                    refresh_worst_n_best_cost(arena, bucket);
                }
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
        if bucket.states.len() >= beam_size {
            let Some((worst_position, worst_cost)) = worst_global else {
                return;
            };
            if candidate.total_cost >= worst_cost {
                return;
            }
            let worst_index = bucket.states[worst_position];
            arena[worst_index] = candidate;
            refresh_worst_n_best_cost(arena, bucket);
            return;
        }

        let index = arena.len();
        let total_cost = candidate.total_cost;
        arena.push(candidate);
        bucket.states.push(index);
        bucket.worst_total_cost = Some(
            bucket
                .worst_total_cost
                .map_or(total_cost, |worst_cost| worst_cost.max(total_cost)),
        );
        return;
    }

    let Some((worst_position, worst_cost)) = worst_same_state else {
        return;
    };
    if candidate.total_cost < worst_cost {
        let worst_index = bucket.states[worst_position];
        let replaced_worst = bucket.worst_total_cost == Some(arena[worst_index].total_cost);
        arena[worst_index] = candidate;
        if replaced_worst {
            refresh_worst_n_best_cost(arena, bucket);
        }
    }
}

fn refresh_worst_n_best_cost(arena: &[NBestNode<'_>], bucket: &mut NBestBucket) {
    bucket.worst_total_cost = bucket
        .states
        .iter()
        .map(|&index| arena[index].total_cost)
        .max();
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
    *VALUE.get_or_init(|| tuning_parameter("SLIME_KATAKANA_BASE", 1_000))
}

fn katakana_run_character_cost() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("SLIME_KATAKANA_PER_CHAR", 4_000))
}

fn number_cost() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("SLIME_NUMBER_COST", 2_000))
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

/// Cost of one dictionary entry standing alone as the whole conversion:
/// BOS→entry→EOS when the connection matrix is in use, the raw word cost
/// otherwise.
fn whole_reading_entry_cost(connection: Option<ConnectionMatrix>, entry: &EntryView<'_>) -> i32 {
    match connection {
        Some(connection) => connection
            .cost(BOS_EOS_POS_ID, entry.left_id)
            .saturating_add(entry.word_cost)
            .saturating_add(connection.cost(entry.right_id, BOS_EOS_POS_ID)),
        None => entry.word_cost,
    }
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
/// digit runs, composed numerals (せんきゅうひゃく → 1900), and katakana runs
/// for unknown foreign words. `end` is the absolute byte offset where the node
/// stops.
#[derive(Clone, Debug)]
struct SyntheticEntry<'a> {
    end: usize,
    surface: &'a str,
    left_id: u16,
    right_id: u16,
    cost: i32,
}

fn synthetic_entries_by_start<'a>(
    reading: &'a str,
    arena: &'a Bump,
) -> Vec<Vec<SyntheticEntry<'a>>> {
    let mut by_start: Vec<Vec<SyntheticEntry>> = (0..=reading.len()).map(|_| Vec::new()).collect();
    for (start, _) in reading.char_indices() {
        push_digit_run_entry(reading, start, &mut by_start[start]);
        push_number_entries(reading, start, arena, &mut by_start[start]);
        push_katakana_entries(reading, start, arena, &mut by_start[start]);
    }
    by_start
}

fn push_digit_run_entry<'a>(reading: &'a str, start: usize, out: &mut Vec<SyntheticEntry<'a>>) {
    let mut end = start;
    for (offset, character) in reading[start..].char_indices() {
        if !matches!(character, '0'..='9' | '０'..='９') {
            break;
        }
        end = start + offset + character.len_utf8();
    }
    if end == start {
        return;
    }
    out.push(SyntheticEntry {
        end,
        surface: &reading[start..end],
        left_id: ARABIC_NUMBER_POS_ID,
        right_id: ARABIC_NUMBER_POS_ID,
        cost: number_cost(),
    });
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
        Candidate, CandidateRanker, ConnectionCostCache, ConnectionMatrix, Conversion, Dictionary,
        DictionaryEntry, DictionaryLayer, NBestBucket, NBestNode, UNKNOWN_POS_ID,
        insert_n_best_node,
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

    struct PreferSurfaceInContext<'a> {
        context: &'a str,
        surface: &'a str,
    }

    impl CandidateRanker for PreferSurfaceInContext<'_> {
        fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
            conversion.cost
        }

        fn ranking_cost_with_context(
            &self,
            _reading: &str,
            left_context: &str,
            conversion: &Conversion,
        ) -> i32 {
            if left_context == self.context && conversion.surface == self.surface {
                i32::MIN
            } else {
                conversion.cost
            }
        }
    }

    #[test]
    fn short_dictionary_entry_strings_stay_inline() {
        let entry = DictionaryEntry::new("かんじ", "漢字", 500);

        assert!(!entry.reading.is_heap_allocated());
        assert!(!entry.surface.is_heap_allocated());
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
    fn full_n_best_bucket_rejects_worse_costs_and_refreshes_its_bound() {
        let mut arena = Vec::new();
        let mut bucket = NBestBucket::default();
        for (right_id, total_cost, surface) in [
            (1, 10, "one"),
            (2, 20, "two"),
            (3, 30, "three"),
            (4, 40, "four"),
            (5, 50, "five"),
            (6, 60, "six"),
            (7, 70, "seven"),
            (8, 80, "eight"),
        ] {
            insert_n_best_node(
                &mut arena,
                &mut bucket,
                NBestNode {
                    start: 0,
                    predecessor: None,
                    reading: "a",
                    surface,
                    segment_cost: total_cost,
                    right_id,
                    total_cost,
                },
                1,
            );
        }

        assert_eq!(bucket.states.len(), 8);
        assert_eq!(bucket.worst_total_cost, Some(80));
        insert_n_best_node(
            &mut arena,
            &mut bucket,
            NBestNode {
                start: 0,
                predecessor: None,
                reading: "a",
                surface: "worse",
                segment_cost: 80,
                right_id: 9,
                total_cost: 80,
            },
            1,
        );
        assert_eq!(arena.len(), 8);
        assert_eq!(bucket.worst_total_cost, Some(80));

        insert_n_best_node(
            &mut arena,
            &mut bucket,
            NBestNode {
                start: 0,
                predecessor: None,
                reading: "a",
                surface: "better",
                segment_cost: 5,
                right_id: 9,
                total_cost: 5,
            },
            1,
        );
        assert_eq!(bucket.worst_total_cost, Some(70));

        insert_n_best_node(
            &mut arena,
            &mut bucket,
            NBestNode {
                start: 0,
                predecessor: None,
                reading: "a",
                surface: "seven replacement",
                segment_cost: 60,
                right_id: 7,
                total_cost: 60,
            },
            1,
        );
        assert_eq!(bucket.states.len(), 8);
        assert_eq!(bucket.worst_total_cost, Some(60));
    }

    #[test]
    fn exact_candidates_are_ordered_by_connected_cost() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates("にほん");
        let surfaces: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.surface.as_str())
            .collect();

        assert_eq!(surfaces[0], "日本");
        assert!(surfaces.contains(&"二本"), "surfaces: {surfaces:?}");
        assert!(surfaces.contains(&"ニホン"), "surfaces: {surfaces:?}");
        assert_eq!(candidates.last().unwrap().surface, "にほん");
    }

    #[test]
    fn bounded_compound_recall_combines_lower_ranked_exact_entries() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.compound_candidates("あさいり", 4, 16);

        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.surface == "浅煎り"),
            "{candidates:?}"
        );
        assert!(candidates.len() <= 16);
    }

    #[test]
    fn exact_compound_diagnostic_distinguishes_known_components() {
        let dictionary = Dictionary::bundled();

        assert!(dictionary.is_exact_compound_surface("こうなんりょうよう", "硬軟両様"));
        assert!(!dictionary.is_exact_compound_surface("こうなんりょうよう", "未知表層"));
        assert!(!dictionary.is_exact_compound_surface("こうなん", "硬軟"));
    }

    #[test]
    fn bounded_compound_recall_combines_three_exact_segments() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("うえ", "第二", 20),
            DictionaryEntry::new("おか", "第三", 30),
        ]);

        assert_eq!(
            dictionary.compound_candidates("あいうえおか", 4, 16),
            vec![Candidate {
                surface: "第一第二第三".to_owned(),
                cost: 60,
            }]
        );
    }

    #[test]
    fn bounded_compound_recall_includes_a_one_character_reading_segment() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("う", "中", 20),
            DictionaryEntry::new("えお", "第三", 30),
        ]);

        assert_eq!(
            dictionary.compound_candidates("あいうえお", 4, 16),
            vec![Candidate {
                surface: "第一中第三".to_owned(),
                cost: 60,
            }]
        );
    }

    #[test]
    fn bounded_compound_recall_combines_four_exact_segments() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("うえ", "第二", 20),
            DictionaryEntry::new("おか", "第三", 30),
            DictionaryEntry::new("きく", "第四", 40),
        ]);

        assert_eq!(
            dictionary.compound_candidates("あいうえおかきく", 4, 16),
            vec![Candidate {
                surface: "第一第二第三第四".to_owned(),
                cost: 100,
            }]
        );
    }

    #[test]
    fn bounded_compound_recall_combines_five_exact_segments() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("うえ", "第二", 20),
            DictionaryEntry::new("おか", "第三", 30),
            DictionaryEntry::new("きく", "第四", 40),
            DictionaryEntry::new("けこ", "第五", 50),
        ]);

        assert_eq!(
            dictionary.compound_candidates("あいうえおかきくけこ", 4, 16),
            vec![Candidate {
                surface: "第一第二第三第四第五".to_owned(),
                cost: 150,
            }]
        );
    }

    #[test]
    fn bounded_compound_recall_combines_six_exact_segments() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("うえ", "第二", 20),
            DictionaryEntry::new("おか", "第三", 30),
            DictionaryEntry::new("きく", "第四", 40),
            DictionaryEntry::new("けこ", "第五", 50),
            DictionaryEntry::new("さし", "第六", 60),
        ]);

        assert_eq!(
            dictionary.compound_candidates("あいうえおかきくけこさし", 4, 16),
            vec![Candidate {
                surface: "第一第二第三第四第五第六".to_owned(),
                cost: 210,
            }]
        );
    }

    #[test]
    fn fixed_segment_variants_change_surfaces_within_best_boundaries() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("あい", "別一", 20),
            DictionaryEntry::new("うえ", "第二", 10),
            DictionaryEntry::new("うえ", "別二", 20),
            DictionaryEntry::new("おか", "第三", 10),
            DictionaryEntry::new("おか", "別三", 20),
        ]);

        let variants = dictionary.fixed_segment_variants("あいうえおか", 2, 8);

        assert_eq!(variants.len(), 7);
        assert!(variants.contains(&"別一第二第三".to_owned()));
        assert!(variants.contains(&"第一別二第三".to_owned()));
        assert!(variants.contains(&"第一第二別三".to_owned()));
        assert!(variants.contains(&"別一別二別三".to_owned()));
        assert!(!variants.contains(&"第一第二第三".to_owned()));
    }

    #[test]
    fn fixed_segment_variants_obey_input_and_output_bounds() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("あい", "別一", 20),
            DictionaryEntry::new("うえ", "第二", 10),
            DictionaryEntry::new("うえ", "別二", 20),
        ]);

        assert_eq!(
            dictionary
                .fixed_segment_variants("あいうえ", usize::MAX, usize::MAX)
                .len(),
            3
        );
        assert!(
            dictionary
                .fixed_segment_variants("あいうえ", 0, 8)
                .is_empty()
        );
        assert!(
            dictionary
                .fixed_segment_variants("あいうえ", 8, 0)
                .is_empty()
        );
        assert!(
            dictionary
                .fixed_segment_variants(&"あ".repeat(129), 8, 8)
                .is_empty()
        );
    }

    #[test]
    fn bounded_compound_recall_combines_a_name_and_affiliation() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("やまだ", "山田", 10),
            DictionaryEntry::new("たろう", "太郎", 20),
            DictionaryEntry::new("だいに", "第二", 25),
            DictionaryEntry::new("けんきゅう", "研究", 30),
            DictionaryEntry::new("しつ", "室", 40),
        ]);

        assert_eq!(
            dictionary.compound_candidates("やまだたろうだいにけんきゅうしつ", 4, 16),
            vec![Candidate {
                surface: "山田太郎第二研究室".to_owned(),
                cost: 125,
            }]
        );
    }

    #[test]
    fn bounded_compound_recall_connects_a_kana_only_dictionary_segment() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("やまだ", "山田", 10),
            DictionaryEntry::new("の", "の", 20),
            DictionaryEntry::new("けんきゅう", "研究", 30),
            DictionaryEntry::new("しつ", "室", 40),
        ]);

        assert_eq!(
            dictionary.compound_candidates("やまだのけんきゅうしつ", 4, 16),
            vec![Candidate {
                surface: "山田の研究室".to_owned(),
                cost: 100,
            }]
        );
    }

    #[test]
    fn bounded_compound_recall_does_not_return_all_literal_or_prefer_literal_variants() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "あい", 1),
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("うえ", "うえ", 1),
            DictionaryEntry::new("うえ", "第二", 20),
        ]);

        assert_eq!(
            dictionary.compound_candidates("あいうえ", 4, 16),
            vec![Candidate {
                surface: "第一第二".to_owned(),
                cost: 30,
            }]
        );

        let literal_only = Dictionary::new(vec![
            DictionaryEntry::new("あい", "あい", 1),
            DictionaryEntry::new("うえ", "うえ", 1),
        ]);
        assert!(
            literal_only
                .compound_candidates("あいうえ", 4, 16)
                .is_empty()
        );
    }

    #[test]
    fn bounded_compound_recall_rejects_unbounded_inputs() {
        let dictionary = Dictionary::new(vec![DictionaryEntry::new("あい", "第一", 10)]);

        assert!(dictionary.compound_candidates("あいうえ", 0, 16).is_empty());
        assert!(dictionary.compound_candidates("あいうえ", 4, 0).is_empty());
        assert!(
            dictionary
                .compound_candidates("あいうえおかきくけこさしすせそたち", 4, 16)
                .is_empty()
        );
    }

    #[test]
    fn phrase_reading_prefers_its_phrase_entry_over_patchwork_paths() {
        let dictionary = Dictionary::bundled();

        // Regression: a hard-coded low cost for かんじ→漢字 once leaked into
        // the lattice and turned いいかんじ into いい漢字. The dictionary's
        // own phrase entry must win in the window and the preview alike.
        assert_eq!(dictionary.candidates("いいかんじ")[0].surface, "いい感じ");
        assert_eq!(
            dictionary.convert_best("いいかんじ").unwrap().surface,
            "いい感じ"
        );
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

    /// 橋 outranks 箸 on word cost and both are plain nouns, so nothing in
    /// the current model can pick 箸 from the reading alone. Tracked here as
    /// an honest gap for the planned context model; do not make it pass by
    /// editing dictionary costs or adding the test sentence as an entry.
    #[test]
    #[ignore = "requires a context model"]
    fn semantically_ambiguous_noun_needs_context() {
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
    fn explicit_wider_search_recovers_a_deep_compound_candidate() {
        let dictionary = Dictionary::bundled();

        assert!(
            !dictionary
                .candidates("あさいり")
                .iter()
                .any(|candidate| candidate.surface == "浅煎り")
        );
        assert!(
            dictionary
                .candidates_with_limit("あさいり", 32)
                .iter()
                .any(|candidate| candidate.surface == "浅煎り")
        );
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
    fn candidate_ranker_receives_left_context_without_changing_the_legacy_api() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あ", "亜", 10),
            DictionaryEntry::new("あ", "阿", 20),
        ]);
        let ranker = PreferSurfaceInContext {
            context: "文脈",
            surface: "阿",
        };

        assert_eq!(
            dictionary.candidates_with_ranker("あ", 5, &ranker)[0].surface,
            "亜"
        );
        assert_eq!(
            dictionary.candidates_with_context_ranker("あ", "文脈", 5, &ranker)[0].surface,
            "阿"
        );
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
    fn reports_generated_number_surfaces_for_candidate_metadata() {
        let dictionary = Dictionary::new(Vec::new());

        assert_eq!(
            dictionary.generated_number_surfaces("せんきゅうひゃくきゅうじゅういち"),
            ["１９９１", "千九百九十一", "1991"]
        );
        assert_eq!(dictionary.generated_number_surfaces("１２３"), ["１２３"]);
        assert!(dictionary.generated_number_surfaces("にほん").is_empty());
    }

    #[test]
    fn literal_digit_runs_use_number_connections() {
        let dictionary = Dictionary::bundled();
        for (reading, expected) in [("２じごろ", "２時頃"), ("100えん", "100円")] {
            let candidates = dictionary.candidates(reading);
            assert!(
                candidates
                    .iter()
                    .take(10)
                    .any(|candidate| candidate.surface == expected),
                "missing {expected} for {reading}: {candidates:?}"
            );
        }
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

        // 感じ is the cheaper word; 漢字-first would need context or user
        // history, never a cost override in the bundled dictionary.
        assert_eq!(dictionary.candidates("かんじ")[0].surface, "感じ");
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
