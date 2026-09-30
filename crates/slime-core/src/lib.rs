//! Platform-independent IME state machine.

use slime_converter::{Candidate, Conversion, Dictionary, Segment};
use slime_romaji::RomajiComposer;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

// A stamp distinguishes edits even if the visible state returns to its old
// value. Global stamps also keep independently edited engine clones apart.
static NEXT_LIVE_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_live_generation() -> u64 {
    NEXT_LIVE_GENERATION.fetch_add(1, Ordering::Relaxed)
}

mod date_time_candidates;
mod dictionary_packs;
mod domain_dictionaries;
mod english_reverse;
mod live_conversion;
mod session_history;
mod text_transform;
mod typo_correction;
mod user_data;

use dictionary_packs::DictionaryPackStore;
use english_reverse::{ReverseMatch, ReverseReading};
use live_conversion::Decision as LiveConversionDecision;
use session_history::SessionHistory;

pub use dictionary_packs::{
    DictionaryPackInfo, DictionaryPackLoadError, DictionaryPackTrust,
    DictionaryPackVerificationKey, DictionaryPackVersionFloor, DictionaryPackWord,
    validate_dictionary_pack,
};
pub use domain_dictionaries::{
    ALL_DOMAIN_DICTIONARIES, BUSINESS_DICTIONARY, CREATIVE_DICTIONARY, TECHNOLOGY_DICTIONARY,
    words as domain_dictionary_words,
};
pub use user_data::{HistoryEntry, UserData, UserDictionaryEntry};

/// Every built-in date candidate format, used as the default by adapters.
pub const ALL_DATE_FORMATS: u32 = date_time_candidates::ALL_FORMATS;

const EXPANDED_N_BEST: usize = 32;
const MAX_EXPANDED_READING_CHARACTERS: usize = 8;
const MAX_COMPOUND_READING_CHARACTERS: usize = 16;
const COMPOUND_ENTRIES_PER_SEGMENT: usize = 8;
const COMPOUND_CANDIDATE_LIMIT: usize = 32;
const FIXED_SEGMENT_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_CANDIDATE_LIMIT: usize = 22;
const RECOMBINED_SOURCE_PATHS: usize = 10;
const RECOMBINED_CANDIDATE_LIMIT: usize = 32;
const CONTEXT_RULE_PROMOTION_LIMIT: usize = 8;
const EXPLICIT_RANKING_CANDIDATE_LIMIT: usize = 10;
const EXPLICIT_LONG_RANKING_CANDIDATE_LIMIT: usize = 16;
const EXPLICIT_RECOMBINED_CANDIDATE_LIMIT: usize = 4;
const EXPLICIT_LONG_RANKING_MIN_CHARACTERS: usize = 20;

fn explicit_candidate_search_limit(reading: &str) -> usize {
    if reading.chars().count() >= EXPLICIT_LONG_RANKING_MIN_CHARACTERS {
        EXPLICIT_LONG_RANKING_CANDIDATE_LIMIT
    } else {
        EXPLICIT_RANKING_CANDIDATE_LIMIT
    }
}

const LIVE_RANKING_CANDIDATE_LIMIT: usize = 15;
const LIVE_WIDE_RANKING_MIN_READING_CHARACTERS: usize = 9;
const MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS: usize = 4;
const MAX_REOPENED_STABLE_TOPIC_SUFFIX_CHARACTERS: usize = 4;
const MAX_LIVE_KATAKANA_REOPEN_SUFFIX_CHARACTERS: usize = 6;
const MAX_LIVE_PHRASE_REOPEN_SUFFIX_CHARACTERS: usize = 6;
const LIVE_FRAGMENTED_SUFFIX_PATH_LIMIT: usize = 4;
const LIVE_LITERAL_PRESERVATION_COST_GAP: i32 = 500;
const LIVE_LITERAL_PRESERVATION_MIN_CHARACTERS: usize = 5;
const MINIMUM_PENDING_PREFIX_REWRITE_READING_CHARACTERS: usize = 4;
const MAXIMUM_WORD_CHECKPOINT_EXTENSION_CHARACTERS: usize = 3;
const MAXIMUM_WORD_CHECKPOINT_ENTRY_COST: i32 = 5_000;
const MINIMUM_WORD_CHECKPOINT_COST_MARGIN: i32 = 500;
const MINIMUM_SUFFIX_CONTINUITY_READING_CHARACTERS: usize = 10;
const LIVE_IMPLICIT_NUMERIC_FIXED_SEGMENT_ENTRIES: usize = 8;
const LIVE_IMPLICIT_NUMERIC_FIXED_SEGMENT_CANDIDATES: usize = 22;
const LIVE_IMPLICIT_NUMERIC_DIVERSITY_REPLACEMENTS: usize = 6;
const LIVE_IMPLICIT_NUMERIC_EXISTING_REPAIR_COST_GAP: i32 = 1_000;
const LIVE_IMPLICIT_NUMERIC_REPAIR_PRIOR_COST_GAP: i32 = 250;
const LIVE_RECOMBINED_DIVERSITY_MIN_READING_CHARACTERS: usize = 9;
const LIVE_RECOMBINED_DIVERSITY_CANDIDATES: usize = 2;
const MINIMUM_AMBIGUOUS_CONTINUITY_READING_CHARACTERS: usize = 4;
const AMBIGUOUS_CONTINUITY_PATH_LIMIT: usize = 4;
const MAXIMUM_AMBIGUOUS_CONTINUITY_COST_GAP: i32 = 500;
const MAXIMUM_AMBIGUOUS_CONTINUITY_SEARCH_READING_CHARACTERS: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    Character(char),
    Space,
    Enter,
    Escape,
    Backspace,
    NextCandidate,
    PreviousCandidate,
    SelectCandidate(u32),
    AcceptCandidate,
    TransformHiragana,
    TransformFullKatakana,
    TransformHalfKatakana,
    TransformFullAlphanumeric,
    TransformHalfAlphanumeric,
    NextSegment,
    PreviousSegment,
    ExpandSegment,
    ShrinkSegment,
}

const _: () = assert!(std::mem::size_of::<InputEvent>() <= 8);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlimeAction {
    UpdatePreedit(String),
    UpdateSegmentedPreedit {
        text: String,
        selection_start: usize,
        selection_length: usize,
    },
    ShowCandidates {
        candidates: Vec<String>,
        details: Vec<CandidateDetail>,
        selected: usize,
    },
    HideCandidates,
    Commit(String),
    Clear,
    ForwardKey,
}

/// Semantic origin of one candidate. Adapters localize these values instead
/// of embedding explanatory text in the surface that will be committed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CandidateAnnotation {
    None = 0,
    UserDictionary = 1,
    History = 2,
    Correction = 3,
    Completion = 4,
    DateTime = 5,
    Number = 6,
    Context = 7,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateDetail {
    pub value: String,
    pub annotation: CandidateAnnotation,
    pub detail: Option<String>,
}

#[doc(hidden)]
pub type EvaluationLivePrefix<'a> = Option<(&'a str, &'a str)>;

/// One dictionary candidate eligible for optional post-ranking after the user
/// explicitly starts conversion. Protected candidates such as history and
/// user-dictionary entries are deliberately absent from this request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRankingItem {
    pub surface: String,
    pub cost: i32,
}

/// Bounded input for an optional explicit-conversion ranker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRankingRequest {
    pub reading: String,
    pub left_context: String,
    pub candidates: Vec<CandidateRankingItem>,
}

/// Explicit ranking result with an optional bounded dictionary expansion request.
/// Source surfaces must belong to the current request; the first must be the
/// selected surface in `ranked_surfaces`. Only one expansion round is allowed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRankingPlan {
    pub ranked_surfaces: Vec<String>,
    pub recombination_sources: Option<[String; 2]>,
}

/// Immutable engine state for delayed LIVE candidate ranking.
///
/// The dictionary clone shares its static data and `Arc`-backed layers. A
/// platform adapter can therefore capture this snapshot on the input thread,
/// then run the wider candidate search and model on a worker without retaining
/// or concurrently accessing the mutable engine.
#[derive(Clone, Debug)]
pub struct LiveCandidateRankingSnapshot {
    generation: u64,
    dictionary: Dictionary,
    resolved_reading: String,
    target_reading: String,
    left_context: String,
    external_left_context: String,
    display_surface: String,
    base_surface: String,
    prefix: Option<LiveStablePrefix>,
    original_prefix_before_boundary_reopen: Option<LiveStablePrefix>,
    boundary_is_personalized: bool,
    boundary_alternatives: Vec<CandidateRankingItem>,
    boundary_paths: Vec<Conversion>,
    prefix_repair_paths: OnceLock<Vec<Conversion>>,
    joint_repair_paths: OnceLock<Vec<Conversion>>,
    guided_joint_repair_approval: OnceLock<(String, String, String)>,
    object_inflection_paths: OnceLock<Vec<Conversion>>,
    verb_auxiliary_candidates: OnceLock<Vec<Candidate>>,
    verb_auxiliary_approval: OnceLock<(CandidateRankingRequest, String)>,
    object_inflection_approval: OnceLock<(String, String, String)>,
    pending_prefix: Option<LiveStablePrefix>,
    word_checkpoint: Option<LiveStablePrefix>,
    literal_extension_checkpoint: bool,
}

impl LiveCandidateRankingSnapshot {
    /// Preserves the original snapshot for stale-state validation while
    /// reopening one literal particle at a dictionary-supported boundary.
    /// Personalized scopes remain unchanged. Call on the ranking worker.
    #[doc(hidden)]
    pub fn prepare_particle_boundary_reopen(&mut self) -> bool {
        if self.original_prefix_before_boundary_reopen.is_some() || self.boundary_is_personalized {
            return false;
        }
        let Some(prefix) = self.prefix.as_ref() else {
            return false;
        };
        let Some(last) = prefix.reading.chars().last() else {
            return false;
        };
        if !is_likely_particle_character(last) || !prefix.surface.ends_with(last) {
            return false;
        }
        let reading_end = prefix.reading.len() - last.len_utf8();
        let surface_end = prefix.surface.len() - last.len_utf8();
        if reading_end == 0 || surface_end == 0 {
            return false;
        }
        let reading = self.resolved_reading[reading_end..].to_owned();
        // Require a real dictionary word to cross the old boundary.
        let paths = self
            .dictionary
            .convert_n_best(&reading, LIVE_RANKING_CANDIDATE_LIMIT);
        if !paths.first().is_some_and(|path| {
            path.segments.first().is_some_and(|segment| {
                segment.reading.chars().count() >= 2
                    && segment.surface.chars().any(is_kanji_character)
            })
        }) {
            return false;
        }
        let shortened = LiveStablePrefix {
            reading: prefix.reading[..reading_end].to_owned(),
            surface: prefix.surface[..surface_end].to_owned(),
        };
        let particle = last.to_string();
        self.boundary_alternatives = paths
            .iter()
            .filter(|path| {
                path.segments.len() > 1
                    && path.segments.first().is_some_and(|segment| {
                        segment.reading == particle && segment.surface == particle
                    })
            })
            .map(|path| CandidateRankingItem {
                surface: path.surface.clone(),
                cost: path.cost,
            })
            .collect();
        self.boundary_paths = paths;
        self.original_prefix_before_boundary_reopen = Some(prefix.clone());
        self.left_context = self.external_left_context.clone();
        self.left_context.push_str(&shortened.surface);
        self.prefix = Some(shortened);
        self.target_reading = reading;
        self.prefix_repair_paths = OnceLock::new();
        self.joint_repair_paths = OnceLock::new();
        self.guided_joint_repair_approval = OnceLock::new();
        self.object_inflection_paths = OnceLock::new();
        self.verb_auxiliary_candidates = OnceLock::new();
        self.verb_auxiliary_approval = OnceLock::new();
        self.object_inflection_approval = OnceLock::new();
        true
    }

    /// Checks that the selected path actually uses the reopened
    /// boundary, rather than changing an unrelated word later in the suffix.
    #[doc(hidden)]
    pub fn boundary_winner_uses_reopened_edge(&self, selected: &str) -> bool {
        let Some(original) = self.original_prefix_before_boundary_reopen.as_ref() else {
            return false;
        };
        let Some(path) = self.boundary_paths.iter().find(|p| p.surface == selected) else {
            return false;
        };
        let Some(first) = path.segments.first() else {
            return false;
        };
        if first.reading.chars().count() >= 2 && first.surface.chars().any(is_kanji_character) {
            return true;
        }
        let Some(particle) = original.reading.chars().last() else {
            return false;
        };
        matches!(path.segments.as_slice(), [edge, word]
            if self.target_reading.chars().count() <= MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS + 1
                && edge.reading == particle.to_string() && edge.surface == edge.reading
                && word.reading.chars().count() >= 2 && word.surface.chars().any(is_kanji_character))
    }

    /// Checks immutable application guards on the worker so an unusable
    /// boundary repair can fall back to the original ranking scope.
    #[doc(hidden)]
    #[must_use]
    pub fn boundary_winner_can_apply(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        self.selected_surface_for_application(request, selected)
            .is_some()
    }

    /// Records model support for a newly permitted short literal repair.
    /// Dictionary cost alone cannot authorize this application exception.
    #[doc(hidden)]
    pub fn approve_model_supported_object_inflection(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
        logliks: &[f64],
    ) {
        if logliks.len() != request.candidates.len() || logliks.iter().any(|s| !s.is_finite()) {
            return;
        }
        let Some(index) = request
            .candidates
            .iter()
            .position(|c| c.surface == selected)
        else {
            return;
        };
        if logliks.iter().any(|score| *score > logliks[index])
            || !self.object_inflection_candidate_is_safe(request, selected)
        {
            return;
        }
        let _ = self.object_inflection_approval.set((
            request.reading.clone(),
            request.left_context.clone(),
            selected.to_owned(),
        ));
    }

    fn object_inflection_repair_is_safe(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        self.object_inflection_approval
            .get()
            .is_some_and(|(reading, context, surface)| {
                reading == &request.reading
                    && context == &request.left_context
                    && surface == selected
            })
            && self.object_inflection_candidate_is_safe(request, selected)
    }

    fn verb_auxiliary_scope_is_safe(&self, request: &CandidateRankingRequest) -> bool {
        self.original_prefix_before_boundary_reopen.is_none()
            && request.reading == self.target_reading
            && request.left_context == self.left_context
            && !self.request_reopens_stable_prefix(request)
            && (3..=8).contains(&self.target_reading.chars().count())
            && self.prefix.as_ref().is_some_and(|prefix| {
                prefix.reading.ends_with('を')
                    && prefix.surface.ends_with('を')
                    && prefix.surface.chars().any(is_kanji_character)
            })
    }

    fn scoped_verb_auxiliary_candidates(&self, request: &CandidateRankingRequest) -> &[Candidate] {
        if !self.verb_auxiliary_scope_is_safe(request) {
            return &[];
        }
        self.verb_auxiliary_candidates.get_or_init(|| {
            self.dictionary
                .verb_auxiliary_candidates(&self.target_reading, &self.left_context, 8)
                .into_iter()
                .filter(|c| {
                    let mut chars = c.surface.chars();
                    chars.next().is_some_and(is_kanji_character)
                        && !chars.as_str().is_empty()
                        && chars.as_str().chars().all(is_hiragana_or_mark)
                        && self.target_reading.ends_with(chars.as_str())
                })
                .collect()
        })
    }

    /// Exposes worker-cached grammatical paths for bounded neural scoring.
    #[doc(hidden)]
    #[must_use]
    pub fn request_has_verb_auxiliary_candidates(&self, request: &CandidateRankingRequest) -> bool {
        !self.scoped_verb_auxiliary_candidates(request).is_empty()
    }

    /// Authorizes a grammatical repair only with strong full and body model
    /// support. Approval is bound to the entire request and selected surface.
    #[doc(hidden)]
    pub fn approve_model_supported_verb_auxiliary(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
        full: &[f64],
        body: &[f64],
    ) -> bool {
        if full.len() != request.candidates.len()
            || body.len() != full.len()
            || full.iter().chain(body).any(|x| !x.is_finite())
            || !self
                .scoped_verb_auxiliary_candidates(request)
                .iter()
                .any(|c| c.surface == selected)
        {
            return false;
        }
        let Some(index) = request
            .candidates
            .iter()
            .position(|c| c.surface == selected)
        else {
            return false;
        };
        if index == 0
            || full[index] - full[0] < 2.0
            || body[index] - body[0] < 2.0
            || full.iter().any(|x| *x > full[index])
            || body.iter().any(|x| *x > body[index])
        {
            return false;
        }
        let _ = self
            .verb_auxiliary_approval
            .set((request.clone(), selected.to_owned()));
        self.verb_auxiliary_repair_is_safe(request, selected)
    }

    /// Checks the exact worker approval without rebuilding dictionary paths.
    #[doc(hidden)]
    #[must_use]
    pub fn verb_auxiliary_repair_is_safe(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        self.verb_auxiliary_scope_is_safe(request)
            && self
                .verb_auxiliary_approval
                .get()
                .is_some_and(|(approved, surface)| approved == request && surface == selected)
    }

    fn object_inflection_candidate_is_safe(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        let Some(prefix) = self.prefix.as_ref() else {
            return false;
        };
        if !prefix.reading.ends_with('を')
            || !prefix.surface.ends_with('を')
            || !prefix.surface.chars().any(is_kanji_character)
            || !(2..=4).contains(&self.target_reading.chars().count())
            || self.base_surface.strip_prefix(&prefix.surface) != Some(self.target_reading.as_str())
        {
            return false;
        }
        let target = if self.request_reopens_stable_prefix(request) {
            let Some(target) = selected.strip_prefix(&prefix.surface) else {
                return false;
            };
            target
        } else {
            selected
        };
        let mut characters = target.chars();
        if !characters.next().is_some_and(is_kanji_character) {
            return false;
        }
        let suffix = characters.as_str();
        if suffix.is_empty()
            || !suffix.chars().all(is_hiragana_or_mark)
            || !self.target_reading.ends_with(suffix)
        {
            return false;
        }
        let paths = self.object_inflection_paths.get_or_init(|| {
            self.dictionary
                .convert_n_best(&self.target_reading, LIVE_RANKING_CANDIDATE_LIMIT)
        });
        paths.iter().any(|path| {
            path.surface == target
                && matches!(path.segments.as_slice(), [word] if word.reading == self.target_reading)
        })
    }

    fn selected_surface_for_application(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> Option<String> {
        let boundary_word_repair = self.boundary_word_repair_is_safe(request, selected)
            || self.object_inflection_repair_is_safe(request, selected)
            || self.verb_auxiliary_repair_is_safe(request, selected);
        if !self.reopened_request_changes_are_bounded(request, selected)
            || (!boundary_word_repair && !self.ranked_target_is_safe(request, selected))
        {
            return None;
        }
        let mut surface = if self.request_reopens_stable_prefix(request) {
            String::new()
        } else {
            self.prefix
                .as_ref()
                .map_or_else(String::new, |prefix| prefix.surface.clone())
        };
        surface.push_str(selected);
        if surface == self.display_surface
            || (!boundary_word_repair
                && rewrites_only_protected_literal_tail(
                    &self.resolved_reading,
                    &self.base_surface,
                    &surface,
                ))
        {
            return None;
        }
        Some(surface)
    }

    fn boundary_word_repair_is_safe(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        let Some(original) = self.original_prefix_before_boundary_reopen.as_ref() else {
            return false;
        };
        if request.reading != self.target_reading
            || request.reading.chars().count() > MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS + 1
        {
            return false;
        }
        let Some(base) = self.base_target_surface_for_request(request) else {
            return false;
        };
        let Some(particle) = original.reading.chars().last() else {
            return false;
        };
        let Some(path) = self.boundary_paths.iter().find(|p| p.surface == selected) else {
            return false;
        };
        let converted_word = |surface: &str| {
            surface.chars().any(is_kanji_character)
                && surface
                    .chars()
                    .all(|c| is_kanji_character(c) || matches!(c, 'ぁ'..='ゖ'))
        };
        match path.segments.as_slice() {
            [word] => {
                base.starts_with(particle)
                    && base.chars().any(is_kanji_character)
                    && base.chars().count() == selected.chars().count() + 1
                    && word.reading == request.reading
                    && converted_word(&word.surface)
                    && selected.chars().next().is_some_and(is_kanji_character)
                    && selected
                        .chars()
                        .last()
                        .is_some_and(|c| matches!(c, 'ぁ'..='ゖ') && base.ends_with(c))
            }
            [edge, word] => {
                base == request.reading
                    && base.chars().count() == selected.chars().count()
                    && edge.reading == particle.to_string()
                    && edge.surface == edge.reading
                    && word.reading.chars().count() >= 2
                    && converted_word(&word.surface)
            }
            _ => false,
        }
    }

    /// Returns whether this snapshot contains a stable prefix.
    ///
    /// This is exposed for evaluator diagnostics, not adapter policy.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_has_stable_prefix(&self) -> bool {
        self.prefix.is_some()
    }

    /// Returns the currently displayed target surface for `request`.
    ///
    /// This is exposed for evaluator diagnostics, not adapter policy.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_base_target_surface_for_request(
        &self,
        request: &CandidateRankingRequest,
    ) -> Option<&str> {
        self.base_target_surface_for_request(request)
    }

    /// Appends bounded alternative generation paths to an existing LIVE
    /// request for evaluator A/B runs.
    ///
    /// This is deliberately separate from [`Self::candidate_ranking_request`]
    /// so product adapters cannot enable a slower candidate path by accident.
    /// The request must still describe this snapshot's actual worker scope;
    /// suffix-only requests therefore stay suffix-only.
    #[doc(hidden)]
    pub fn evaluation_extend_candidate_ranking_request(
        &self,
        request: &mut CandidateRankingRequest,
        fixed_segment_limit: usize,
        recombined_limit: usize,
    ) -> bool {
        if !self.request_matches_ranking_scope(request) {
            return false;
        }

        let fixed = self.dictionary.fixed_segment_candidates(
            &request.reading,
            FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
            fixed_segment_limit,
        );
        let recombined = self.dictionary.recombined_n_best_variants(
            &request.reading,
            EXPANDED_N_BEST,
            recombined_limit,
        );
        for candidate in fixed.into_iter().chain(recombined) {
            if request
                .candidates
                .iter()
                .any(|existing| existing.surface == candidate.surface)
            {
                continue;
            }
            request.candidates.push(CandidateRankingItem {
                surface: candidate.surface,
                cost: candidate.cost,
            });
        }
        true
    }

    /// Replaces low-ranked tail candidates with structurally guarded
    /// alternatives while preserving the original request width.
    ///
    /// Fixed-boundary paths are useful when a bounded full-reading repair can
    /// jointly fix an earlier homophone and a long literal tail. Recombined
    /// paths are useful for already-converted suffix scopes, where independent
    /// segment alternatives can otherwise miss their cross product. Applying
    /// either search to a long literal suffix destabilizes strong existing
    /// candidates, so this evaluator policy keeps those cases separate.
    #[doc(hidden)]
    pub fn evaluation_guarded_replace_candidate_ranking_request(
        &self,
        request: &mut CandidateRankingRequest,
        fixed_segment_limit: usize,
        recombined_limit: usize,
    ) -> bool {
        if !self.request_matches_ranking_scope(request) {
            return false;
        }
        let reopens_stable_prefix = self.request_reopens_stable_prefix(request);
        let has_long_literal_suffix = self.request_target_has_unresolved_literal_suffix(request, 4);
        let (fixed_segment_limit, recombined_limit) =
            if reopens_stable_prefix && has_long_literal_suffix {
                (fixed_segment_limit, 0)
            } else if !reopens_stable_prefix && !has_long_literal_suffix {
                (0, recombined_limit)
            } else {
                return true;
            };
        let original_len = request.candidates.len();
        if !self.evaluation_extend_candidate_ranking_request(
            request,
            fixed_segment_limit,
            recombined_limit,
        ) {
            return false;
        }
        if request.candidates.len() == original_len {
            return true;
        }

        let additions = request.candidates.split_off(original_len);
        let replaceable: Vec<_> = (1..request.candidates.len())
            .rev()
            .filter(|&index| request.candidates[index].surface != request.reading)
            .take(additions.len())
            .collect();
        let replacement_count = replaceable.len();
        for index in replaceable {
            request.candidates.remove(index);
        }
        request
            .candidates
            .extend(additions.into_iter().take(replacement_count));
        true
    }

    /// Returns whether `request` intentionally reopens the bounded stable
    /// prefix instead of ranking only the unresolved suffix.
    #[must_use]
    pub fn request_reopens_stable_prefix(&self, request: &CandidateRankingRequest) -> bool {
        self.prefix.is_some()
            && self.target_reading != self.resolved_reading
            && request.reading == self.resolved_reading
            && request.left_context == self.external_left_context
    }

    fn request_matches_ranking_scope(&self, request: &CandidateRankingRequest) -> bool {
        (request.reading == self.target_reading && request.left_context == self.left_context)
            || self.request_reopens_stable_prefix(request)
    }

    #[must_use]
    pub fn request_has_bounded_kanji_run_length_repair(
        &self,
        request: &CandidateRankingRequest,
    ) -> bool {
        if !self.request_reopens_stable_prefix(request) {
            return false;
        }
        let Some(prefix) = self.prefix.as_ref() else {
            return false;
        };
        let Some(target_surface) = self.base_surface.strip_prefix(&prefix.surface) else {
            return false;
        };
        request.candidates.iter().any(|candidate| {
            candidate
                .surface
                .strip_suffix(target_surface)
                .is_some_and(|candidate_prefix| {
                    candidate_prefix.chars().count() != prefix.surface.chars().count()
                        && live_conversion::bounded_kanji_run_length_difference(
                            &prefix.surface,
                            candidate_prefix,
                        )
                })
        })
    }

    /// Returns the logical baseline target for the given ranking scope.
    /// Continuity-only previews may use literal reading as this baseline.
    #[must_use]
    pub fn base_target_surface_for_request(
        &self,
        request: &CandidateRankingRequest,
    ) -> Option<&str> {
        if self.request_reopens_stable_prefix(request) {
            return Some(&self.base_surface);
        }
        self.prefix.as_ref().map_or_else(
            || Some(self.base_surface.as_str()),
            |prefix| self.base_surface.strip_prefix(&prefix.surface),
        )
    }

    /// Returns the currently displayed target for a matching worker scope.
    #[must_use]
    pub fn display_target_surface_for_request(
        &self,
        request: &CandidateRankingRequest,
    ) -> Option<&str> {
        if self.request_reopens_stable_prefix(request) {
            return Some(&self.display_surface);
        }
        self.prefix.as_ref().map_or_else(
            || Some(self.display_surface.as_str()),
            |prefix| self.display_surface.strip_prefix(&prefix.surface),
        )
    }

    fn reopened_request_changes_are_bounded(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        if !self.request_reopens_stable_prefix(request) {
            return true;
        }
        let Some(prefix) = self.prefix.as_ref() else {
            return false;
        };
        let Some(target_surface) = self.base_surface.strip_prefix(&prefix.surface) else {
            return false;
        };
        selected.starts_with(&prefix.surface)
            || selected
                .strip_suffix(target_surface)
                .is_some_and(|candidate_prefix| {
                    live_conversion::bounded_surface_difference(&prefix.surface, candidate_prefix)
                        || live_conversion::bounded_kanji_run_length_difference(
                            &prefix.surface,
                            candidate_prefix,
                        )
                        || self
                            .reopened_prefix_repairs_one_inflected_segment(prefix, candidate_prefix)
                        || self.reopened_prefix_repairs_two_kanji_words(prefix, candidate_prefix)
                })
            || self.reopened_request_repairs_prefix_and_literal_target(
                &prefix.surface,
                target_surface,
                selected,
            )
            || self.reopened_request_repairs_shifted_target(prefix, target_surface, selected)
            || self.reopened_request_repairs_two_kanji_scopes(request, selected)
            || self.reopened_request_repairs_two_aligned_words(request, selected)
    }

    fn reopened_request_repairs_two_aligned_words(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        if !self.request_reopens_stable_prefix(request)
            || !live_conversion::bounded_two_word_kanji_difference(&self.base_surface, selected)
        {
            return false;
        }
        let paths = self.joint_repair_paths.get_or_init(|| {
            self.dictionary
                .convert_n_best(&self.resolved_reading, LIVE_RANKING_CANDIDATE_LIMIT)
        });
        live_conversion::two_word_kanji_surface_difference(paths, &self.base_surface, selected)
            || self
                .guided_joint_repair_approval
                .get()
                .is_some_and(|(reading, context, surface)| {
                    reading == &request.reading
                        && context == &request.left_context
                        && surface == selected
                        && request
                            .candidates
                            .iter()
                            .any(|candidate| candidate.surface == selected)
                })
    }

    // Worker-only preparation: application reads a surface/scope-bound approval
    // and never starts a guided dictionary search on the input event thread.
    fn prepare_guided_joint_repair(&self, request: &CandidateRankingRequest, selected: &str) {
        if self.guided_joint_repair_approval.get().is_some()
            || !self.request_reopens_stable_prefix(request)
            || !live_conversion::bounded_two_word_kanji_difference(&self.base_surface, selected)
            || !request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == selected)
        {
            return;
        }
        let Some(paths) = [self.base_surface.as_str(), selected]
            .into_iter()
            .map(|surface| {
                self.dictionary
                    .conversion_for_surface(&self.resolved_reading, surface)
            })
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if live_conversion::two_word_kanji_surface_difference(&paths, &self.base_surface, selected)
        {
            let _ = self.guided_joint_repair_approval.set((
                request.reading.clone(),
                request.left_context.clone(),
                selected.to_owned(),
            ));
        }
    }

    fn reopened_request_repairs_two_kanji_scopes(
        &self,
        request: &CandidateRankingRequest,
        selected: &str,
    ) -> bool {
        if !self.request_reopens_stable_prefix(request) {
            return false;
        }
        let Some(prefix) = self.prefix.as_ref() else {
            return false;
        };
        let Some(target) = self.base_surface.strip_prefix(&prefix.surface) else {
            return false;
        };
        if target.is_empty() || !target.chars().all(is_kanji_character) {
            return false;
        }
        let boundary = selected
            .char_indices()
            .nth(prefix.surface.chars().count())
            .map_or(selected.len(), |(index, _)| index);
        let (selected_prefix, selected_target) = selected.split_at(boundary);
        if !selected_target.chars().all(is_kanji_character)
            || !live_conversion::bounded_surface_difference(&prefix.surface, selected_prefix)
            || !live_conversion::bounded_surface_difference(target, selected_target)
        {
            return false;
        }
        let paths = self.joint_repair_paths.get_or_init(|| {
            self.dictionary
                .convert_n_best(&self.resolved_reading, LIVE_RANKING_CANDIDATE_LIMIT)
        });
        live_conversion::two_scope_kanji_surface_difference(
            paths,
            &self.base_surface,
            selected,
            &prefix.reading,
            &prefix.surface,
        )
    }

    fn reopened_prefix_repairs_two_kanji_words(
        &self,
        prefix: &LiveStablePrefix,
        selected_prefix: &str,
    ) -> bool {
        if !live_conversion::bounded_two_word_kanji_difference(&prefix.surface, selected_prefix) {
            return false;
        }
        let paths = self.prefix_repair_paths.get_or_init(|| {
            self.dictionary
                .convert_n_best(&prefix.reading, LIVE_RANKING_CANDIDATE_LIMIT)
        });
        live_conversion::two_word_kanji_surface_difference(paths, &prefix.surface, selected_prefix)
    }

    fn reopened_prefix_repairs_one_inflected_segment(
        &self,
        prefix: &LiveStablePrefix,
        selected_prefix: &str,
    ) -> bool {
        if !live_conversion::bounded_inflected_surface_difference(&prefix.surface, selected_prefix)
        {
            return false;
        }
        let paths = self.prefix_repair_paths.get_or_init(|| {
            self.dictionary
                .convert_n_best(&prefix.reading, LIVE_RANKING_CANDIDATE_LIMIT)
        });
        live_conversion::single_segment_inflected_surface_difference(
            paths,
            &prefix.surface,
            selected_prefix,
        )
    }

    /// Prepares immutable dictionary evidence on the ranking worker. Application
    /// still validates the snapshot generation and selected candidate; caching
    /// only avoids repeating bounded repair searches on the input thread.
    #[doc(hidden)]
    pub fn prepare_ranked_prefix_validation(
        &self,
        request: &CandidateRankingRequest,
        ranked_surfaces: &[String],
    ) {
        if self.request_reopens_stable_prefix(request)
            && let Some(prefix) = self.prefix.as_ref()
            && let Some(target) = self.base_surface.strip_prefix(&prefix.surface)
            && let Some(selected) = ranked_surfaces.first()
            && let Some(selected_prefix) = selected.strip_suffix(target)
        {
            self.reopened_prefix_repairs_one_inflected_segment(prefix, selected_prefix);
            self.reopened_prefix_repairs_two_kanji_words(prefix, selected_prefix);
        }
        if let Some(selected) = ranked_surfaces.first() {
            let scopes_aligned = self.reopened_request_repairs_two_kanji_scopes(request, selected);
            let words_aligned = self.reopened_request_repairs_two_aligned_words(request, selected);
            if !scopes_aligned && !words_aligned {
                self.prepare_guided_joint_repair(request, selected);
            }
            self.object_inflection_candidate_is_safe(request, selected);
        }
    }

    fn reopened_request_repairs_prefix_and_literal_target(
        &self,
        stable_surface: &str,
        target_surface: &str,
        selected: &str,
    ) -> bool {
        if target_surface != self.target_reading {
            return false;
        }
        let candidates = if self.left_context.is_empty() {
            self.dictionary
                .candidates_with_limit(&self.target_reading, LIVE_RANKING_CANDIDATE_LIMIT)
        } else {
            self.dictionary.candidates_with_context_limit(
                &self.target_reading,
                &self.left_context,
                LIVE_RANKING_CANDIDATE_LIMIT,
            )
        };
        candidates.into_iter().any(|candidate| {
            candidate.surface != target_surface
                && selected
                    .strip_suffix(&candidate.surface)
                    .is_some_and(|candidate_prefix| {
                        live_conversion::bounded_surface_difference(
                            stable_surface,
                            candidate_prefix,
                        ) && live_ranked_target_is_safe(
                            true,
                            &self.target_reading,
                            target_surface,
                            &candidate.surface,
                        )
                    })
        })
    }

    fn reopened_request_repairs_shifted_target(
        &self,
        stable_prefix: &LiveStablePrefix,
        target_surface: &str,
        selected: &str,
    ) -> bool {
        if target_surface != self.target_reading {
            return false;
        }
        let Some((_reading_boundary, shifted_reading)) =
            stable_prefix.reading.char_indices().next_back()
        else {
            return false;
        };
        let Some((surface_boundary, shifted_surface)) =
            stable_prefix.surface.char_indices().next_back()
        else {
            return false;
        };
        if shifted_reading != shifted_surface
            || !is_live_boundary_particle_character(shifted_reading)
        {
            return false;
        }
        let shortened_surface = &stable_prefix.surface[..surface_boundary];
        let Some(selected_target) = selected.strip_prefix(shortened_surface) else {
            return false;
        };
        if selected_target.starts_with(shifted_surface) {
            return false;
        }
        let mut widened_reading = String::with_capacity(
            shifted_reading
                .len_utf8()
                .saturating_add(self.target_reading.len()),
        );
        widened_reading.push(shifted_reading);
        widened_reading.push_str(&self.target_reading);
        let mut widened_base_surface = String::with_capacity(
            shifted_surface
                .len_utf8()
                .saturating_add(target_surface.len()),
        );
        widened_base_surface.push(shifted_surface);
        widened_base_surface.push_str(target_surface);
        if !live_ranked_target_is_safe(
            true,
            &widened_reading,
            &widened_base_surface,
            selected_target,
        ) {
            return false;
        }
        let mut widened_left_context = self.external_left_context.clone();
        widened_left_context.push_str(shortened_surface);
        let candidates = if widened_left_context.is_empty() {
            self.dictionary
                .candidates_with_limit(&widened_reading, LIVE_RANKING_CANDIDATE_LIMIT)
        } else {
            self.dictionary.candidates_with_context_limit(
                &widened_reading,
                &widened_left_context,
                LIVE_RANKING_CANDIDATE_LIMIT,
            )
        };
        candidates
            .iter()
            .any(|candidate| candidate.surface == selected_target)
    }

    fn ranked_target_is_safe(&self, request: &CandidateRankingRequest, selected: &str) -> bool {
        let reopens_stable_prefix = self.request_reopens_stable_prefix(request);
        if self
            .display_target_surface_for_request(request)
            .is_some_and(|display_target| {
                rewrites_katakana_word_tail_as_particle(display_target, selected)
            })
        {
            return false;
        }
        let Some(base_target_surface) = self.base_target_surface_for_request(request) else {
            return false;
        };
        if !rewrites_literal_tail_as_katakana_list(&request.reading, base_target_surface, selected)
            && safer_literal_tail_alternative(request, selected).is_some()
        {
            return false;
        }
        if self.prefix.is_none()
            && !request.left_context.is_empty()
            && request
                .candidates
                .first()
                .is_some_and(|candidate| candidate.surface == selected)
            && rewrites_particle_looking_suffix_as_two_kanji_word(
                &request.reading,
                base_target_surface,
                selected,
            )
        {
            return true;
        }
        if reopens_stable_prefix
            && let Some(prefix) = self.prefix.as_ref()
            && let Some(target_surface) = self.base_surface.strip_prefix(&prefix.surface)
            && let Some(selected_target) = selected.strip_prefix(&prefix.surface)
        {
            if request
                .candidates
                .first()
                .is_some_and(|base| base.surface != selected)
                && rewrites_reopened_two_kana_inflected_target(
                    &self.target_reading,
                    target_surface,
                    selected_target,
                )
            {
                return true;
            }
            return live_ranked_target_is_safe(
                true,
                &self.target_reading,
                target_surface,
                selected_target,
            );
        }
        live_ranked_target_is_safe(
            self.prefix.is_some() && !reopens_stable_prefix,
            &request.reading,
            base_target_surface,
            selected,
        )
    }

    /// Reconstructs the complete marked-text surfaces that this worker request
    /// can produce. This keeps evaluation aligned with the actual LIVE scope:
    /// suffix-only requests cannot repair a homophone already held in the
    /// stable prefix even when explicit Space has that full-sentence candidate.
    #[doc(hidden)]
    #[must_use]
    pub fn rankable_output_surfaces(
        &self,
        request: &CandidateRankingRequest,
    ) -> Option<Vec<String>> {
        if !self.request_matches_ranking_scope(request) {
            return None;
        }
        let stable_surface = if self.request_reopens_stable_prefix(request) {
            ""
        } else {
            self.prefix
                .as_ref()
                .map_or("", |prefix| prefix.surface.as_str())
        };
        Some(
            request
                .candidates
                .iter()
                .map(|candidate| {
                    let mut surface = String::with_capacity(
                        stable_surface.len().saturating_add(candidate.surface.len()),
                    );
                    surface.push_str(stable_surface);
                    surface.push_str(&candidate.surface);
                    surface
                })
                .collect(),
        )
    }

    /// Returns the complete reading represented by the marked text.
    ///
    /// Candidate generation may target only the suffix after a stable prefix,
    /// but confidence policy still needs the full composition length.
    #[must_use]
    pub fn resolved_reading(&self) -> &str {
        &self.resolved_reading
    }

    /// Reports whether the rankable target is still displayed as its literal
    /// reading. A strong dictionary winner may safely replace this unresolved
    /// display without overriding an already-converted synchronous LIVE result.
    #[must_use]
    pub fn target_is_literal(&self) -> bool {
        self.prefix.as_ref().map_or_else(
            || self.base_surface == self.target_reading,
            |prefix| {
                self.base_surface
                    .strip_prefix(&prefix.surface)
                    .is_some_and(|surface| surface == self.target_reading)
            },
        )
    }

    /// Request-aware counterpart used after worker-side ranking scope has
    /// optionally reopened a close topic homophone in the stable prefix.
    #[must_use]
    pub fn request_target_is_literal(&self, request: &CandidateRankingRequest) -> bool {
        self.base_target_surface_for_request(request)
            .is_some_and(|surface| surface == request.reading)
    }

    /// Reports whether the current target still ends in at least `minimum`
    /// reading characters displayed literally as hiragana.
    #[must_use]
    pub fn target_has_unresolved_literal_suffix(&self, minimum: usize) -> bool {
        if minimum == 0 {
            return true;
        }
        let Some(base_target_surface) = self.prefix.as_ref().map_or_else(
            || Some(self.base_surface.as_str()),
            |prefix| self.base_surface.strip_prefix(&prefix.surface),
        ) else {
            return false;
        };
        base_target_surface
            .chars()
            .rev()
            .zip(self.target_reading.chars().rev())
            .take_while(|(surface, reading)| surface == reading && is_hiragana_or_mark(*surface))
            .take(minimum)
            .count()
            == minimum
    }

    /// Request-aware counterpart of [`Self::target_has_unresolved_literal_suffix`].
    #[must_use]
    pub fn request_target_has_unresolved_literal_suffix(
        &self,
        request: &CandidateRankingRequest,
        minimum: usize,
    ) -> bool {
        if minimum == 0 {
            return true;
        }
        let Some(base_target_surface) = self.base_target_surface_for_request(request) else {
            return false;
        };
        base_target_surface
            .chars()
            .rev()
            .zip(request.reading.chars().rev())
            .take_while(|(surface, reading)| surface == reading && is_hiragana_or_mark(*surface))
            .take(minimum)
            .count()
            == minimum
    }

    /// Reports whether the dictionary base preserves the converted prefix
    /// byte-for-byte and replaces only a short literal tail with a specific
    /// multi-ideograph word. This is exposed for the delayed worker's
    /// post-rejection fallback policy; the apply boundary validates the same
    /// shape again before changing marked text.
    #[doc(hidden)]
    #[must_use]
    pub fn dictionary_base_repairs_specific_literal_tail(
        &self,
        request: &CandidateRankingRequest,
    ) -> bool {
        let display_target = if self.request_reopens_stable_prefix(request) {
            Some(self.display_surface.as_str())
        } else {
            self.prefix.as_ref().map_or_else(
                || Some(self.display_surface.as_str()),
                |prefix| self.display_surface.strip_prefix(&prefix.surface),
            )
        };
        let Some(display_target) = display_target else {
            return false;
        };
        request.candidates.first().is_some_and(|candidate| {
            rewrites_literal_tail_as_specific_compound(
                &request.reading,
                display_target,
                &candidate.surface,
            )
        })
    }

    /// Reports whether the model winner would leave the currently displayed
    /// worker target unchanged. A delayed worker uses this to distinguish an
    /// intentional alternative conversion from a no-op before considering a
    /// structurally validated dictionary-base repair.
    #[doc(hidden)]
    #[must_use]
    pub fn ranked_winner_keeps_display_target(
        &self,
        request: &CandidateRankingRequest,
        ranked_surfaces: &[String],
    ) -> bool {
        ranked_surfaces.first().is_some_and(|winner| {
            self.display_target_surface_for_request(request) == Some(winner.as_str())
        })
    }

    /// Reports the target-surface safety checks for the model winner. The
    /// mutable engine separately validates generation, the complete candidate
    /// permutation, and alignment with any reopened prefix before applying it.
    #[doc(hidden)]
    #[must_use]
    pub fn ranked_winner_is_safe(
        &self,
        request: &CandidateRankingRequest,
        ranked_surfaces: &[String],
    ) -> bool {
        ranked_surfaces
            .first()
            .is_some_and(|winner| self.ranked_target_is_safe(request, winner))
    }

    /// Returns an otherwise identical candidate that preserves a protected
    /// short hiragana tail when the model winner changes only that tail. This
    /// lets the delayed worker keep the model's content-word choice without
    /// introducing forms such as `カラ` or `トカ` at the end of a phrase.
    #[doc(hidden)]
    #[must_use]
    pub fn safer_literal_tail_for_ranked_winner(
        &self,
        request: &CandidateRankingRequest,
        ranked_surfaces: &[String],
    ) -> Option<String> {
        if !self.request_matches_ranking_scope(request) {
            return None;
        }
        let selected = ranked_surfaces.first()?;
        let base_target_surface = self.base_target_surface_for_request(request)?;
        if rewrites_literal_tail_as_katakana_list(&request.reading, base_target_surface, selected) {
            return None;
        }
        safer_literal_tail_alternative(request, selected).map(str::to_owned)
    }

    /// Reports whether the context-ranked dictionary base can replace a short
    /// target that synchronous LIVE left on another candidate. This is used
    /// only after neural ranking declines to choose: the candidate must be the
    /// actual generated base for a non-empty left context, not a katakana
    /// spelling, an implicit number, or a short terminal-form verb. The latter
    /// stays with the ranker because a bare dictionary cost cannot distinguish
    /// common alternatives such as `なる`/`成る`/`鳴る` or `つく`/`付く`/
    /// `着く` safely.
    #[doc(hidden)]
    #[must_use]
    pub fn dictionary_base_replaces_short_contextual_target(
        &self,
        request: &CandidateRankingRequest,
    ) -> bool {
        if self.request_reopens_stable_prefix(request)
            || request.left_context.is_empty()
            || !(1..=3).contains(&request.reading.chars().count())
        {
            return false;
        }
        let Some(base_target) = self.base_target_surface_for_request(request) else {
            return false;
        };
        let Some(candidate) = request.candidates.first() else {
            return false;
        };
        let terminal_kana = request.reading.chars().last();
        if candidate.surface == base_target
            || candidate.surface == text_transform::full_katakana(&request.reading)
            || request.reading == "とか"
            || terminal_kana.is_some_and(|last| {
                matches!(
                    last,
                    'う' | 'く' | 'ぐ' | 'す' | 'つ' | 'ぬ' | 'ぶ' | 'む' | 'る'
                ) && candidate.surface.ends_with(last)
            })
            || (contains_decimal_digit(&candidate.surface)
                && !contains_decimal_digit(&request.reading))
        {
            return false;
        }
        true
    }

    /// Generates the bounded dictionary N-best represented by this snapshot.
    #[must_use]
    pub fn candidate_ranking_request(&self) -> Option<CandidateRankingRequest> {
        if self.original_prefix_before_boundary_reopen.is_some() {
            return self.candidate_ranking_request_for_scope(false);
        }

        let reopens_stable_prefix = self.prefix.as_ref().is_some_and(|prefix| {
            let topic_challenger = self.target_reading.chars().count()
                <= MAX_REOPENED_STABLE_TOPIC_SUFFIX_CHARACTERS
                && live_conversion::stable_prefix_has_close_topic_challenger(
                    &self.dictionary,
                    &prefix.reading,
                    &prefix.surface,
                );
            let bounded_surface_challenger = self
                .base_surface
                .strip_prefix(&prefix.surface)
                .is_some_and(|target_surface| {
                    live_conversion::stable_prefix_has_bounded_surface_challenger(
                        &self.dictionary,
                        &self.resolved_reading,
                        &self.base_surface,
                        &prefix.surface,
                        &self.target_reading,
                        target_surface,
                        &self.left_context,
                    )
                });
            topic_challenger || bounded_surface_challenger
        });
        self.candidate_ranking_request_for_scope(reopens_stable_prefix)
    }

    /// Generates the original suffix-only request after an optional bounded
    /// full-reading repair was rejected. This lets the delayed worker retain
    /// an otherwise useful target conversion without weakening the full
    /// request's confidence boundary.
    #[doc(hidden)]
    #[must_use]
    pub fn candidate_ranking_request_without_reopening(&self) -> Option<CandidateRankingRequest> {
        self.prefix
            .as_ref()
            .and_then(|_| self.candidate_ranking_request_for_scope(false))
    }

    /// The lattice's literal fallback carries `i32::MAX` because Space
    /// conversion should rank real dictionary paths first. Delayed LIVE
    /// ranking has a different job: when the currently displayed target is
    /// still literal, the language model must be able to preserve a normal
    /// hiragana function word or colloquial form instead of being forced into
    /// カタカナ/漢字 by an unreachable fallback cost.
    fn preserve_displayed_literal(&self, request: &mut CandidateRankingRequest) {
        let Some(best) = request.candidates.first() else {
            return;
        };
        let reading_length = request.reading.chars().count();
        let should_preserve_literal = self.request_target_is_literal(request)
            && ((reading_length == 1 && !request.left_context.is_empty())
                || request.reading == "とか"
                || (reading_length >= 4
                    && best.surface == text_transform::full_katakana(&request.reading))
                || (reading_length >= LIVE_LITERAL_PRESERVATION_MIN_CHARACTERS
                    && converted_surface_run_count(&best.surface) == 1));
        if !should_preserve_literal {
            return;
        }
        let literal_cost = best.cost.saturating_add(LIVE_LITERAL_PRESERVATION_COST_GAP);
        if let Some(literal) = request
            .candidates
            .iter_mut()
            .find(|candidate| candidate.surface == request.reading)
        {
            literal.cost = literal_cost;
        } else {
            request.candidates.pop();
            request.candidates.push(CandidateRankingItem {
                surface: request.reading.clone(),
                cost: literal_cost,
            });
        }
    }

    fn candidate_ranking_request_for_scope(
        &self,
        reopens_stable_prefix: bool,
    ) -> Option<CandidateRankingRequest> {
        let (target_reading, left_context) = if reopens_stable_prefix {
            (&self.resolved_reading, &self.external_left_context)
        } else {
            (&self.target_reading, &self.left_context)
        };
        let candidate_limit =
            if target_reading.chars().count() >= LIVE_WIDE_RANKING_MIN_READING_CHARACTERS {
                LIVE_RANKING_CANDIDATE_LIMIT
            } else {
                EXPLICIT_RANKING_CANDIDATE_LIMIT
            };
        let candidates = if left_context.is_empty() {
            self.dictionary
                .candidates_with_limit(target_reading, candidate_limit)
        } else {
            self.dictionary.candidates_with_context_limit(
                target_reading,
                left_context,
                candidate_limit,
            )
        };
        let mut candidates: Vec<_> = candidates.into_iter().take(candidate_limit).collect();
        diversify_implicit_numeric_live_candidates(
            &self.dictionary,
            target_reading,
            &mut candidates,
        );
        let candidates = candidates
            .into_iter()
            .map(|candidate| CandidateRankingItem {
                surface: candidate.surface,
                cost: candidate.cost,
            })
            .collect();
        let mut request = CandidateRankingRequest {
            reading: target_reading.clone(),
            left_context: left_context.clone(),
            candidates,
        };
        for candidate in self.scoped_verb_auxiliary_candidates(&request) {
            if !request
                .candidates
                .iter()
                .any(|c| c.surface == candidate.surface)
            {
                request.candidates.push(CandidateRankingItem {
                    surface: candidate.surface.clone(),
                    cost: candidate.cost,
                });
            }
        }
        self.preserve_displayed_literal(&mut request);
        let has_long_literal_suffix =
            self.request_target_has_unresolved_literal_suffix(&request, 4);
        diversify_recombined_live_candidates(
            &self.dictionary,
            &mut request,
            reopens_stable_prefix,
            has_long_literal_suffix,
        );
        if self.original_prefix_before_boundary_reopen.is_some() && !reopens_stable_prefix {
            let mut added = 0;
            for candidate in &self.boundary_alternatives {
                if request
                    .candidates
                    .iter()
                    .any(|existing| existing.surface == candidate.surface)
                {
                    continue;
                }
                request.candidates.push(candidate.clone());
                added += 1;
                if added == 4 {
                    break;
                }
            }
        }
        (request.candidates.len() >= 2).then_some(request)
    }
}

fn diversify_recombined_live_candidates(
    dictionary: &Dictionary,
    request: &mut CandidateRankingRequest,
    reopens_stable_prefix: bool,
    has_long_literal_suffix: bool,
) {
    if reopens_stable_prefix
        || has_long_literal_suffix
        || request.reading.chars().count() < LIVE_RECOMBINED_DIVERSITY_MIN_READING_CHARACTERS
        || request.candidates.len() < 2
    {
        return;
    }
    let additions: Vec<_> = dictionary
        .recombined_n_best_variants(
            &request.reading,
            EXPANDED_N_BEST,
            LIVE_RECOMBINED_DIVERSITY_CANDIDATES,
        )
        .into_iter()
        .filter(|candidate| {
            !request
                .candidates
                .iter()
                .any(|existing| existing.surface == candidate.surface)
        })
        .map(|candidate| CandidateRankingItem {
            surface: candidate.surface,
            cost: candidate.cost,
        })
        .collect();
    if additions.is_empty() {
        return;
    }

    let replaceable: Vec<_> = (1..request.candidates.len())
        .rev()
        .filter(|&index| request.candidates[index].surface != request.reading)
        .take(additions.len())
        .collect();
    let replacement_count = replaceable.len();
    for index in replaceable {
        request.candidates.remove(index);
    }
    request
        .candidates
        .extend(additions.into_iter().take(replacement_count));
}

fn diversify_implicit_numeric_live_candidates(
    dictionary: &Dictionary,
    reading: &str,
    candidates: &mut Vec<Candidate>,
) {
    let Some(base) = candidates.first() else {
        return;
    };
    if reading.chars().count() < LIVE_WIDE_RANKING_MIN_READING_CHARACTERS
        || contains_decimal_digit(reading)
        || !contains_decimal_digit(&base.surface)
        || candidates.iter().skip(1).any(|candidate| {
            candidate.surface != reading
                && !contains_decimal_digit(&candidate.surface)
                && candidate.cost
                    <= base
                        .cost
                        .saturating_add(LIVE_IMPLICIT_NUMERIC_EXISTING_REPAIR_COST_GAP)
        })
    {
        return;
    }

    let repair_prior_cost = base
        .cost
        .saturating_add(LIVE_IMPLICIT_NUMERIC_REPAIR_PRIOR_COST_GAP);
    let replacements: Vec<_> = dictionary
        .fixed_segment_candidates(
            reading,
            LIVE_IMPLICIT_NUMERIC_FIXED_SEGMENT_ENTRIES,
            LIVE_IMPLICIT_NUMERIC_FIXED_SEGMENT_CANDIDATES,
        )
        .into_iter()
        .filter(|candidate| {
            !contains_decimal_digit(&candidate.surface)
                && !candidates
                    .iter()
                    .any(|existing| existing.surface == candidate.surface)
        })
        .map(|mut candidate| {
            candidate.cost = candidate.cost.min(repair_prior_cost);
            candidate
        })
        .take(LIVE_IMPLICIT_NUMERIC_DIVERSITY_REPLACEMENTS)
        .collect();
    if replacements.is_empty() {
        return;
    }

    let replaceable: Vec<_> = (1..candidates.len())
        .rev()
        .filter(|&index| contains_decimal_digit(&candidates[index].surface))
        .take(replacements.len())
        .collect();
    let replacement_count = replaceable.len();
    for index in replaceable {
        candidates.remove(index);
    }
    candidates.extend(replacements.into_iter().take(replacement_count));
}

fn ranked_surfaces_match_request(
    request: &CandidateRankingRequest,
    ranked_surfaces: &[String],
) -> bool {
    ranked_surfaces.len() == request.candidates.len()
        && !ranked_surfaces.iter().enumerate().any(|(index, surface)| {
            ranked_surfaces[..index].contains(surface)
                || !request
                    .candidates
                    .iter()
                    .any(|candidate| candidate.surface == *surface)
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Composing,
    Converting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct EnginePreferences {
    pub live_conversion: bool,
    pub history_completion: bool,
    pub history_learning: bool,
    pub dictionary_packs: u32,
    pub private_mode: bool,
    pub date_format_mask: u32,
}

impl Default for EnginePreferences {
    fn default() -> Self {
        Self {
            live_conversion: false,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: date_time_candidates::ALL_FORMATS,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateKind {
    Conversion,
    SegmentedConversion,
    Completion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConversionSearch {
    Initial,
    Expanded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditableSegment {
    reading: String,
    surface: String,
    explicitly_selected: bool,
}

struct LiveContextualSurface {
    text: String,
    particle_extension: bool,
}

impl LiveContextualSurface {
    fn protects_preview(
        &self,
        preview: Option<&LivePreview>,
        prefix: Option<&LiveStablePrefix>,
    ) -> bool {
        if !self.particle_extension {
            return true;
        }
        let Some(preview) = preview else {
            return false;
        };
        let target = match prefix {
            Some(prefix) => preview.surface.strip_prefix(&prefix.surface),
            None => Some(preview.surface.as_str()),
        };
        target == Some(self.text.as_str())
    }
}

#[derive(Clone, Debug)]
struct LivePreview {
    /// Complete kana reading covered by `surface`.
    reading: String,
    surface: String,
    /// Whether the current surface is backed by a confident path, rather than
    /// only being a literal extension carried through a temporary ambiguity.
    reliable: bool,
    /// Whether the confident path is a short or weak exact word that commonly
    /// disappears as soon as the next kana extends the reading.
    prefix_fragile: bool,
    /// Whether the current right edge is a delayed-lock bunsetsu boundary.
    sealable_bunsetsu: bool,
    /// A lattice-validated prefix that stays in marked text while later live
    /// conversion searches operate only on the right-hand suffix.
    stable_prefix: Option<LiveStablePrefix>,
    /// A previously sealable left edge remembered while incomplete right-hand
    /// input temporarily makes the full-reading lattice cross that boundary.
    /// It becomes stable only after a later confident full path independently
    /// aligns with both its reading and surface.
    pending_prefix: Option<LiveStablePrefix>,
    /// A complete dictionary word established to the right of
    /// `pending_prefix`. It can protect that word from returning to hiragana
    /// without promoting either boundary to a stable suffix-only search.
    word_checkpoint: Option<LiveStablePrefix>,
    /// A punctuation-closed prefix used only while the following full-reading
    /// search is ambiguous. Unlike `stable_prefix`, this never narrows the
    /// lattice search and may be replaced by a later confident full result.
    fallback_prefix: Option<LiveStablePrefix>,
    checkpoint_kind: LiveCheckpointKind,
}

impl LivePreview {
    fn is_continuity_checkpoint(&self) -> bool {
        matches!(
            self.checkpoint_kind,
            LiveCheckpointKind::Continuity | LiveCheckpointKind::CloseContinuity
        )
    }

    fn is_close_continuity_checkpoint(&self) -> bool {
        self.checkpoint_kind == LiveCheckpointKind::CloseContinuity
    }

    fn is_literal_extension_checkpoint(&self) -> bool {
        self.checkpoint_kind == LiveCheckpointKind::LiteralExtension
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveCheckpointKind {
    None,
    /// A stable suffix retains its display through ambiguity. Worker ranking
    /// still treats that suffix as unresolved, allowing full prefix repair.
    SuffixContinuity,
    /// `fallback_prefix` represents a bounded, lattice-backed path continuity
    /// decision rather than a punctuation or generic soft fallback.
    Continuity,
    /// A close N-best path supports the displayed prefix even though the
    /// dictionary winner crosses it. This is display-only: delayed ranking is
    /// withheld until later input resolves the competing paths.
    CloseContinuity,
    /// One newly resolved kana displays a sealable previous bunsetsu plus a
    /// literal tail. Unlike lattice continuity this is never fed back into the
    /// next synchronous search.
    LiteralExtension,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveStablePrefix {
    reading: String,
    surface: String,
}

struct LiveTargetDecision {
    surface: String,
    ends_bunsetsu: bool,
    kind: LiveTargetDecisionKind,
}

#[derive(Clone, Copy)]
struct LiveSuffixBoundaryDecision {
    confident: bool,
    stable_extension: bool,
    ends_bunsetsu: bool,
}

struct LiveSuffixPrefixContext<'a> {
    previous: &'a LivePreview,
    stable: &'a LiveStablePrefix,
    resolved: &'a str,
    surface: &'a str,
    pending: Option<LiveStablePrefix>,
    protected_pending: Option<LiveStablePrefix>,
    decision: LiveSuffixBoundaryDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveInstabilityGuard {
    /// Converted reading that most recently fell back to an all-literal
    /// extension in this composition.
    rolled_back_reading: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DelayedLiveRankingAvailability {
    Unavailable,
    Available,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveTargetDecisionKind {
    Confident,
    StableExtension,
    LatticeFallback,
    SuffixContinuity,
    ProtectedLiteral,
    Literal,
}

const MINIMUM_LIVE_STABLE_PREFIX_CHARACTERS: usize = 2;
const MINIMUM_LIVE_STABLE_TARGET_CHARACTERS: usize = 4;
const MAXIMUM_LIVE_INSTABILITY_GUARD_EXTENSION_CHARACTERS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
struct CandidateCorrection {
    surface: String,
    reading: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransformStyle {
    Hiragana,
    FullKatakana,
    HalfKatakana,
    FullAlphanumeric,
    HalfAlphanumeric,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    pub preedit: String,
    pub candidates: Vec<String>,
    pub selected: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct SlimeEngine {
    live_generation: u64,
    dictionary: Dictionary,
    romaji: RomajiComposer,
    reading: String,
    /// Keys typed for the whole reading. A deleted kana cannot be mapped back
    /// to its keys, so a kana backspace makes this `None` until the
    /// composition is empty again.
    raw_input: Option<String>,
    candidates: Vec<String>,
    candidate_corrections: Vec<CandidateCorrection>,
    selected: usize,
    candidate_kind: Option<CandidateKind>,
    completion_selected: bool,
    conversion_search: ConversionSearch,
    segments: Vec<EditableSegment>,
    active_segment: usize,
    transformed_surface: Option<String>,
    preferences: EnginePreferences,
    live_preview: Option<LivePreview>,
    live_preview_suppressed: bool,
    delayed_live_ranking: DelayedLiveRankingAvailability,
    /// The prior reading was a prefix-fragile exact word intentionally left
    /// literal for delayed ranking. This is separate from `live_preview` so
    /// deferral preserves the exact synchronous display path that an
    /// ambiguous decision used before delayed ranking was available.
    deferred_fragile_reading: Option<String>,
    live_instability_guard: Option<LiveInstabilityGuard>,
    user_data: UserData,
    installed_packs: DictionaryPackStore,
    dictionary_pack_trust: DictionaryPackTrust,
    session_history: SessionHistory,
    recent_live_selections: Vec<RecentLiveSelection>,
    live_neural_selection: Option<LiveNeuralSelection>,
    candidate_choice: CandidateChoice,
    uses_bundled_dictionary: bool,
    context_annotations: Option<ContextAnnotationCache>,
    /// `(lowercased key, surface)` pairs of ASCII words from the enabled
    /// domain dictionaries and the user dictionary, for reverse matching
    /// English words typed in kana mode.
    ascii_surfaces: Vec<(String, String)>,
}

const MAX_RECENT_LIVE_SELECTIONS: usize = 64;

/// Explicit conversion candidates for one reading.
struct ExplicitCandidates {
    candidates: Vec<String>,
    corrections: Vec<CandidateCorrection>,
    /// Candidates that installed context rules promoted.
    context: Vec<String>,
}

/// Installed context-rule surfaces for one candidate-window reading.
/// Navigation rebuilds annotations on every key, while the dictionary search
/// behind them depends only on this key and on data that `set_preferences`
/// and `reload_user_data` replace.
#[derive(Clone, Debug)]
struct ContextAnnotationCache {
    reading: String,
    previous_surface: String,
    surfaces: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecentLiveSelection {
    reading: String,
    surface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveNeuralSelection {
    generation: u64,
    reading: String,
    surface: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CandidateChoice {
    #[default]
    Default,
    Explicit,
}

impl SlimeEngine {
    #[must_use]
    pub fn new(dictionary: Dictionary) -> Self {
        Self {
            live_generation: next_live_generation(),
            dictionary,
            romaji: RomajiComposer::new(),
            reading: String::new(),
            raw_input: Some(String::new()),
            candidates: Vec::new(),
            candidate_corrections: Vec::new(),
            selected: 0,
            candidate_kind: None,
            completion_selected: false,
            conversion_search: ConversionSearch::Initial,
            segments: Vec::new(),
            active_segment: 0,
            transformed_surface: None,
            preferences: EnginePreferences::default(),
            live_preview: None,
            live_preview_suppressed: false,
            delayed_live_ranking: DelayedLiveRankingAvailability::Unavailable,
            deferred_fragile_reading: None,
            live_instability_guard: None,
            user_data: UserData::default(),
            installed_packs: DictionaryPackStore::default(),
            dictionary_pack_trust: DictionaryPackTrust::default(),
            session_history: SessionHistory::default(),
            recent_live_selections: Vec::new(),
            live_neural_selection: None,
            candidate_choice: CandidateChoice::Default,
            uses_bundled_dictionary: false,
            context_annotations: None,
            ascii_surfaces: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_user_data(dictionary: Dictionary, user_data: UserData) -> Self {
        let mut engine = Self {
            user_data,
            ..Self::new(dictionary)
        };
        engine.rebuild_ascii_surfaces();
        engine
    }

    /// Returns the reading and surface of the current LIVE preview when the
    /// synchronous confidence policy considers it safe to close as a
    /// bunsetsu. This is exposed for transition evaluation only.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_live_sealable_bunsetsu(&self) -> Option<(&str, &str)> {
        self.live_preview
            .as_ref()
            .filter(|preview| preview.sealable_bunsetsu)
            .map(|preview| (preview.reading.as_str(), preview.surface.as_str()))
    }

    /// Returns the stable and pending prefix boundaries of the current LIVE
    /// preview. This is exposed for transition diagnostics only.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_live_prefixes(&self) -> (EvaluationLivePrefix<'_>, EvaluationLivePrefix<'_>) {
        let stable = self.live_preview.as_ref().and_then(|preview| {
            preview
                .stable_prefix
                .as_ref()
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str()))
        });
        let pending = self.live_preview.as_ref().and_then(|preview| {
            preview
                .pending_prefix
                .as_ref()
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str()))
        });
        (stable, pending)
    }

    /// Returns the independent right-side word checkpoint, when present.
    /// This is exposed for transition diagnostics only.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_live_word_checkpoint(&self) -> EvaluationLivePrefix<'_> {
        self.live_preview.as_ref().and_then(|preview| {
            preview
                .word_checkpoint
                .as_ref()
                .map(|checkpoint| (checkpoint.reading.as_str(), checkpoint.surface.as_str()))
        })
    }

    /// Reports whether the current display is a short-lived continuity
    /// checkpoint rather than the logical base used by delayed ranking.
    /// This is exposed for transition diagnostics only.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_live_literal_extension_checkpoint(&self) -> bool {
        self.live_preview
            .as_ref()
            .is_some_and(LivePreview::is_literal_extension_checkpoint)
    }

    /// Configures whether an adapter will actually schedule delayed LIVE
    /// ranking. When available, the synchronous path can leave fragile exact
    /// words literal while the user is still typing; adapters without a
    /// worker retain the immediate dictionary behavior.
    #[doc(hidden)]
    pub fn set_delayed_live_ranking_available(&mut self, available: bool) {
        self.live_generation = next_live_generation();
        self.delayed_live_ranking = if available {
            DelayedLiveRankingAvailability::Available
        } else {
            DelayedLiveRankingAvailability::Unavailable
        };
        if !available {
            self.deferred_fragile_reading = None;
        }
    }

    #[must_use]
    pub fn bundled() -> Self {
        let mut engine = Self::new(Dictionary::bundled());
        engine.uses_bundled_dictionary = true;
        engine
    }

    #[must_use]
    pub fn bundled_with_user_data(user_data: UserData) -> Self {
        Self::bundled_with_user_data_and_pack_trust(user_data, DictionaryPackTrust::default())
    }

    /// Creates a bundled engine whose installed dictionary packs must satisfy
    /// the supplied trust policy. The policy is retained across data reloads.
    #[must_use]
    pub fn bundled_with_user_data_and_pack_trust(
        user_data: UserData,
        dictionary_pack_trust: DictionaryPackTrust,
    ) -> Self {
        let installed_packs =
            DictionaryPackStore::load_with_trust(user_data.directory(), &dictionary_pack_trust);
        let dictionary = bundled_dictionary_with_packs(0, &user_data, &installed_packs);
        let mut engine = Self::with_user_data(dictionary, user_data);
        engine.installed_packs = installed_packs;
        engine.dictionary_pack_trust = dictionary_pack_trust;
        engine.uses_bundled_dictionary = true;
        engine.rebuild_ascii_surfaces();
        engine
    }

    pub fn set_preferences(&mut self, preferences: EnginePreferences) -> Vec<SlimeAction> {
        self.live_generation = next_live_generation();
        if (!self.preferences.private_mode && preferences.private_mode)
            || (self.preferences.history_learning && !preferences.history_learning)
        {
            self.session_history.reset_context();
        }
        if preferences.private_mode || !preferences.live_conversion {
            self.live_neural_selection = None;
            self.deferred_fragile_reading = None;
            self.live_instability_guard = None;
        }
        if self.uses_bundled_dictionary
            && self.preferences.dictionary_packs != preferences.dictionary_packs
        {
            self.dictionary = bundled_dictionary_with_packs(
                preferences.dictionary_packs,
                &self.user_data,
                &self.installed_packs,
            );
        }
        self.preferences = preferences;
        self.context_annotations = None;
        self.rebuild_ascii_surfaces();
        self.live_preview_suppressed = false;
        self.refresh_live_preview();
        self.refresh_completion_actions(true)
    }

    pub fn reload_user_data(&mut self) -> Vec<SlimeAction> {
        self.live_generation = next_live_generation();
        // A reload can remove the commit retained by the current session.
        // Keeping it would recreate a deleted context edge on the next commit.
        self.session_history.reset_context();
        self.recent_live_selections.clear();
        self.user_data.reload();
        self.installed_packs = DictionaryPackStore::load_with_trust(
            self.user_data.directory(),
            &self.dictionary_pack_trust,
        );
        if self.uses_bundled_dictionary {
            self.dictionary = bundled_dictionary_with_packs(
                self.preferences.dictionary_packs,
                &self.user_data,
                &self.installed_packs,
            );
        }
        self.context_annotations = None;
        self.rebuild_ascii_surfaces();
        self.refresh_live_preview();
        self.refresh_completion_actions(true)
    }

    /// Breaks the transient left-context chain after an external caret,
    /// document, or input-client boundary without deleting persisted history.
    pub fn reset_context(&mut self) {
        self.live_generation = next_live_generation();
        self.session_history.reset_context();
    }

    /// Replaces the transient context with committed text owned by the input
    /// client. This surface is bounded in memory, is never persisted, and is
    /// not used to learn a contextual history edge because its reading is
    /// unknown.
    pub fn set_external_left_context(&mut self, surface: &str) {
        self.live_generation = next_live_generation();
        if self.preferences.private_mode {
            self.session_history.reset_context();
        } else {
            self.session_history.set_external_context(surface);
        }
    }

    /// Returns conversion candidates without changing the active composition.
    /// Platform search integrations use this path so querying alternatives
    /// cannot move the user's selection or commit text as a side effect.
    #[must_use]
    pub fn conversion_candidates(&self, reading: &str) -> Vec<String> {
        self.conversion_candidates_for_reading(reading)
    }

    /// Returns conversion candidates for an explicit transient left context.
    ///
    /// This query does not mutate the active composition, session history, or
    /// persisted user data. It is intended for offline pack evaluation and
    /// platform integrations that already own a trusted committed surface.
    #[must_use]
    pub fn conversion_candidates_with_left_context(
        &self,
        previous_surface: &str,
        reading: &str,
    ) -> Vec<String> {
        self.conversion_candidates_for_reading_with_limit_and_context(
            reading,
            None,
            Some(previous_surface),
        )
    }

    /// Records a selection made outside the normal composition UI only when
    /// the surface is one of the engine's current conversions for `reading`.
    pub fn record_external_selection(&mut self, reading: &str, surface: &str) -> bool {
        self.live_generation = next_live_generation();
        if !self
            .conversion_candidates_for_reading(reading)
            .iter()
            .any(|candidate| candidate == surface)
        {
            return false;
        }
        self.record_recent_live_selection(reading, surface);
        self.record_history(reading, surface);
        true
    }

    /// Starts explicit reconversion for a selected committed surface. An
    /// empty action list means the surface has no safe dictionary reading.
    pub fn begin_reconversion(&mut self, surface: &str) -> Vec<SlimeAction> {
        self.live_generation = next_live_generation();
        // The selected text may be anywhere in the document, so the last
        // composition commit is not a valid left neighbor for reconversion.
        self.session_history.reset_context();
        let mut readings = self.dictionary.readings_for_surface(surface);
        if readings.is_empty() {
            let hiragana = text_transform::hiragana(surface);
            if hiragana != surface || surface.chars().all(is_hiragana_or_mark) {
                readings.push(hiragana);
            }
        }
        let Some(reading) = readings.into_iter().next() else {
            return Vec::new();
        };

        self.clear_composition();
        self.reading = reading;
        self.candidates = self.conversion_candidates_for_reading(&self.reading);
        if let Some(index) = self
            .candidates
            .iter()
            .position(|candidate| candidate == surface)
        {
            self.selected = index;
        } else {
            self.candidates.insert(0, surface.to_owned());
            self.selected = 0;
        }
        self.candidate_kind = Some(CandidateKind::Conversion);
        self.candidate_actions()
    }

    fn rebuild_ascii_surfaces(&mut self) {
        self.ascii_surfaces.clear();
        let user_entries = self
            .user_data
            .dictionary_entries()
            .map(|(_, surface)| surface);
        let domain_words = domain_dictionaries::words(self.preferences.dictionary_packs)
            .into_iter()
            .map(|(_, surface)| surface);
        let installed_words = self.installed_packs.words().map(|(_, surface)| surface);
        for surface in user_entries.chain(domain_words).chain(installed_words) {
            if let Some(key) = english_reverse::surface_key(surface)
                && !self
                    .ascii_surfaces
                    .iter()
                    .any(|(_, existing)| existing == surface)
            {
                self.ascii_surfaces.push((key, surface.to_owned()));
            }
        }
    }

    pub fn installed_dictionary_packs(&self) -> impl Iterator<Item = &DictionaryPackInfo> {
        self.installed_packs.infos()
    }

    #[must_use]
    pub fn installed_dictionary_pack_words(&self, id: &str) -> Option<Vec<DictionaryPackWord>> {
        self.installed_packs.pack_words(id)
    }

    #[must_use]
    pub fn dictionary_pack_load_errors(&self) -> &[DictionaryPackLoadError] {
        self.installed_packs.errors()
    }

    /// Returns ASCII surfaces whose spelling the current reading retypes,
    /// exact matches first.
    fn exact_english_reverse_surfaces<'a>(
        &'a self,
        reading: &str,
    ) -> impl Iterator<Item = &'a String> + 'a {
        let reading = (!self.ascii_surfaces.is_empty()).then(|| ReverseReading::new(reading));
        self.ascii_surfaces
            .iter()
            .filter(move |(key, _)| {
                reading
                    .as_ref()
                    .is_some_and(|reading| reading.reverse_match(key) == Some(ReverseMatch::Exact))
            })
            .map(|(_, surface)| surface)
    }

    fn english_reverse_surfaces(&self, target: &str) -> Vec<String> {
        if target.is_empty() || self.ascii_surfaces.is_empty() {
            return Vec::new();
        }
        let mut exact = Vec::new();
        let mut prefix = Vec::new();
        let target = ReverseReading::new(target);
        for (key, surface) in &self.ascii_surfaces {
            match target.reverse_match(key) {
                Some(ReverseMatch::Exact) => exact.push(surface.clone()),
                Some(ReverseMatch::Prefix) => prefix.push(surface.clone()),
                None => {}
            }
        }
        exact.extend(prefix);
        exact.truncate(3);
        exact
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            phase: self.phase(),
            preedit: self.preedit(),
            candidates: self.candidates.clone(),
            selected: (!self.candidates.is_empty()).then_some(self.selected),
        }
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        if matches!(
            self.candidate_kind,
            Some(CandidateKind::Conversion | CandidateKind::SegmentedConversion)
        ) {
            Phase::Converting
        } else {
            Phase::Composing
        }
    }

    /// Returns the dictionary-only candidates that an optional post-ranker may
    /// reorder for the current explicit conversion. Live conversion,
    /// completion, segmented conversion, and private mode never expose a
    /// request.
    #[must_use]
    pub fn candidate_ranking_request(&self) -> Option<CandidateRankingRequest> {
        if self.candidate_kind != Some(CandidateKind::Conversion) || self.preferences.private_mode {
            return None;
        }

        let reading = self.active_candidate_reading();
        if reading.is_empty() {
            return None;
        }
        let limit = explicit_candidate_search_limit(reading);
        let left_context = self.session_history.previous_surface().unwrap_or_default();
        let mut dictionary_candidates = if left_context.is_empty() {
            self.dictionary.candidates_with_limit(reading, limit)
        } else {
            self.dictionary
                .candidates_with_context_limit(reading, left_context, limit)
        };
        let original_count = dictionary_candidates.len();
        self.append_explicit_recombinations(
            reading,
            left_context,
            limit,
            &mut dictionary_candidates,
        );
        let ranking_limit = limit + dictionary_candidates.len() - original_count;
        let details = self.candidate_details();
        let candidates: Vec<_> = details
            .iter()
            .filter(|detail| detail.annotation == CandidateAnnotation::None)
            .filter_map(|detail| {
                dictionary_candidates
                    .iter()
                    .find(|candidate| candidate.surface == detail.value)
                    .map(|candidate| CandidateRankingItem {
                        surface: detail.value.clone(),
                        cost: candidate.cost,
                    })
            })
            .take(ranking_limit)
            .collect();
        (candidates.len() >= 2).then(|| CandidateRankingRequest {
            reading: reading.to_owned(),
            left_context: left_context.to_owned(),
            candidates,
        })
    }

    fn live_snapshot_base_surface(
        preview: Option<&LivePreview>,
        prefix: Option<&LiveStablePrefix>,
        resolved_reading: &str,
        target_reading: &str,
        display_surface: &str,
    ) -> String {
        if preview
            .is_some_and(|preview| preview.checkpoint_kind == LiveCheckpointKind::SuffixContinuity)
        {
            prefix.map_or_else(
                || resolved_reading.to_owned(),
                |prefix| joined_live_surface(&prefix.surface, target_reading),
            )
        } else if prefix.is_none()
            && preview.is_some_and(|preview| {
                preview.pending_prefix.is_some()
                    || preview.is_continuity_checkpoint()
                    || preview.is_literal_extension_checkpoint()
            })
        {
            resolved_reading.to_owned()
        } else {
            display_surface.to_owned()
        }
    }

    fn boundary_reopen_is_personalized(
        &self,
        prefix: Option<&LiveStablePrefix>,
        resolved_reading: &str,
        external_left_context: &str,
    ) -> bool {
        prefix.is_some_and(|prefix| {
            let Some(last) = prefix.reading.chars().last() else {
                return false;
            };
            if !is_likely_particle_character(last) || !prefix.surface.ends_with(last) {
                return false;
            }
            let expanded = &resolved_reading[prefix.reading.len() - last.len_utf8()..];
            let mut context = external_left_context.to_owned();
            context.push_str(&prefix.surface[..prefix.surface.len() - last.len_utf8()]);
            self.user_data
                .has_personalization_in_reading(expanded, self.history_is_available())
                || self
                    .recent_live_selections
                    .iter()
                    .any(|selection| expanded.contains(&selection.reading))
                || self
                    .live_contextual_surface(expanded, &context, true)
                    .is_some()
        })
    }

    /// Captures a generation-safe LIVE ranking snapshot without running the
    /// wider candidate search. Explicit choices and user-dictionary surfaces
    /// remain protected from automatic model replacement.
    #[must_use]
    pub fn live_candidate_ranking_snapshot(&self) -> Option<LiveCandidateRankingSnapshot> {
        if !self.preferences.live_conversion
            || self.preferences.private_mode
            || self.phase() != Phase::Composing
            || self.live_preview_suppressed
            || !self.romaji.pending().is_empty()
            || self
                .live_preview
                .as_ref()
                .is_some_and(LivePreview::is_close_continuity_checkpoint)
        {
            return None;
        }
        let resolved_reading = self.resolved_reading();
        let preview = self.live_preview.as_ref();
        if preview.is_some_and(|preview| preview.reading != resolved_reading) {
            return None;
        }

        let prefix = preview.and_then(|preview| preview.stable_prefix.clone());
        let target_reading = if let Some(prefix) = prefix.as_ref() {
            resolved_reading.strip_prefix(&prefix.reading)?.to_owned()
        } else {
            resolved_reading.clone()
        };
        let target_character_count = target_reading.chars().count();
        let has_left_context = prefix.is_some()
            || self
                .session_history
                .previous_surface()
                .is_some_and(|surface| !surface.is_empty());
        let allows_contextual_single_kana = target_character_count == 1 && has_left_context;
        if (target_character_count < live_conversion::MINIMUM_READING_CHARACTERS
            && !allows_contextual_single_kana)
            || self.live_ranking_target_is_personalized(&target_reading)
        {
            return None;
        }

        let external_left_context = self
            .session_history
            .previous_surface()
            .unwrap_or_default()
            .to_owned();
        let mut left_context = external_left_context.clone();
        if let Some(prefix) = prefix.as_ref() {
            left_context.push_str(&prefix.surface);
        }
        if self
            .live_contextual_surface(&target_reading, &left_context, prefix.is_some())
            .is_some_and(|surface| surface.protects_preview(preview, prefix.as_ref()))
        {
            return None;
        }
        let display_surface = self.preedit();
        let base_surface = Self::live_snapshot_base_surface(
            preview,
            prefix.as_ref(),
            &resolved_reading,
            &target_reading,
            &display_surface,
        );
        let boundary_is_personalized = self.boundary_reopen_is_personalized(
            prefix.as_ref(),
            &resolved_reading,
            &external_left_context,
        );
        Some(LiveCandidateRankingSnapshot {
            generation: self.live_generation,
            dictionary: self.dictionary.clone(),
            resolved_reading,
            target_reading,
            left_context,
            external_left_context,
            display_surface,
            base_surface,
            prefix,
            original_prefix_before_boundary_reopen: None,
            boundary_is_personalized,
            boundary_alternatives: Vec::new(),
            boundary_paths: Vec::new(),
            prefix_repair_paths: OnceLock::new(),
            joint_repair_paths: OnceLock::new(),
            guided_joint_repair_approval: OnceLock::new(),
            object_inflection_paths: OnceLock::new(),
            verb_auxiliary_candidates: OnceLock::new(),
            verb_auxiliary_approval: OnceLock::new(),
            object_inflection_approval: OnceLock::new(),
            pending_prefix: preview.and_then(|preview| preview.pending_prefix.clone()),
            word_checkpoint: preview.and_then(|preview| preview.word_checkpoint.clone()),
            literal_extension_checkpoint: preview
                .is_some_and(LivePreview::is_literal_extension_checkpoint),
        })
    }

    fn live_ranking_target_is_personalized(&self, reading: &str) -> bool {
        self.user_data
            .exact_dictionary_surfaces(reading)
            .next()
            .is_some()
            || self.recent_live_selection_surface(reading).is_some()
            || (self.history_is_available()
                && self
                    .user_data
                    .repeated_live_phrase_surface(reading)
                    .is_some())
    }

    /// Applies a completed LIVE ranking only while the mutable engine still
    /// matches the snapshot. Stale results, changed candidate sets, private
    /// mode, and duplicate or foreign model output are ignored.
    pub fn apply_live_candidate_ranking(
        &mut self,
        snapshot: &LiveCandidateRankingSnapshot,
        request: &CandidateRankingRequest,
        ranked_surfaces: &[String],
    ) -> Option<Vec<SlimeAction>> {
        if self.live_generation != snapshot.generation
            || !self.preferences.live_conversion
            || self.preferences.private_mode
            || self.phase() != Phase::Composing
            || self.live_preview_suppressed
            || !self.romaji.pending().is_empty()
            || self.resolved_reading() != snapshot.resolved_reading
            || !snapshot.request_matches_ranking_scope(request)
        {
            return None;
        }
        if !self.matches_live_candidate_snapshot(snapshot) {
            return None;
        }
        let mut current_left_context = self
            .session_history
            .previous_surface()
            .unwrap_or_default()
            .to_owned();
        let reopens_stable_prefix = snapshot.request_reopens_stable_prefix(request);
        if !reopens_stable_prefix && let Some(prefix) = snapshot.prefix.as_ref() {
            current_left_context.push_str(&prefix.surface);
        }
        if current_left_context != request.left_context
            || !ranked_surfaces_match_request(request, ranked_surfaces)
        {
            return None;
        }

        let selected = ranked_surfaces.first()?;
        let surface = snapshot.selected_surface_for_application(request, selected)?;
        let applied_generation = next_live_generation();
        self.live_neural_selection = Some(LiveNeuralSelection {
            generation: applied_generation,
            reading: snapshot.resolved_reading.clone(),
            surface: surface.clone(),
        });
        self.live_preview = Some(LivePreview {
            reading: snapshot.resolved_reading.clone(),
            surface: surface.clone(),
            reliable: true,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: snapshot
                .prefix
                .clone()
                .filter(|prefix| !reopens_stable_prefix || surface.starts_with(&prefix.surface))
                .or_else(|| {
                    snapshot.pending_prefix.as_ref().and_then(|prefix| {
                        (snapshot.resolved_reading.starts_with(&prefix.reading)
                            && surface.starts_with(&prefix.surface))
                        .then(|| prefix.clone())
                    })
                }),
            pending_prefix: snapshot.pending_prefix.clone().filter(|prefix| {
                !surface.starts_with(&prefix.surface)
                    && snapshot.resolved_reading.starts_with(&prefix.reading)
            }),
            word_checkpoint: None,
            fallback_prefix: None,
            checkpoint_kind: LiveCheckpointKind::None,
        });
        self.live_generation = applied_generation;
        Some(vec![SlimeAction::UpdatePreedit(surface)])
    }

    /// Returns the validated delayed LIVE display while its exact input generation
    /// and composition remain current. The worker may have used a dictionary-only
    /// fallback; this does not certify that model scoring ran. Consume it synchronously
    /// under the same exclusive engine borrow when starting Space conversion.
    #[must_use]
    pub fn current_live_neural_surface(&self) -> Option<&str> {
        let selection = self.live_neural_selection.as_ref()?;
        (selection.generation == self.live_generation
            && self.preferences.live_conversion
            && !self.preferences.private_mode
            && self.phase() == Phase::Composing
            && !self.live_preview_suppressed
            && self.romaji.pending().is_empty()
            && selection.reading == self.resolved_reading()
            && selection.surface == self.preedit())
        .then_some(selection.surface.as_str())
    }

    fn matches_live_candidate_snapshot(&self, snapshot: &LiveCandidateRankingSnapshot) -> bool {
        let preview = self.live_preview.as_ref();
        self.preedit() == snapshot.display_surface
            && preview.and_then(|preview| preview.stable_prefix.as_ref())
                == snapshot
                    .original_prefix_before_boundary_reopen
                    .as_ref()
                    .or(snapshot.prefix.as_ref())
            && preview.and_then(|preview| preview.pending_prefix.as_ref())
                == snapshot.pending_prefix.as_ref()
            && preview.and_then(|preview| preview.word_checkpoint.as_ref())
                == snapshot.word_checkpoint.as_ref()
            && preview.is_some_and(LivePreview::is_literal_extension_checkpoint)
                == snapshot.literal_extension_checkpoint
    }

    /// Applies a permutation produced for [`Self::candidate_ranking_request`].
    /// The candidate set must match exactly; protected candidates retain their
    /// positions and the model cannot introduce a surface absent from the
    /// dictionary N-best list.
    pub fn apply_candidate_ranking(
        &mut self,
        ranked_surfaces: &[String],
    ) -> Option<Vec<SlimeAction>> {
        let request = self.candidate_ranking_request()?;
        self.apply_validated_candidate_ranking(&request, ranked_surfaces)
    }

    /// Ranks and applies candidates while the composition is exclusively borrowed.
    /// The callback may decline to rank; returned surfaces must be an exact
    /// permutation. Holding the borrow lets application reuse the dictionary
    /// request without a second search or a cache that could outlive its state.
    pub fn rank_candidates_with(
        &mut self,
        rank: impl FnOnce(&CandidateRankingRequest) -> Option<Vec<String>>,
    ) -> Option<Vec<SlimeAction>> {
        let request = self.candidate_ranking_request()?;
        let ranked = rank(&request)?;
        self.apply_validated_candidate_ranking(&request, &ranked)
    }

    /// Ranks, optionally generates two dictionary combinations, and applies the
    /// result under one exclusive borrow. The callback sees new candidates only
    /// on its second call; an invalid or failed second result keeps the first
    /// validated ranking and does not change the candidate menu size.
    pub fn rank_candidates_with_recombination(
        &mut self,
        mut rank: impl FnMut(&CandidateRankingRequest) -> Option<CandidateRankingPlan>,
    ) -> Option<Vec<SlimeAction>> {
        let request = self.candidate_ranking_request()?;
        let CandidateRankingPlan {
            ranked_surfaces: baseline,
            recombination_sources,
        } = rank(&request)?;
        if !ranked_surfaces_match_request(&request, &baseline) {
            return None;
        }
        let Some(sources) = recombination_sources else {
            return self.apply_validated_candidate_ranking(&request, &baseline);
        };
        let can_expand = request.left_context.is_empty()
            && request.reading.chars().count() >= EXPLICIT_LONG_RANKING_MIN_CHARACTERS
            && baseline.first() == Some(&sources[0])
            && sources
                .iter()
                .all(|surface| request.candidates.iter().any(|c| &c.surface == surface))
            && self
                .candidates
                .first()
                .is_some_and(|surface| request.candidates.iter().any(|c| &c.surface == surface));
        if !can_expand {
            return self.apply_validated_candidate_ranking(&request, &baseline);
        }
        let additions: Vec<_> = self
            .dictionary
            .recombined_candidates_from_surfaces(
                &request.reading,
                [sources[0].as_str(), sources[1].as_str()],
                2,
            )
            .into_iter()
            .filter(|c| !self.candidates.contains(&c.surface))
            .collect();
        if additions.is_empty() {
            return self.apply_validated_candidate_ranking(&request, &baseline);
        }
        let mut expanded = request.clone();
        expanded
            .candidates
            .extend(additions.iter().map(|c| CandidateRankingItem {
                surface: c.surface.clone(),
                cost: c.cost,
            }));
        let Some(final_plan) = rank(&expanded)
            .filter(|plan| ranked_surfaces_match_request(&expanded, &plan.ranked_surfaces))
        else {
            return self.apply_validated_candidate_ranking(&request, &baseline);
        };
        let old_length = self.candidates.len();
        self.candidates
            .extend(additions.into_iter().map(|c| c.surface));
        if let Some(actions) =
            self.apply_validated_candidate_ranking(&expanded, &final_plan.ranked_surfaces)
        {
            return Some(actions);
        }
        self.candidates.truncate(old_length);
        self.apply_validated_candidate_ranking(&request, &baseline)
    }

    fn apply_validated_candidate_ranking(
        &mut self,
        request: &CandidateRankingRequest,
        ranked_surfaces: &[String],
    ) -> Option<Vec<SlimeAction>> {
        if !ranked_surfaces_match_request(request, ranked_surfaces) {
            return None;
        }

        let rankable_positions: Vec<_> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(index, surface)| {
                request
                    .candidates
                    .iter()
                    .any(|candidate| candidate.surface == *surface)
                    .then_some(index)
            })
            .collect();
        if rankable_positions.len() != ranked_surfaces.len() {
            return None;
        }
        for (position, surface) in rankable_positions.into_iter().zip(ranked_surfaces) {
            self.candidates[position].clone_from(surface);
        }
        self.selected = 0;
        Some(self.candidate_actions())
    }

    pub fn handle(&mut self, event: InputEvent) -> Vec<SlimeAction> {
        self.live_generation = next_live_generation();
        match event {
            InputEvent::Character(character) => self.handle_character(character),
            InputEvent::Space => self.start_or_cycle_conversion(),
            InputEvent::NextCandidate => self.next_candidate(),
            InputEvent::PreviousCandidate => self.previous_candidate(),
            InputEvent::SelectCandidate(index) => self.select_candidate(index),
            InputEvent::AcceptCandidate => self.accept_candidate(),
            InputEvent::TransformHiragana => self.transform(TransformStyle::Hiragana),
            InputEvent::TransformFullKatakana => self.transform(TransformStyle::FullKatakana),
            InputEvent::TransformHalfKatakana => self.transform(TransformStyle::HalfKatakana),
            InputEvent::TransformFullAlphanumeric => {
                self.transform(TransformStyle::FullAlphanumeric)
            }
            InputEvent::TransformHalfAlphanumeric => {
                self.transform(TransformStyle::HalfAlphanumeric)
            }
            InputEvent::NextSegment => self.move_segment(true),
            InputEvent::PreviousSegment => self.move_segment(false),
            InputEvent::ExpandSegment => self.resize_segment(true),
            InputEvent::ShrinkSegment => self.resize_segment(false),
            InputEvent::Enter => self.commit(),
            InputEvent::Escape => self.cancel(),
            InputEvent::Backspace => self.backspace(),
        }
    }

    fn handle_character(&mut self, character: char) -> Vec<SlimeAction> {
        let mut actions = Vec::with_capacity(4);
        let had_completions = self.candidate_kind == Some(CandidateKind::Completion);
        if self.phase() == Phase::Converting || self.transformed_surface.is_some() {
            let used_explicit_whole_phrase_choice = self.candidate_kind
                == Some(CandidateKind::Conversion)
                && self.candidate_choice == CandidateChoice::Explicit;
            let committed = self.committed_surface();
            let reading = if self.phase() == Phase::Converting {
                self.selected_learning_reading().to_owned()
            } else {
                self.reading.clone()
            };
            if used_explicit_whole_phrase_choice {
                self.record_recent_live_selection(&reading, &committed);
            }
            self.record_conversion_history(&reading, &committed);
            actions.push(SlimeAction::Commit(committed));
            self.clear_composition();
            actions.push(SlimeAction::HideCandidates);
        } else if had_completions {
            self.clear_candidates();
        }

        if let Some(raw_input) = &mut self.raw_input {
            raw_input.push(character);
        }

        if character.is_ascii_alphabetic()
            || (character == '\'' && matches!(self.romaji.pending(), "n" | "t" | "d"))
        {
            let kana = self
                .romaji
                .push(character.to_ascii_lowercase())
                .expect("ASCII romaji was validated");
            self.reading.push_str(&kana);
        } else {
            self.reading.push_str(&self.romaji.flush());
            self.reading.push(normalize_ascii_character(character));
        }

        actions.extend(self.refresh_composition_actions());
        if had_completions
            && !actions.contains(&SlimeAction::HideCandidates)
            && self.candidates.is_empty()
        {
            actions.push(SlimeAction::HideCandidates);
        }
        actions
    }

    fn start_or_cycle_conversion(&mut self) -> Vec<SlimeAction> {
        let mut actions = Vec::with_capacity(3);
        if matches!(
            self.candidate_kind,
            Some(CandidateKind::Conversion | CandidateKind::SegmentedConversion)
        ) {
            self.candidate_choice = CandidateChoice::Explicit;
            if self.selected + 1 == self.candidates.len() {
                self.expand_conversion_candidates_if_needed();
            }
            self.selected = (self.selected + 1) % self.candidates.len();
            self.update_active_segment_surface();
            return self.candidate_actions();
        }
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.clear_candidates();
            actions.push(SlimeAction::HideCandidates);
        }

        self.reading.push_str(&self.romaji.flush());
        if self.reading.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        let explicit = self.conversion_candidates_with_corrections(
            &self.reading,
            self.complete_raw_input().unwrap_or_default(),
        );
        self.candidates = explicit.candidates;
        self.candidate_corrections = explicit.corrections;
        self.store_context_annotations(self.reading.clone(), explicit.context);
        self.selected = 0;
        self.candidate_kind = Some(CandidateKind::Conversion);
        self.completion_selected = false;
        self.candidate_choice = CandidateChoice::Default;
        actions.extend(self.candidate_actions());
        actions
    }

    fn append_explicit_recombinations(
        &self,
        reading: &str,
        left_context: &str,
        limit: usize,
        candidates: &mut Vec<Candidate>,
    ) {
        // Recombination costs currently have no previous-word connection cost.
        if !left_context.is_empty()
            || limit != EXPLICIT_LONG_RANKING_CANDIDATE_LIMIT
            || reading.chars().count() < EXPLICIT_LONG_RANKING_MIN_CHARACTERS
        {
            return;
        }
        let Some(base) = candidates.first() else {
            return;
        };
        let hiragana: String = base
            .surface
            .chars()
            .filter(|c| matches!(c, '\u{3041}'..='\u{3096}'))
            .collect();
        for candidate in self.dictionary.recombined_n_best_variants(
            reading,
            limit,
            EXPLICIT_RECOMBINED_CANDIDATE_LIMIT,
        ) {
            if candidate
                .surface
                .chars()
                .filter(|c| matches!(c, '\u{3041}'..='\u{3096}'))
                .eq(hiragana.chars())
                && !candidates
                    .iter()
                    .any(|existing| existing.surface == candidate.surface)
            {
                candidates.push(candidate);
            }
        }
    }

    fn conversion_candidates_for_reading(&self, reading: &str) -> Vec<String> {
        self.conversion_candidates_for_reading_with_limit(reading, None)
    }

    fn conversion_candidates_for_reading_with_limit(
        &self,
        reading: &str,
        dictionary_limit: Option<usize>,
    ) -> Vec<String> {
        self.conversion_candidates_for_reading_with_limit_and_context(
            reading,
            dictionary_limit,
            None,
        )
    }

    fn conversion_candidates_for_reading_with_limit_and_context(
        &self,
        reading: &str,
        dictionary_limit: Option<usize>,
        explicit_previous_surface: Option<&str>,
    ) -> Vec<String> {
        self.conversion_candidates_and_context(reading, dictionary_limit, explicit_previous_surface)
            .0
    }

    /// Returns the candidates and, separately, those that installed context
    /// rules promoted, so candidate annotations need not search again.
    fn conversion_candidates_and_context(
        &self,
        reading: &str,
        dictionary_limit: Option<usize>,
        explicit_previous_surface: Option<&str>,
    ) -> (Vec<String>, Vec<String>) {
        let mut candidates = Vec::new();
        let previous_surface = if self.preferences.private_mode {
            None
        } else {
            explicit_previous_surface.or_else(|| self.session_history.previous_surface())
        };
        for surface in self.user_data.exact_dictionary_surfaces(reading) {
            push_unique(&mut candidates, surface.to_owned());
        }
        if self.history_is_available() {
            for surface in
                self.contextual_history_surfaces_for_reading(reading, explicit_previous_surface)
            {
                push_unique(&mut candidates, surface.to_owned());
            }
            for surface in self.user_data.exact_history_surfaces(reading) {
                push_unique(&mut candidates, surface.to_owned());
            }
        }
        for surface in self.exact_english_reverse_surfaces(reading) {
            push_unique(&mut candidates, surface.clone());
        }
        // The literal hiragana reading stays selectable; hiding it made
        // single-kana words like み unreachable through the candidate window.
        let mut dictionary_candidates = match (dictionary_limit, previous_surface) {
            (Some(limit), Some(context)) => self
                .dictionary
                .candidates_with_context_limit(reading, context, limit),
            (None, Some(context)) => self.dictionary.candidates_with_context(reading, context),
            (Some(limit), None) => self.dictionary.candidates_with_limit(reading, limit),
            (None, None) => self.dictionary.candidates(reading),
        };
        if let Some(limit) = dictionary_limit {
            self.append_explicit_recombinations(
                reading,
                previous_surface.unwrap_or_default(),
                limit,
                &mut dictionary_candidates,
            );
        }
        let dictionary_surfaces: Vec<_> = dictionary_candidates
            .into_iter()
            .map(|candidate| candidate.surface)
            .collect();
        for surface in self.contextual_particle_history_surfaces(
            reading,
            explicit_previous_surface,
            &dictionary_surfaces,
        ) {
            push_unique(&mut candidates, surface);
        }
        let context = if self.preferences.private_mode {
            Vec::new()
        } else {
            self.installed_contextual_conversion_surfaces(
                previous_surface.unwrap_or_default(),
                reading,
                &dictionary_surfaces,
            )
        };
        for surface in &context {
            if !candidates.contains(surface) {
                candidates.push(surface.clone());
            }
        }
        for surface in dictionary_surfaces {
            push_unique(&mut candidates, surface);
        }
        insert_visible_katakana_candidate(&mut candidates, reading);
        insert_unique_candidates_after_first(
            &mut candidates,
            date_time_candidates::candidates(reading, self.preferences.date_format_mask),
        );
        (candidates, context)
    }

    fn conversion_candidates_with_corrections(
        &self,
        reading: &str,
        raw_input: &str,
    ) -> ExplicitCandidates {
        let limit = explicit_candidate_search_limit(reading);
        let limit = (limit > EXPLICIT_RANKING_CANDIDATE_LIMIT).then_some(limit);
        let (ordinary, context) = self.conversion_candidates_and_context(reading, limit, None);
        if self.dictionary.has_exact_reading(reading)
            || self
                .user_data
                .exact_dictionary_surfaces(reading)
                .next()
                .is_some()
            || (self.history_is_available()
                && !self.user_data.exact_history_surfaces(reading).is_empty())
            || !self
                .contextual_particle_history_surfaces(reading, None, &ordinary)
                .is_empty()
            || self
                .exact_english_reverse_surfaces(reading)
                .next()
                .is_some()
        {
            return ExplicitCandidates {
                candidates: ordinary,
                corrections: Vec::new(),
                context,
            };
        }

        let mut ranked_corrections: Vec<(CandidateCorrection, (u8, i32))> = Vec::new();
        for corrected in typo_correction::corrected_readings(raw_input, reading) {
            let has_user_entry = self
                .user_data
                .exact_dictionary_surfaces(&corrected.reading)
                .next()
                .is_some();
            if !has_user_entry && !self.dictionary.has_exact_reading(&corrected.reading) {
                continue;
            }

            for (surface, candidate_cost) in self
                .user_data
                .exact_dictionary_surfaces(&corrected.reading)
                .map(|surface| (surface.to_owned(), i32::MIN))
                .chain(
                    self.dictionary
                        .candidates_with_limit(&corrected.reading, 3)
                        .into_iter()
                        .map(|candidate| (candidate.surface, candidate.cost)),
                )
            {
                if surface == corrected.reading {
                    continue;
                }
                let rank = (corrected.edit_priority, candidate_cost);
                let correction = CandidateCorrection {
                    surface,
                    reading: corrected.reading.clone(),
                };
                if let Some((existing, existing_rank)) = ranked_corrections
                    .iter_mut()
                    .find(|(existing, _)| existing.surface == correction.surface)
                {
                    if rank < *existing_rank {
                        *existing = correction;
                        *existing_rank = rank;
                    }
                } else {
                    ranked_corrections.push((correction, rank));
                }
            }
        }

        ranked_corrections.sort_unstable_by(|(left, left_rank), (right, right_rank)| {
            left_rank
                .cmp(right_rank)
                .then_with(|| left.reading.cmp(&right.reading))
                .then_with(|| left.surface.cmp(&right.surface))
        });
        let corrections = select_candidate_corrections(ranked_corrections, 3);

        if corrections.is_empty() {
            return ExplicitCandidates {
                candidates: ordinary,
                corrections,
                context,
            };
        }

        let mut candidates = Vec::with_capacity(ordinary.len() + corrections.len() + 1);
        push_unique(&mut candidates, reading.to_owned());
        for correction in &corrections {
            push_unique(&mut candidates, correction.surface.clone());
        }
        for candidate in ordinary {
            push_unique(&mut candidates, candidate);
        }
        ExplicitCandidates {
            candidates,
            corrections,
            context,
        }
    }

    fn history_is_available(&self) -> bool {
        self.preferences.history_completion && !self.preferences.private_mode
    }

    fn contextual_history_surfaces_for_reading(
        &self,
        reading: &str,
        explicit_previous_surface: Option<&str>,
    ) -> Vec<&str> {
        if !self.history_is_available() {
            return Vec::new();
        }
        if let Some(previous_surface) = explicit_previous_surface {
            return self
                .user_data
                .contextual_history_surfaces_for_external_surface(previous_surface, reading);
        }
        if let Some((previous_reading, previous_surface)) = self.session_history.previous_commit() {
            return self.user_data.contextual_history_surfaces(
                previous_reading,
                previous_surface,
                reading,
            );
        }
        self.session_history
            .previous_surface()
            .map_or_else(Vec::new, |previous_surface| {
                self.user_data
                    .contextual_history_surfaces_for_external_surface(previous_surface, reading)
            })
    }

    fn contextual_particle_history_surfaces(
        &self,
        reading: &str,
        explicit_previous_surface: Option<&str>,
        available: &[String],
    ) -> Vec<String> {
        self.contextual_particle_history_candidates(reading, explicit_previous_surface)
            .into_iter()
            .filter(|surface| available.contains(surface))
            .collect()
    }

    fn contextual_particle_history_candidates(
        &self,
        reading: &str,
        explicit_previous_surface: Option<&str>,
    ) -> Vec<String> {
        let Some((end, particle)) = reading.char_indices().next_back() else {
            return Vec::new();
        };
        if end == 0 || !is_likely_particle_character(particle) {
            return Vec::new();
        }
        self.contextual_history_surfaces_for_reading(&reading[..end], explicit_previous_surface)
            .into_iter()
            .map(|surface| format!("{surface}{particle}"))
            .collect()
    }

    fn next_candidate(&mut self) -> Vec<SlimeAction> {
        if self.candidates.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        if self.selected + 1 == self.candidates.len() {
            self.expand_conversion_candidates_if_needed();
        }
        self.selected = (self.selected + 1) % self.candidates.len();
        if matches!(
            self.candidate_kind,
            Some(CandidateKind::Conversion | CandidateKind::SegmentedConversion)
        ) {
            self.candidate_choice = CandidateChoice::Explicit;
        }
        self.update_active_segment_surface();
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        self.candidate_actions()
    }

    fn previous_candidate(&mut self) -> Vec<SlimeAction> {
        if self.candidates.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        if self.selected == 0 {
            self.expand_conversion_candidates_if_needed();
        }
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.candidates.len() - 1);
        if matches!(
            self.candidate_kind,
            Some(CandidateKind::Conversion | CandidateKind::SegmentedConversion)
        ) {
            self.candidate_choice = CandidateChoice::Explicit;
        }
        self.update_active_segment_surface();
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        self.candidate_actions()
    }

    fn expand_conversion_candidates_if_needed(&mut self) {
        if self.candidate_kind != Some(CandidateKind::Conversion)
            || self.conversion_search == ConversionSearch::Expanded
        {
            return;
        }
        self.conversion_search = ConversionSearch::Expanded;

        let mut merged = self.candidates.clone();
        let reading_length = self.reading.chars().count();
        if reading_length <= MAX_EXPANDED_READING_CHARACTERS {
            for candidate in self
                .conversion_candidates_for_reading_with_limit(&self.reading, Some(EXPANDED_N_BEST))
            {
                push_unique(&mut merged, candidate);
            }
        }
        if reading_length <= MAX_COMPOUND_READING_CHARACTERS {
            for candidate in self.dictionary.compound_candidates(
                &self.reading,
                COMPOUND_ENTRIES_PER_SEGMENT,
                COMPOUND_CANDIDATE_LIMIT,
            ) {
                push_unique(&mut merged, candidate.surface);
            }
        }
        if reading_length > MAX_EXPANDED_READING_CHARACTERS {
            for surface in self.dictionary.fixed_segment_variants(
                &self.reading,
                FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
                FIXED_SEGMENT_CANDIDATE_LIMIT,
            ) {
                push_unique(&mut merged, surface);
            }
            for candidate in self.dictionary.recombined_n_best_variants(
                &self.reading,
                RECOMBINED_SOURCE_PATHS,
                RECOMBINED_CANDIDATE_LIMIT,
            ) {
                push_unique(&mut merged, candidate.surface);
            }
        }
        self.candidates = merged;
    }

    fn select_candidate(&mut self, index: u32) -> Vec<SlimeAction> {
        let index = index as usize;
        if index >= self.candidates.len() {
            return Vec::new();
        }

        self.selected = index;
        if matches!(
            self.candidate_kind,
            Some(CandidateKind::Conversion | CandidateKind::SegmentedConversion)
        ) {
            self.candidate_choice = CandidateChoice::Explicit;
        }
        self.update_active_segment_surface();
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        // Candidate consumers keep their own highlighted row. Always resend
        // the selected index together with the preedit so programmatic,
        // keyboard, number, and pointer selection cannot diverge from the
        // surface that Enter or Finalize will commit.
        self.candidate_actions()
    }

    fn accept_candidate(&mut self) -> Vec<SlimeAction> {
        if self.candidates.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        self.commit()
    }

    fn transform(&mut self, style: TransformStyle) -> Vec<SlimeAction> {
        self.reading.push_str(&self.romaji.flush());
        if self.reading.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            let reading = self.segments[self.active_segment].reading.clone();
            let transformed = transform_text(style, &reading, None);
            self.segments[self.active_segment].surface = transformed;
            self.segments[self.active_segment].explicitly_selected = true;
            self.candidates = vec![self.segments[self.active_segment].surface.clone()];
            self.selected = 0;
            return self.candidate_actions();
        }

        let had_candidates = self.candidate_kind.is_some();
        self.clear_candidates();
        self.transformed_surface = Some(transform_text(
            style,
            &self.reading,
            self.complete_raw_input(),
        ));
        let mut actions = vec![SlimeAction::UpdatePreedit(self.preedit())];
        if had_candidates {
            actions.push(SlimeAction::HideCandidates);
        }
        actions
    }

    fn move_segment(&mut self, forward: bool) -> Vec<SlimeAction> {
        if self.phase() != Phase::Converting {
            return vec![SlimeAction::ForwardKey];
        }
        let entered = self.enter_segment_mode();
        if self.segments.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }
        if !entered || forward {
            if forward {
                self.active_segment = (self.active_segment + 1).min(self.segments.len() - 1);
            } else {
                self.active_segment = self.active_segment.saturating_sub(1);
            }
        }
        self.activate_segment_candidates();
        self.candidate_actions()
    }

    fn resize_segment(&mut self, expand: bool) -> Vec<SlimeAction> {
        if self.phase() != Phase::Converting {
            return vec![SlimeAction::ForwardKey];
        }
        let entered = self.enter_segment_mode();
        if self.segments.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }
        let can_resize = if expand {
            self.active_segment + 1 < self.segments.len()
        } else {
            self.segments[self.active_segment]
                .reading
                .chars()
                .nth(1)
                .is_some()
        };
        if !can_resize {
            if entered {
                self.activate_segment_candidates();
            }
            return self.candidate_actions();
        }

        if expand {
            let next_index = self.active_segment + 1;
            let next_reading = self.segments[next_index].reading.clone();
            let character = next_reading.chars().next().expect("non-empty segment");
            self.segments[self.active_segment].reading.push(character);
            next_reading[character.len_utf8()..].clone_into(&mut self.segments[next_index].reading);
            if self.segments[next_index].reading.is_empty() {
                self.segments.remove(next_index);
            } else {
                self.reset_segment_surface(next_index);
            }
        } else {
            let character = self.segments[self.active_segment]
                .reading
                .pop()
                .expect("a resizable segment has two characters");
            let next_index = self.active_segment + 1;
            if next_index == self.segments.len() {
                self.segments.push(EditableSegment {
                    reading: character.to_string(),
                    surface: character.to_string(),
                    explicitly_selected: false,
                });
            } else {
                self.segments[next_index].reading.insert(0, character);
            }
            self.reset_segment_surface(next_index);
        }
        self.reset_active_segment_candidates();
        self.candidate_actions()
    }

    /// Returns true when this call changed whole-phrase conversion into
    /// segmented conversion. Callers show the active segment's candidates.
    fn enter_segment_mode(&mut self) -> bool {
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            return false;
        }
        let selected_surface = self.selected_candidate().to_owned();
        // Most selections are the 1-best surface. Reuse its segmentation and
        // run the wide N-best search (milliseconds) only for another surface.
        let best = self.dictionary.convert_best(&self.reading);
        let conversion = match best {
            Some(best) if best.surface == selected_surface => Some(best),
            best => self
                .dictionary
                .convert_n_best(&self.reading, 32)
                .into_iter()
                .find(|conversion| conversion.surface == selected_surface)
                .or(best),
        };
        self.segments = conversion.map_or_else(
            || {
                vec![EditableSegment {
                    reading: self.reading.clone(),
                    surface: selected_surface.clone(),
                    explicitly_selected: false,
                }]
            },
            |conversion| {
                conversion
                    .segments
                    .into_iter()
                    .map(editable_segment)
                    .collect()
            },
        );
        if self
            .segments
            .iter()
            .map(|segment| segment.surface.as_str())
            .collect::<String>()
            != selected_surface
        {
            self.segments = vec![EditableSegment {
                reading: self.reading.clone(),
                surface: selected_surface,
                explicitly_selected: false,
            }];
        }
        self.active_segment = 0;
        self.candidate_kind = Some(CandidateKind::SegmentedConversion);
        true
    }

    fn activate_segment_candidates(&mut self) {
        let segment = &self.segments[self.active_segment];
        let reading = segment.reading.clone();
        let surface = segment.surface.clone();
        self.candidate_corrections.clear();
        let (candidates, context) = self.conversion_candidates_and_context(&reading, None, None);
        self.candidates = candidates;
        self.store_context_annotations(reading, context);
        if let Some(index) = self
            .candidates
            .iter()
            .position(|candidate| candidate == &surface)
        {
            self.selected = index;
        } else {
            self.candidates.insert(0, surface);
            self.selected = 0;
        }
    }

    /// Resets the active segment to its best candidate and shows the same
    /// candidate list, generating it once.
    fn reset_active_segment_candidates(&mut self) {
        let reading = self.segments[self.active_segment].reading.clone();
        self.candidate_corrections.clear();
        self.candidates = self.conversion_candidates_for_reading(&reading);
        if self.candidates.is_empty() {
            self.candidates.push(reading);
        }
        self.selected = 0;
        let segment = &mut self.segments[self.active_segment];
        segment.surface.clone_from(&self.candidates[0]);
        segment.explicitly_selected = false;
    }

    fn reset_segment_surface(&mut self, index: usize) {
        let reading = self.segments[index].reading.clone();
        self.segments[index].surface = self
            .conversion_candidates_for_reading(&reading)
            .into_iter()
            .next()
            .unwrap_or(reading);
        self.segments[index].explicitly_selected = false;
    }

    fn candidate_actions(&mut self) -> Vec<SlimeAction> {
        let mut actions = Vec::with_capacity(2);
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            actions.push(self.segmented_preedit_action());
        } else if self.candidate_kind == Some(CandidateKind::Conversion) || self.completion_selected
        {
            actions.push(SlimeAction::UpdatePreedit(
                self.selected_candidate().to_owned(),
            ));
        }
        let details = self.cached_candidate_details();
        actions.push(SlimeAction::ShowCandidates {
            candidates: self.displayed_candidates(),
            details,
            selected: self.selected,
        });
        actions
    }

    fn displayed_candidates(&self) -> Vec<String> {
        self.candidates
            .iter()
            .map(|candidate| {
                self.candidate_corrections
                    .iter()
                    .find(|correction| correction.surface == *candidate)
                    .map_or_else(
                        || candidate.clone(),
                        |correction| format!("{candidate}　（{}に訂正）", correction.reading),
                    )
            })
            .collect()
    }

    fn candidate_details(&self) -> Vec<CandidateDetail> {
        let reading = self.active_candidate_reading();
        if let Some(context) = self.cached_context_annotation_surfaces(reading) {
            return self.candidate_details_with_context(context);
        }
        self.candidate_details_with_context(&self.context_annotation_surfaces(reading))
    }

    /// Like `candidate_details`, but stores the installed context-rule search
    /// so navigation keys reuse it while the reading and left context stay
    /// the same.
    fn cached_candidate_details(&mut self) -> Vec<CandidateDetail> {
        let reading = self.active_candidate_reading();
        if self.uses_context_annotations()
            && self.cached_context_annotation_surfaces(reading).is_none()
        {
            let cache = ContextAnnotationCache {
                reading: reading.to_owned(),
                previous_surface: self
                    .session_history
                    .previous_surface()
                    .unwrap_or_default()
                    .to_owned(),
                surfaces: self.context_annotation_surfaces(reading),
            };
            self.context_annotations = Some(cache);
        }
        self.candidate_details()
    }

    /// Seeds the annotation cache with the context-rule surfaces that the
    /// candidate search for `reading` has just produced.
    fn store_context_annotations(&mut self, reading: String, surfaces: Vec<String>) {
        if self.preferences.private_mode || !self.installed_packs.has_contextual_rules() {
            return;
        }
        self.context_annotations = Some(ContextAnnotationCache {
            reading,
            previous_surface: self
                .session_history
                .previous_surface()
                .unwrap_or_default()
                .to_owned(),
            surfaces,
        });
    }

    fn uses_context_annotations(&self) -> bool {
        self.candidate_kind != Some(CandidateKind::Completion)
            && !self.preferences.private_mode
            && self.installed_packs.has_contextual_rules()
    }

    fn cached_context_annotation_surfaces(&self, reading: &str) -> Option<&[String]> {
        let previous_surface = self.session_history.previous_surface().unwrap_or_default();
        self.context_annotations
            .as_ref()
            .filter(|cache| cache.reading == reading && cache.previous_surface == previous_surface)
            .map(|cache| cache.surfaces.as_slice())
    }

    /// Installed context-rule surfaces among the dictionary candidates for
    /// `reading`, used only to label candidates as `Context`.
    fn context_annotation_surfaces(&self, reading: &str) -> Vec<String> {
        if !self.uses_context_annotations() {
            return Vec::new();
        }
        let limit = explicit_candidate_search_limit(reading);
        let previous_surface = self.session_history.previous_surface().unwrap_or_default();
        let dictionary_surfaces: Vec<_> = if previous_surface.is_empty() {
            self.dictionary.candidates_with_limit(reading, limit)
        } else {
            self.dictionary
                .candidates_with_context_limit(reading, previous_surface, limit)
        }
        .into_iter()
        .map(|candidate| candidate.surface)
        .collect();
        self.installed_contextual_conversion_surfaces(
            previous_surface,
            reading,
            &dictionary_surfaces,
        )
    }

    fn candidate_details_with_context(&self, context: &[String]) -> Vec<CandidateDetail> {
        if self.candidate_kind == Some(CandidateKind::Completion) {
            return self
                .candidates
                .iter()
                .map(|candidate| CandidateDetail {
                    value: candidate.clone(),
                    annotation: CandidateAnnotation::Completion,
                    detail: None,
                })
                .collect();
        }

        let reading = self.active_candidate_reading();
        let user_dictionary: Vec<_> = self.user_data.exact_dictionary_surfaces(reading).collect();
        let mut history = if self.history_is_available() {
            self.user_data.exact_history_surfaces(reading)
        } else {
            Vec::new()
        };
        history.extend(self.contextual_history_surfaces_for_reading(reading, None));
        let particle_history =
            self.contextual_particle_history_surfaces(reading, None, &self.candidates);
        let date_time =
            date_time_candidates::candidates(reading, self.preferences.date_format_mask);
        let numbers = self.dictionary.generated_number_surfaces(reading);
        self.candidates
            .iter()
            .map(|candidate| {
                let correction = self
                    .candidate_corrections
                    .iter()
                    .find(|correction| correction.surface == *candidate);
                let (annotation, detail) = if let Some(correction) = correction {
                    (
                        CandidateAnnotation::Correction,
                        Some(correction.reading.clone()),
                    )
                } else if user_dictionary.contains(&candidate.as_str()) {
                    (CandidateAnnotation::UserDictionary, None)
                } else if history.contains(&candidate.as_str())
                    || particle_history.contains(candidate)
                {
                    (CandidateAnnotation::History, None)
                } else if context.contains(candidate) {
                    (CandidateAnnotation::Context, None)
                } else if date_time.contains(candidate) {
                    (CandidateAnnotation::DateTime, None)
                } else if numbers.contains(candidate) {
                    (CandidateAnnotation::Number, None)
                } else {
                    (CandidateAnnotation::None, None)
                };
                CandidateDetail {
                    value: candidate.clone(),
                    annotation,
                    detail,
                }
            })
            .collect()
    }

    fn active_candidate_reading(&self) -> &str {
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            &self.segments[self.active_segment].reading
        } else {
            &self.reading
        }
    }

    fn selected_learning_reading(&self) -> &str {
        let selected = self.selected_candidate();
        self.candidate_corrections
            .iter()
            .find(|correction| correction.surface == selected)
            .map_or(self.reading.as_str(), |correction| {
                correction.reading.as_str()
            })
    }

    fn commit(&mut self) -> Vec<SlimeAction> {
        // Capture the exact marked text before flushing pending romaji. Live
        // conversion may intentionally retain a converted prefix and a
        // literal suffix, and Enter must commit exactly what was visible.
        let displayed = self.preedit();
        self.reading.push_str(&self.romaji.flush());
        let used_conversion = matches!(
            self.candidate_kind,
            Some(CandidateKind::Conversion | CandidateKind::SegmentedConversion)
        );
        let used_completion =
            self.candidate_kind == Some(CandidateKind::Completion) && self.completion_selected;
        let used_explicit_whole_phrase_choice = self.candidate_kind
            == Some(CandidateKind::Conversion)
            && self.candidate_choice == CandidateChoice::Explicit;
        let committed = if used_conversion || used_completion {
            self.committed_surface()
        } else if let Some(transformed) = &self.transformed_surface {
            transformed.clone()
        } else {
            displayed
        };

        if committed.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        let reading = self.reading.clone();
        if used_completion {
            self.record_completion_history(&reading, &committed);
        } else if used_conversion {
            let learning_reading = self.selected_learning_reading().to_owned();
            if used_explicit_whole_phrase_choice {
                self.record_recent_live_selection(&learning_reading, &committed);
            }
            self.record_conversion_history(&learning_reading, &committed);
        } else {
            // Live conversion is an implicit presentation decision, not an
            // explicit candidate choice. Learning it would let an unnoticed
            // mistake immediately override the confidence gate next time.
            self.session_history.reset_context();
        }
        let had_candidates = self.candidate_kind.is_some();
        self.clear_composition();
        let mut actions = vec![SlimeAction::Commit(committed), SlimeAction::Clear];
        if had_candidates {
            actions.push(SlimeAction::HideCandidates);
        }
        actions
    }

    fn cancel(&mut self) -> Vec<SlimeAction> {
        if self.candidate_kind.is_some() {
            self.clear_candidates();
            return vec![
                SlimeAction::HideCandidates,
                SlimeAction::UpdatePreedit(self.preedit()),
            ];
        }

        if self.live_preview.is_some() && !self.live_preview_suppressed {
            self.live_preview_suppressed = true;
            return vec![SlimeAction::UpdatePreedit(self.preedit())];
        }

        if self.reading.is_empty() && self.romaji.pending().is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        self.clear_composition();
        vec![SlimeAction::Clear]
    }

    fn backspace(&mut self) -> Vec<SlimeAction> {
        if self.phase() == Phase::Converting || self.transformed_surface.is_some() {
            self.clear_candidates();
            self.transformed_surface = None;
            return vec![
                SlimeAction::HideCandidates,
                SlimeAction::UpdatePreedit(self.preedit()),
            ];
        }
        let had_completions = self.candidate_kind == Some(CandidateKind::Completion);
        if had_completions {
            self.clear_candidates();
        }

        if self.romaji.backspace() {
            if let Some(raw_input) = &mut self.raw_input {
                raw_input.pop();
            }
        } else {
            self.reading.pop();
            self.raw_input = self.reading.is_empty().then(String::new);
        }

        let mut actions = self.refresh_composition_actions();
        if had_completions
            && !actions.contains(&SlimeAction::HideCandidates)
            && self.candidates.is_empty()
        {
            actions.push(SlimeAction::HideCandidates);
        }
        actions
    }

    /// Keys typed for the whole reading, or `None` once they no longer spell it.
    fn complete_raw_input(&self) -> Option<&str> {
        self.raw_input
            .as_deref()
            .filter(|raw_input| !raw_input.is_empty())
    }

    fn preedit(&self) -> String {
        if let Some(transformed) = &self.transformed_surface {
            return transformed.clone();
        }
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            return self.segmented_surface();
        }
        if self.candidate_kind == Some(CandidateKind::Conversion)
            || (self.candidate_kind == Some(CandidateKind::Completion) && self.completion_selected)
        {
            return self.selected_candidate().to_owned();
        }

        if let Some(preview) = &self.live_preview
            && !self.live_preview_suppressed
        {
            let resolved = self.resolved_reading();
            if resolved == preview.reading {
                return preview.surface.clone();
            }

            // Keep the surface only while the next kana is still pending.
            // Once a vowel resolves the suffix, the whole reading is ranked
            // again, so a stale segmentation can never leak into the result.
            if !self.romaji.pending().is_empty() && self.reading == preview.reading {
                let mut preedit =
                    String::with_capacity(preview.surface.len() + self.romaji.preview().len());
                preedit.push_str(&preview.surface);
                preedit.push_str(self.romaji.preview());
                return preedit;
            }
        }

        let mut preedit = self.reading.clone();
        preedit.push_str(self.romaji.preview());
        preedit
    }

    fn selected_candidate(&self) -> &str {
        &self.candidates[self.selected]
    }

    fn committed_surface(&self) -> String {
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            self.segmented_surface()
        } else if let Some(transformed) = &self.transformed_surface {
            transformed.clone()
        } else if self.candidate_kind.is_some() {
            self.selected_candidate().to_owned()
        } else {
            self.preedit()
        }
    }

    fn segmented_surface(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.surface.as_str())
            .collect()
    }

    fn segmented_preedit_action(&self) -> SlimeAction {
        let selection_start = self.segments[..self.active_segment]
            .iter()
            .map(|segment| segment.surface.encode_utf16().count())
            .sum();
        let selection_length = self.segments[self.active_segment]
            .surface
            .encode_utf16()
            .count();
        SlimeAction::UpdateSegmentedPreedit {
            text: self.segmented_surface(),
            selection_start,
            selection_length,
        }
    }

    fn update_active_segment_surface(&mut self) {
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion)
            && let Some(segment) = self.segments.get_mut(self.active_segment)
            && let Some(surface) = self.candidates.get(self.selected)
        {
            segment.surface.clone_from(surface);
            segment.explicitly_selected = true;
        }
    }

    fn clear_composition(&mut self) {
        self.romaji.clear();
        self.reading.clear();
        self.raw_input.get_or_insert_default().clear();
        self.live_preview = None;
        self.live_preview_suppressed = false;
        self.deferred_fragile_reading = None;
        self.live_instability_guard = None;
        self.live_neural_selection = None;
        self.segments.clear();
        self.active_segment = 0;
        self.transformed_surface = None;
        self.clear_candidates();
    }

    fn clear_candidates(&mut self) {
        self.candidates.clear();
        self.candidate_corrections.clear();
        self.selected = 0;
        self.candidate_kind = None;
        self.completion_selected = false;
        self.candidate_choice = CandidateChoice::Default;
        self.conversion_search = ConversionSearch::Initial;
        self.segments.clear();
        self.active_segment = 0;
    }

    fn refresh_composition_actions(&mut self) -> Vec<SlimeAction> {
        self.refresh_live_preview();
        let preedit = self.preedit();
        let mut actions = if preedit.is_empty() {
            vec![SlimeAction::Clear]
        } else {
            vec![SlimeAction::UpdatePreedit(preedit)]
        };
        actions.extend(self.refresh_completion_actions(false));
        actions
    }

    /// Returns the reading with the pending romaji resolved the way a flush
    /// would resolve it, so previews and commits are computed on equal input.
    fn resolved_reading(&self) -> String {
        let mut resolved = self.reading.clone();
        resolved.push_str(&self.romaji.clone().flush());
        resolved
    }

    fn refresh_live_preview(&mut self) {
        if !self.preferences.live_conversion {
            self.live_preview = None;
            return;
        }

        // After an explicit `nn` has resolved to `ん`, the next `n` starts a
        // possible `な`/`に`/`ぬ`/`ね`/`の` syllable. Flushing that pending
        // key for speculative live conversion would create a phantom second
        // `ん` and can replace a stable preview with an unrelated candidate
        // (for example ライブ変換 -> ライブ返還ン while typing 変換の).
        // Keep the existing preview until the following vowel resolves the
        // actual kana, then evaluate the full reading below on the next key.
        if self.romaji.pending() == "n"
            && self.reading.ends_with('ん')
            && self
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.reading == self.reading)
        {
            return;
        }

        let resolved = self.resolved_reading();
        self.live_instability_guard = self.live_instability_guard.take().filter(|guard| {
            resolved.starts_with(&guard.rolled_back_reading)
                && resolved
                    .chars()
                    .count()
                    .saturating_sub(guard.rolled_back_reading.chars().count())
                    <= MAXIMUM_LIVE_INSTABILITY_GUARD_EXTENSION_CHARACTERS
        });
        let can_evaluate = !resolved
            .chars()
            .any(|character| character.is_ascii_alphabetic())
            && resolved.chars().count() >= live_conversion::MINIMUM_READING_CHARACTERS;
        let mut previous = self.live_preview.clone();
        if can_evaluate && let Some(previous) = previous.as_mut() {
            reopen_stable_prefix_before_sokuon(previous, &resolved);
        }

        if can_evaluate {
            if let Some(previous) = previous.as_ref()
                && let Some(prefix) = previous.stable_prefix.as_ref()
                && let Some(target_reading) = resolved.strip_prefix(&prefix.reading)
            {
                self.refresh_live_suffix_preview(previous, prefix, &resolved, target_reading);
                return;
            }
            if self.refresh_full_live_preview(previous.as_ref(), &resolved) {
                return;
            }
        }

        if can_evaluate && self.apply_live_fallback_preview(previous.as_ref(), &resolved) {
            return;
        }

        // A pending romaji suffix (`n`, `k`, `sh`, ...) has not extended the
        // kana reading yet. Preserve the preview for exactly that unchanged
        // reading, but discard it as soon as completed kana changes the input.
        if !self.romaji.pending().is_empty()
            && self
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.reading == self.reading)
        {
            return;
        }

        if can_evaluate
            && let Some(previous) = previous.filter(|preview| {
                resolved.starts_with(&preview.reading) && preview.surface != preview.reading
            })
        {
            if let Some(surface) = sealable_nonfragile_live_rollback_surface(&previous, &resolved) {
                self.set_live_literal_extension_checkpoint_preview(&resolved, surface);
                return;
            }
            self.live_instability_guard = Some(LiveInstabilityGuard {
                rolled_back_reading: previous.reading.clone(),
            });
        }
        self.live_preview = None;
    }

    fn refresh_full_live_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
    ) -> bool {
        let follows_deferred_fragile = self.take_deferred_fragile_extension(resolved);
        let pending_prefix = pending_live_prefix(previous, resolved);
        let boundary_fallback = live_boundary_fallback(previous, resolved);
        let previous_display =
            previous.filter(|preview| !self.is_fragile_learned_live_preview(preview));
        let (stable_surface, protected_pending_prefix_surface) =
            full_live_display_surfaces(previous_display, pending_prefix.as_ref(), resolved);
        let mut decision = self.live_conversion_decision_with_pending_prefix_protection(
            resolved,
            previous.map(|preview| preview.surface.as_str()),
            previous.is_some_and(LivePreview::is_continuity_checkpoint),
            stable_surface.as_deref(),
            None,
            protected_pending_prefix_surface,
        );
        self.apply_live_instability_guard_to_decision(&mut decision);
        if pending_prefix
            .as_ref()
            .is_some_and(|prefix| self.apply_live_katakana_crossing(previous, resolved, prefix))
        {
            return true;
        }
        let stable_extension = matches!(&decision, LiveConversionDecision::StableExtension(_));
        match decision {
            LiveConversionDecision::Confident(surface)
            | LiveConversionDecision::StableExtension(surface) => {
                self.set_confident_full_live_preview(
                    previous,
                    resolved,
                    pending_prefix,
                    surface,
                    stable_extension,
                    follows_deferred_fragile,
                );
                true
            }
            LiveConversionDecision::LatticeFallback(surface) => {
                if let Some(prefix) = boundary_fallback.as_ref()
                    && !surface.text.starts_with(&prefix.surface)
                {
                    self.set_live_boundary_fallback_preview(prefix.clone());
                    return true;
                }
                if let Some(prefix) = pending_prefix.as_ref()
                    && !surface.text.starts_with(&prefix.surface)
                {
                    self.set_live_pending_literal_preview(resolved.to_owned(), prefix.clone());
                    return true;
                }
                self.set_live_lattice_fallback(
                    previous,
                    resolved.to_owned(),
                    surface.text,
                    pending_prefix,
                );
                true
            }
            LiveConversionDecision::ProtectedLiteral(surface) => {
                self.set_protected_literal_full_live_preview(
                    previous,
                    resolved,
                    pending_prefix,
                    surface,
                );
                true
            }
            LiveConversionDecision::Continuity(surface) => {
                self.set_live_continuity_checkpoint_preview(resolved, surface);
                true
            }
            LiveConversionDecision::DeferredFragile(surface) => self
                .set_deferred_fragile_full_live_preview(
                    previous,
                    resolved,
                    pending_prefix,
                    boundary_fallback,
                    &surface,
                ),
            LiveConversionDecision::Ambiguous(surface) => self.set_unconfident_full_live_preview(
                previous,
                resolved,
                pending_prefix,
                boundary_fallback,
                Some(&surface),
            ),
            LiveConversionDecision::Literal => self.set_unconfident_full_live_preview(
                previous,
                resolved,
                pending_prefix,
                boundary_fallback,
                None,
            ),
        }
    }

    fn take_deferred_fragile_extension(&mut self, resolved: &str) -> bool {
        // A pending `n` can be previewed as `ん` before a following vowel
        // resolves the actual kana. Keep the one-shot boundary protection
        // until committed kana, rather than speculative romaji, consumes it.
        if self.romaji.pending().is_empty() {
            self.deferred_fragile_reading
                .take()
                .is_some_and(|reading| is_one_kana_extension(&reading, resolved))
        } else {
            self.deferred_fragile_reading
                .as_ref()
                .is_some_and(|reading| is_one_kana_extension(reading, resolved))
        }
    }

    fn set_protected_literal_full_live_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
        pending_prefix: Option<LiveStablePrefix>,
        surface: String,
    ) {
        let Some(prefix) = pending_prefix else {
            self.set_live_soft_checkpoint_preview(resolved, surface);
            return;
        };
        if surface.starts_with(&prefix.surface) {
            self.set_live_lattice_fallback(previous, resolved.to_owned(), surface, Some(prefix));
        } else if unresolved_pending_suffix_is_short(&prefix, resolved) {
            self.set_live_pending_literal_preview(resolved.to_owned(), prefix);
        } else {
            self.set_live_soft_checkpoint_preview(resolved, surface);
        }
    }

    fn apply_live_katakana_crossing(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
        prefix: &LiveStablePrefix,
    ) -> bool {
        let Some(surface) = self.live_katakana_crossing_surface(resolved, prefix) else {
            return false;
        };
        if prefix.reading.ends_with('や') {
            self.set_live_lattice_fallback(previous, resolved.to_owned(), surface, None);
        } else {
            self.set_live_soft_checkpoint_preview(resolved, surface);
        }
        true
    }

    fn set_deferred_fragile_full_live_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
        pending_prefix: Option<LiveStablePrefix>,
        boundary_fallback: Option<LiveStablePrefix>,
        surface: &str,
    ) -> bool {
        self.deferred_fragile_reading = Some(resolved.to_owned());
        self.set_unconfident_full_live_preview(
            previous,
            resolved,
            pending_prefix,
            boundary_fallback,
            Some(surface),
        )
    }

    fn apply_live_instability_guard_to_decision(&mut self, decision: &mut LiveConversionDecision) {
        let suppresses_repeated_prefix_flash = self.live_instability_guard.is_some()
            && matches!(
                decision,
                LiveConversionDecision::Confident(surface)
                    | LiveConversionDecision::StableExtension(surface)
                    if surface.prefix_fragile
            );
        if suppresses_repeated_prefix_flash {
            *decision = LiveConversionDecision::Literal;
        } else if matches!(
            decision,
            LiveConversionDecision::Confident(_) | LiveConversionDecision::StableExtension(_)
        ) {
            self.live_instability_guard = None;
        }
    }

    fn set_unconfident_full_live_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
        pending_prefix: Option<LiveStablePrefix>,
        boundary_fallback: Option<LiveStablePrefix>,
        ambiguous_surface: Option<&str>,
    ) -> bool {
        if let (Some(prefix), Some(surface)) = (pending_prefix.as_ref(), ambiguous_surface)
            && self.ambiguous_pending_surface_has_confident_whole_word_suffix(
                resolved, prefix, surface,
            )
        {
            self.set_live_word_checkpoint_preview(resolved, surface.to_owned(), pending_prefix);
            return true;
        }
        if let Some(prefix) = boundary_fallback {
            self.set_live_boundary_fallback_preview(prefix);
            return true;
        }
        if let Some(prefix) =
            previous.and_then(|preview| word_checkpoint_particle_boundary(preview, resolved))
        {
            self.set_live_word_checkpoint_particle_preview(previous, &prefix);
            return true;
        }
        if previous.is_some_and(|preview| preview.word_checkpoint.is_some())
            && self.apply_live_word_checkpoint_preview(previous, resolved)
        {
            return true;
        }
        if pending_prefix.is_none()
            && let Some(previous) = previous
            && let Some((surface, prefix, close_path)) = ambiguous_long_prefix_literal_extension(
                &self.dictionary,
                previous,
                resolved,
                ambiguous_surface,
            )
        {
            self.set_live_ambiguous_continuity_preview(resolved, surface, prefix, close_path);
            return true;
        }
        let Some(pending_prefix) = pending_prefix else {
            return false;
        };
        self.set_live_pending_literal_preview(resolved.to_owned(), pending_prefix);
        true
    }

    fn set_confident_full_live_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
        pending_prefix: Option<LiveStablePrefix>,
        surface: live_conversion::Surface,
        stable_extension: bool,
        follows_deferred_fragile: bool,
    ) {
        let confident = !stable_extension;
        let previous_reliable = previous.is_some_and(|preview| preview.reliable);
        let reliable = !stable_extension || previous_reliable;
        let deferred_literal_boundary = follows_deferred_fragile
            && surface.text != resolved
            && resolved
                .chars()
                .last()
                .is_some_and(|character| surface.text.ends_with(character));
        let sealable_bunsetsu = surface.ends_bunsetsu
            && reliable
            && !deferred_literal_boundary
            && (!stable_extension
                || previous.is_some_and(|preview| {
                    preview.surface.chars().count() >= MINIMUM_LIVE_STABLE_PREFIX_CHARACTERS
                }));
        let defers_stable_promotion = stable_extension
            && previous.is_some_and(|preview| {
                preview.sealable_bunsetsu && live_boundary_may_continue_word(preview, resolved)
            });
        let stable_prefix = (stable_extension
            && previous.is_some_and(|preview| preview.sealable_bunsetsu)
            && !defers_stable_promotion)
            .then(|| previous.and_then(eligible_live_stable_prefix))
            .flatten()
            .or_else(|| {
                (confident
                    && pending_prefix.as_ref().is_some_and(|prefix| {
                        !live_pending_boundary_can_start_katakana_word(
                            resolved,
                            &surface.text,
                            prefix,
                        ) && self.live_pending_prefix_is_confirmed(resolved, &surface.text, prefix)
                    }))
                .then(|| pending_prefix.clone())
                .flatten()
            });
        let fallback_prefix = if ends_live_fallback_boundary(resolved) {
            Some(LiveStablePrefix {
                reading: resolved.to_owned(),
                surface: surface.text.clone(),
            })
        } else {
            previous
                .and_then(|preview| preview.fallback_prefix.clone())
                .filter(|prefix| resolved.starts_with(&prefix.reading))
                .and_then(|prefix| {
                    if surface.text.starts_with(&prefix.surface) {
                        return Some(prefix);
                    }
                    live_conversion::surface_prefix_at_reading_boundary(
                        &self.dictionary,
                        resolved,
                        &surface.text,
                        &prefix.reading,
                    )
                    .map(|corrected_surface| LiveStablePrefix {
                        reading: prefix.reading,
                        surface: corrected_surface,
                    })
                })
        };
        let retained_pending_prefix = stable_prefix.is_none().then_some(pending_prefix).flatten();
        let retained_word_checkpoint = stable_extension
            .then(|| previous.and_then(|preview| preview.word_checkpoint.clone()))
            .flatten()
            .filter(|checkpoint| {
                resolved.starts_with(&checkpoint.reading)
                    && surface.text.starts_with(&checkpoint.surface)
            });
        let continuity_checkpoint = fallback_prefix.is_some()
            && previous.is_some_and(LivePreview::is_continuity_checkpoint);
        let prefix_fragile = surface.prefix_fragile;
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface: surface.text,
            reliable,
            prefix_fragile,
            sealable_bunsetsu,
            stable_prefix,
            pending_prefix: retained_pending_prefix,
            word_checkpoint: retained_word_checkpoint,
            fallback_prefix,
            checkpoint_kind: if continuity_checkpoint {
                LiveCheckpointKind::Continuity
            } else {
                LiveCheckpointKind::None
            },
        });
    }

    fn set_live_boundary_fallback_preview(&mut self, prefix: LiveStablePrefix) {
        self.live_preview = Some(LivePreview {
            reading: prefix.reading.clone(),
            surface: prefix.surface.clone(),
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: Some(prefix),
            checkpoint_kind: LiveCheckpointKind::None,
        });
    }

    fn set_live_soft_checkpoint_preview(&mut self, resolved: &str, surface: String) {
        let checkpoint = LiveStablePrefix {
            reading: resolved.to_owned(),
            surface: surface.clone(),
        };
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: Some(checkpoint),
            checkpoint_kind: LiveCheckpointKind::None,
        });
    }

    fn set_live_continuity_checkpoint_preview(&mut self, resolved: &str, surface: String) {
        let checkpoint = LiveStablePrefix {
            reading: resolved.to_owned(),
            surface: surface.clone(),
        };
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: Some(checkpoint),
            checkpoint_kind: LiveCheckpointKind::Continuity,
        });
    }

    fn set_live_ambiguous_continuity_preview(
        &mut self,
        resolved: &str,
        surface: String,
        prefix: LiveStablePrefix,
        close_path: bool,
    ) {
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: Some(prefix),
            checkpoint_kind: if close_path {
                LiveCheckpointKind::CloseContinuity
            } else {
                LiveCheckpointKind::Continuity
            },
        });
    }

    fn set_live_literal_extension_checkpoint_preview(&mut self, resolved: &str, surface: String) {
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: None,
            checkpoint_kind: LiveCheckpointKind::LiteralExtension,
        });
    }

    fn set_live_suffix_boundary_fallback_preview(
        &mut self,
        previous: &LivePreview,
        resolved: &str,
        fallback_prefix: LiveStablePrefix,
    ) {
        let surface = literal_extension_surface(previous, resolved)
            .filter(|surface| surface.starts_with(&fallback_prefix.surface))
            .filter(|surface| !contains_decimal_digit(surface) || contains_decimal_digit(resolved))
            .unwrap_or_else(|| {
                let suffix = resolved
                    .strip_prefix(&fallback_prefix.reading)
                    .unwrap_or_default();
                joined_live_surface(&fallback_prefix.surface, suffix)
            });
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: previous.stable_prefix.clone(),
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: Some(fallback_prefix),
            checkpoint_kind: if previous.is_continuity_checkpoint() {
                LiveCheckpointKind::Continuity
            } else {
                LiveCheckpointKind::None
            },
        });
    }

    fn live_suffix_fallback_prefix(
        &self,
        previous: &LivePreview,
        stable_prefix: &LiveStablePrefix,
        resolved: &str,
        surface: &str,
        reliable: bool,
    ) -> Option<LiveStablePrefix> {
        if ends_live_fallback_boundary(resolved)
            && (reliable
                || previous.pending_prefix.is_some()
                || resolved
                    .chars()
                    .next_back()
                    .is_some_and(is_live_opening_boundary))
        {
            return Some(LiveStablePrefix {
                reading: resolved.to_owned(),
                surface: surface.to_owned(),
            });
        }

        let fallback = previous
            .fallback_prefix
            .as_ref()
            .filter(|fallback| resolved.starts_with(&fallback.reading))?;
        if surface.starts_with(&fallback.surface) {
            return Some(fallback.clone());
        }

        let target_reading = resolved.strip_prefix(&stable_prefix.reading)?;
        let target_surface = surface.strip_prefix(&stable_prefix.surface)?;
        let boundary_target_reading = fallback.reading.strip_prefix(&stable_prefix.reading)?;
        let corrected_target_surface = live_conversion::surface_prefix_at_reading_boundary(
            &self.dictionary,
            target_reading,
            target_surface,
            boundary_target_reading,
        )?;
        let mut corrected_surface =
            String::with_capacity(stable_prefix.surface.len() + corrected_target_surface.len());
        corrected_surface.push_str(&stable_prefix.surface);
        corrected_surface.push_str(&corrected_target_surface);
        Some(LiveStablePrefix {
            reading: fallback.reading.clone(),
            surface: corrected_surface,
        })
    }

    fn set_live_pending_literal_preview(
        &mut self,
        resolved: String,
        pending_prefix: LiveStablePrefix,
    ) {
        let target_reading = resolved
            .strip_prefix(&pending_prefix.reading)
            .unwrap_or_default();
        let mut protected_surface =
            String::with_capacity(pending_prefix.surface.len() + target_reading.len());
        protected_surface.push_str(&pending_prefix.surface);
        protected_surface.push_str(target_reading);
        self.live_preview = Some(LivePreview {
            reading: resolved,
            surface: protected_surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: Some(pending_prefix),
            word_checkpoint: None,
            fallback_prefix: None,
            checkpoint_kind: LiveCheckpointKind::None,
        });
    }

    fn set_live_word_checkpoint_preview(
        &mut self,
        resolved: &str,
        surface: String,
        pending_prefix: Option<LiveStablePrefix>,
    ) {
        let word_checkpoint = LiveStablePrefix {
            reading: resolved.to_owned(),
            surface: surface.clone(),
        };
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix,
            word_checkpoint: Some(word_checkpoint),
            fallback_prefix: None,
            checkpoint_kind: LiveCheckpointKind::None,
        });
    }

    fn set_live_word_checkpoint_particle_preview(
        &mut self,
        previous: Option<&LivePreview>,
        prefix: &LiveStablePrefix,
    ) {
        self.live_preview = Some(LivePreview {
            reading: prefix.reading.clone(),
            surface: prefix.surface.clone(),
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: previous.and_then(|preview| preview.pending_prefix.clone()),
            word_checkpoint: None,
            fallback_prefix: None,
            checkpoint_kind: LiveCheckpointKind::None,
        });
    }

    fn apply_live_word_checkpoint_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
    ) -> bool {
        let Some(previous) = previous else {
            return false;
        };
        let Some(surface) = word_checkpoint_extension_surface(previous, resolved) else {
            return false;
        };
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: previous.pending_prefix.clone(),
            word_checkpoint: previous.word_checkpoint.clone(),
            fallback_prefix: previous.fallback_prefix.clone(),
            checkpoint_kind: if previous.is_continuity_checkpoint() {
                LiveCheckpointKind::Continuity
            } else {
                LiveCheckpointKind::None
            },
        });
        true
    }

    fn set_live_lattice_fallback(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: String,
        surface: String,
        pending_prefix: Option<LiveStablePrefix>,
    ) {
        let fallback_prefix = previous
            .and_then(|preview| preview.fallback_prefix.clone())
            .filter(|prefix| {
                resolved.starts_with(&prefix.reading) && surface.starts_with(&prefix.surface)
            });
        let word_checkpoint = previous
            .and_then(|preview| preview.word_checkpoint.clone())
            .filter(|checkpoint| {
                resolved.starts_with(&checkpoint.reading)
                    && surface.starts_with(&checkpoint.surface)
            });
        let continuity_checkpoint = fallback_prefix.is_some()
            && previous.is_some_and(LivePreview::is_continuity_checkpoint);
        self.live_preview = Some(LivePreview {
            reading: resolved,
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix,
            word_checkpoint,
            fallback_prefix,
            checkpoint_kind: if continuity_checkpoint {
                LiveCheckpointKind::Continuity
            } else {
                LiveCheckpointKind::None
            },
        });
    }

    fn apply_live_fallback_preview(
        &mut self,
        previous: Option<&LivePreview>,
        resolved: &str,
    ) -> bool {
        let Some(prefix) = previous.and_then(|preview| preview.fallback_prefix.as_ref()) else {
            return false;
        };
        let Some(suffix) = resolved.strip_prefix(&prefix.reading) else {
            return false;
        };
        let surface = previous
            .and_then(|preview| literal_extension_surface(preview, resolved))
            .filter(|surface| surface.starts_with(&prefix.surface))
            .filter(|surface| !contains_decimal_digit(surface) || contains_decimal_digit(resolved))
            .unwrap_or_else(|| joined_live_surface(&prefix.surface, suffix));
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: previous.and_then(|preview| preview.pending_prefix.clone()),
            word_checkpoint: None,
            fallback_prefix: Some(prefix.clone()),
            checkpoint_kind: if previous.is_some_and(LivePreview::is_continuity_checkpoint) {
                LiveCheckpointKind::Continuity
            } else {
                LiveCheckpointKind::None
            },
        });
        true
    }

    fn refresh_live_suffix_preview(
        &mut self,
        previous: &LivePreview,
        prefix: &LiveStablePrefix,
        resolved: &str,
        target_reading: &str,
    ) {
        if self.apply_live_crossing_checkpoint(resolved, prefix) {
            return;
        }

        let (pending_prefix, previous_target_surface, stable_surface) =
            live_suffix_context(previous, prefix, target_reading);

        let decision =
            self.live_suffix_decision(resolved, prefix, target_reading, stable_surface.as_deref());
        let LiveTargetDecision {
            surface: target_surface,
            ends_bunsetsu,
            kind,
        } = live_suffix_continuity_decision(previous, prefix, resolved, &decision)
            .unwrap_or_else(|| live_target_decision(decision, target_reading));
        let stable_extension = kind == LiveTargetDecisionKind::StableExtension;
        let confident = kind == LiveTargetDecisionKind::Confident;
        let protected_literal = kind == LiveTargetDecisionKind::ProtectedLiteral;

        let surface = joined_live_surface(&prefix.surface, &target_surface);
        if self.apply_live_two_kana_inflected_checkpoint(
            previous,
            prefix,
            resolved,
            target_reading,
            &target_surface,
            stable_extension,
        ) {
            return;
        }
        let protected_pending_prefix = protected_literal
            .then_some(pending_prefix.as_ref())
            .flatten()
            .filter(|pending| surface.starts_with(&pending.surface))
            .cloned();
        let keeps_short_conflicting_pending = protected_literal
            && pending_prefix
                .as_ref()
                .is_some_and(|pending| unresolved_pending_suffix_is_short(pending, resolved));

        if let Some(checkpoint) = pending_boundary_checkpoint(previous, resolved) {
            self.set_live_suffix_boundary_fallback_preview(previous, resolved, checkpoint);
            return;
        }

        if let Some(fallback) = conflicting_live_fallback(previous, resolved, &surface, confident) {
            self.set_live_suffix_boundary_fallback_preview(previous, resolved, fallback.clone());
            return;
        }

        if !confident
            && (!protected_literal || keeps_short_conflicting_pending)
            && let Some(pending_prefix) = pending_prefix.as_ref()
            && !surface.starts_with(&pending_prefix.surface)
        {
            self.set_live_pending_literal_preview(resolved.to_owned(), pending_prefix.clone());
            return;
        }

        let reliable = confident || (stable_extension && previous.reliable);
        let sealable_bunsetsu = ends_bunsetsu
            && reliable
            && (!stable_extension
                || previous_target_surface.chars().count()
                    >= MINIMUM_LIVE_STABLE_PREFIX_CHARACTERS);
        let (stable_prefix, retained_pending_prefix) =
            self.live_suffix_prefixes(LiveSuffixPrefixContext {
                previous,
                stable: prefix,
                resolved,
                surface: &surface,
                pending: pending_prefix,
                protected_pending: protected_pending_prefix,
                decision: LiveSuffixBoundaryDecision {
                    confident,
                    stable_extension,
                    ends_bunsetsu,
                },
            });
        let fallback_prefix = if kind == LiveTargetDecisionKind::ProtectedLiteral {
            Some(LiveStablePrefix {
                reading: resolved.to_owned(),
                surface: surface.clone(),
            })
        } else {
            self.live_suffix_fallback_prefix(previous, prefix, resolved, &surface, reliable)
        };
        self.live_preview = Some(LivePreview {
            reading: resolved.to_owned(),
            surface,
            reliable,
            prefix_fragile: false,
            sealable_bunsetsu,
            stable_prefix,
            pending_prefix: retained_pending_prefix,
            word_checkpoint: None,
            fallback_prefix,
            checkpoint_kind: if kind == LiveTargetDecisionKind::SuffixContinuity {
                LiveCheckpointKind::SuffixContinuity
            } else {
                LiveCheckpointKind::None
            },
        });
    }

    fn live_suffix_prefixes(
        &self,
        context: LiveSuffixPrefixContext<'_>,
    ) -> (Option<LiveStablePrefix>, Option<LiveStablePrefix>) {
        let LiveSuffixPrefixContext {
            previous,
            stable,
            resolved,
            surface,
            pending: pending_prefix,
            protected_pending: protected_pending_prefix,
            decision,
        } = context;
        let defers_stable_promotion = decision.stable_extension
            && previous.sealable_bunsetsu
            && live_boundary_may_continue_word(previous, resolved);
        let defers_pending_promotion = (decision.confident || decision.stable_extension)
            && pending_prefix.as_ref().is_some_and(|pending| {
                live_pending_boundary_can_start_katakana_word(resolved, surface, pending)
            });
        let advanced_stable_prefix =
            (decision.stable_extension && previous.sealable_bunsetsu && !defers_stable_promotion)
                .then(|| eligible_live_stable_prefix(previous))
                .flatten()
                .or_else(|| {
                    (decision.confident
                        && pending_prefix.as_ref().is_some_and(|pending| {
                            live_prefix_ends_particle(pending)
                                && !live_pending_boundary_can_start_katakana_word(
                                    resolved, surface, pending,
                                )
                                && self.live_pending_prefix_is_confirmed(resolved, surface, pending)
                        }))
                    .then(|| pending_prefix.clone())
                    .flatten()
                });
        let stable_prefix = advanced_stable_prefix
            .clone()
            .or_else(|| Some(stable.clone()));
        let retained_pending_prefix = if defers_stable_promotion {
            Some(live_stable_prefix(previous))
        } else if protected_pending_prefix.is_some() {
            protected_pending_prefix
        } else if defers_pending_promotion
            || pending_prefix.as_ref().is_some_and(|pending| {
                pending.reading.ends_with('や')
                    && pending.surface.ends_with('や')
                    && surface.starts_with(&pending.surface)
                    && unresolved_pending_suffix_is_short(pending, resolved)
            })
        {
            pending_prefix
        } else if previous.pending_prefix.is_some() && decision.confident && !decision.ends_bunsetsu
        {
            Some(LiveStablePrefix {
                reading: resolved.to_owned(),
                surface: surface.to_owned(),
            })
        } else {
            None
        };
        (stable_prefix, retained_pending_prefix)
    }

    fn apply_live_crossing_checkpoint(
        &mut self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
    ) -> bool {
        // A stable particle-looking edge can still be inside a long dictionary
        // phrase, lexicalized hiragana expression, or katakana word. Reopen it
        // only when the corresponding bounded full conversion crosses the
        // exact reading boundary.
        let surface = self
            .live_phrase_crossing_surface(resolved_reading, prefix)
            .or_else(|| self.live_lexicalized_hiragana_crossing_surface(resolved_reading, prefix))
            .or_else(|| self.live_katakana_crossing_surface(resolved_reading, prefix));
        let Some(surface) = surface else {
            return false;
        };
        self.set_live_soft_checkpoint_preview(resolved_reading, surface);
        true
    }

    fn apply_live_two_kana_inflected_checkpoint(
        &mut self,
        previous: &LivePreview,
        prefix: &LiveStablePrefix,
        resolved_reading: &str,
        target_reading: &str,
        target_surface: &str,
        stable_extension: bool,
    ) -> bool {
        if !stable_extension || !previous.sealable_bunsetsu {
            return false;
        }
        let Some(surface) = self.live_two_kana_inflected_full_surface(
            resolved_reading,
            prefix,
            target_reading,
            target_surface,
        ) else {
            return false;
        };
        // Do not promote the first literal kana of an ambiguous `とる` into
        // the stable prefix. Keep the context-aware kanji-plus-okurigana form
        // as a soft checkpoint while the delayed ranker sees the full phrase.
        self.set_live_soft_checkpoint_preview(resolved_reading, surface);
        true
    }

    fn live_suffix_decision(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
        target_reading: &str,
        stable_target_surface: Option<&str>,
    ) -> LiveConversionDecision {
        let local_decision =
            if target_reading.chars().count() < live_conversion::MINIMUM_READING_CHARACTERS {
                LiveConversionDecision::Literal
            } else {
                self.live_conversion_decision(
                    target_reading,
                    stable_target_surface,
                    Some(&prefix.surface),
                )
            };
        self.live_full_lattice_suffix_repair(
            resolved_reading,
            prefix,
            target_reading,
            stable_target_surface,
            &local_decision,
        )
        .unwrap_or(local_decision)
    }

    fn live_conversion_decision(
        &self,
        reading: &str,
        stable_surface: Option<&str>,
        marked_prefix_surface: Option<&str>,
    ) -> LiveConversionDecision {
        self.live_conversion_decision_with_pending_prefix_protection(
            reading,
            None,
            false,
            stable_surface,
            marked_prefix_surface,
            None,
        )
    }

    fn live_conversion_decision_with_pending_prefix_protection(
        &self,
        reading: &str,
        previous_display_surface: Option<&str>,
        previous_is_continuity_checkpoint: bool,
        stable_surface: Option<&str>,
        marked_prefix_surface: Option<&str>,
        protected_pending_prefix_surface: Option<&str>,
    ) -> LiveConversionDecision {
        let neural_surface = self.live_neural_extension_surface(reading);
        let mut left_context = self
            .session_history
            .previous_surface()
            .unwrap_or_default()
            .to_owned();
        if let Some(prefix) = marked_prefix_surface {
            left_context.push_str(prefix);
        }
        let contextual_surface =
            self.live_contextual_surface(reading, &left_context, marked_prefix_surface.is_some());
        let mut decision = live_conversion::decide(
            &self.dictionary,
            &self.user_data,
            reading,
            self.history_is_available(),
            self.delayed_live_ranking == DelayedLiveRankingAvailability::Available,
            live_conversion::PreferredSurfaces {
                recent: self.recent_live_selection_surface(reading),
                contextual: contextual_surface
                    .as_ref()
                    .map(|surface| surface.text.as_str()),
                neural: neural_surface.as_deref(),
            },
            live_conversion::DisplaySurfaces {
                previous: previous_display_surface,
                previous_is_continuity_checkpoint,
                stable: stable_surface,
                marked_prefix: marked_prefix_surface,
                protected_pending_prefix: protected_pending_prefix_surface,
            },
        );
        if let Some(contextual) = contextual_surface
            && contextual.particle_extension
            && let LiveConversionDecision::Confident(surface)
            | LiveConversionDecision::StableExtension(surface) = &mut decision
            && surface.text == contextual.text
        {
            surface.ends_bunsetsu = false;
            surface.prefix_fragile = true;
        }
        decision
    }

    fn is_fragile_learned_live_preview(&self, preview: &LivePreview) -> bool {
        preview.prefix_fragile
            && (self
                .contextual_history_surfaces_for_reading(&preview.reading, None)
                .contains(&preview.surface.as_str())
                || self
                    .contextual_particle_history_candidates(&preview.reading, None)
                    .contains(&preview.surface))
    }

    fn live_contextual_surface(
        &self,
        reading: &str,
        left_context: &str,
        has_marked_prefix: bool,
    ) -> Option<LiveContextualSurface> {
        let previous = has_marked_prefix.then_some(left_context);
        self.contextual_history_surfaces_for_reading(reading, previous)
            .into_iter()
            .next()
            .map(|surface| LiveContextualSurface {
                text: surface.to_owned(),
                particle_extension: false,
            })
            .or_else(|| {
                self.contextual_particle_history_candidates(reading, previous)
                    .into_iter()
                    .next()
                    .map(|text| LiveContextualSurface {
                        text,
                        particle_extension: true,
                    })
            })
            .or_else(|| {
                self.installed_contextual_surface(left_context, reading)
                    .map(|text| LiveContextualSurface {
                        text,
                        particle_extension: false,
                    })
            })
    }

    fn live_pending_prefix_is_confirmed(
        &self,
        resolved_reading: &str,
        full_surface: &str,
        prefix: &LiveStablePrefix,
    ) -> bool {
        let Some(target_reading) = resolved_reading.strip_prefix(&prefix.reading) else {
            return false;
        };
        if target_reading.chars().count() < MINIMUM_LIVE_STABLE_TARGET_CHARACTERS {
            return false;
        }
        let Some(target_surface) = full_surface.strip_prefix(&prefix.surface) else {
            return false;
        };
        matches!(
            self.live_conversion_decision(target_reading, None, Some(&prefix.surface)),
            LiveConversionDecision::Confident(surface) if surface.text == target_surface
        )
    }

    fn ambiguous_pending_surface_has_confident_whole_word_suffix(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
        full_surface: &str,
    ) -> bool {
        let Some(target_reading) = resolved_reading.strip_prefix(&prefix.reading) else {
            return false;
        };
        if target_reading.chars().count() < MINIMUM_LIVE_STABLE_TARGET_CHARACTERS {
            return false;
        }
        let Some(target_surface) = full_surface.strip_prefix(&prefix.surface) else {
            return false;
        };
        let conversions = self.dictionary.convert_n_best(target_reading, 2);
        let Some(best) = conversions.first() else {
            return false;
        };
        let runner_up = conversions
            .iter()
            .find(|conversion| conversion.surface != best.surface);
        best.surface == target_surface
            && best.segments.len() == 1
            && best.segments[0].reading == target_reading
            && best.segments[0].cost <= MAXIMUM_WORD_CHECKPOINT_ENTRY_COST
            && runner_up.is_none_or(|runner_up| {
                runner_up.cost.saturating_sub(best.cost) >= MINIMUM_WORD_CHECKPOINT_COST_MARGIN
            })
    }

    fn live_katakana_crossing_surface(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
    ) -> Option<String> {
        let boundary_character = prefix.reading.chars().next_back()?;
        let target_suffix = resolved_reading.strip_prefix(&prefix.reading)?;
        let has_mixed_tail =
            has_mixed_converted_tail_before_boundary(&prefix.surface, boundary_character);
        if !prefix.surface.ends_with(boundary_character)
            || !matches!(
                boundary_character,
                'は' | 'が' | 'や' | 'を' | 'に' | 'へ' | 'と' | 'で' | 'も' | 'の'
            )
            || target_suffix.chars().count() < 2
        {
            return None;
        }
        // A prolonged sound mark cannot begin an independent bunsetsu. A
        // leading ん is similarly decisive after a false が boundary, while
        // keeping the deliberately conservative はんど case protected.
        let impossible_independent_suffix = target_suffix.starts_with('ー')
            || (boundary_character == 'が' && target_suffix.starts_with('ん'));
        let has_long_katakana_tail =
            converted_katakana_tail_before_boundary(&prefix.surface, boundary_character) >= 3;
        let uses_broadened_probe = impossible_independent_suffix || has_long_katakana_tail;
        if boundary_character != 'や' && !has_mixed_tail && !uses_broadened_probe {
            return None;
        }
        if boundary_character != 'や'
            && !has_mixed_tail
            && target_suffix.chars().count() > MAX_LIVE_KATAKANA_REOPEN_SUFFIX_CHARACTERS
        {
            return None;
        }
        let conversion_limit = if boundary_character == 'や' || has_mixed_tail {
            4
        } else {
            1
        };
        let boundary = prefix.reading.len();
        self.dictionary
            .convert_n_best(resolved_reading, conversion_limit)
            .into_iter()
            .enumerate()
            .find(|(rank, conversion)| {
                if conversion.surface.starts_with(&prefix.surface) {
                    return false;
                }
                let Some(boundary_prefix_characters) =
                    crossing_katakana_run_prefix_characters(conversion, boundary)
                else {
                    return false;
                };
                boundary_character == 'や'
                    || (has_mixed_tail && boundary_prefix_characters >= 2)
                    || (*rank == 0
                        && (boundary_prefix_characters >= 3
                            || (impossible_independent_suffix && boundary_prefix_characters >= 1)))
            })
            .map(|(_, conversion)| conversion.surface)
    }

    fn live_phrase_crossing_surface(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
    ) -> Option<String> {
        let boundary_character = prefix.reading.chars().next_back()?;
        let target_suffix = resolved_reading.strip_prefix(&prefix.reading)?;
        if !prefix.surface.ends_with(boundary_character)
            || !matches!(
                boundary_character,
                'は' | 'が' | 'を' | 'に' | 'へ' | 'と' | 'で' | 'も' | 'の'
            )
            || !(2..=MAX_LIVE_PHRASE_REOPEN_SUFFIX_CHARACTERS)
                .contains(&target_suffix.chars().count())
        {
            return None;
        }
        let boundary = prefix.reading.len();
        let conversion = self.dictionary.convert_n_best(resolved_reading, 1).pop()?;
        (!conversion.surface.starts_with(&prefix.surface)
            && (conversion_has_long_phrase_segment_crossing_boundary(&conversion, boundary)
                || conversion_has_kanji_word_crossing_particle_boundary(&conversion, boundary)))
        .then_some(conversion.surface)
    }

    fn live_two_kana_inflected_full_surface(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
        target_reading: &str,
        target_surface: &str,
    ) -> Option<String> {
        if target_reading != "とる" || target_reading != target_surface {
            return None;
        }
        let mut reading = target_reading.chars();
        let _ = reading.next()?;
        let okurigana = reading.next()?;
        let conversion = self.dictionary.convert_n_best(resolved_reading, 1).pop()?;
        let boundary_surface = live_conversion::surface_prefix_at_reading_boundary(
            &self.dictionary,
            resolved_reading,
            &conversion.surface,
            &prefix.reading,
        )?;
        if boundary_surface != prefix.surface {
            return None;
        }
        let mut target = conversion.surface.strip_prefix(&prefix.surface)?.chars();
        (target.next().is_some_and(is_kanji_character)
            && target.next() == Some(okurigana)
            && target.next().is_none())
        .then_some(conversion.surface)
    }

    fn live_lexicalized_hiragana_crossing_surface(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
    ) -> Option<String> {
        let boundary_character = prefix.reading.chars().next_back()?;
        if !prefix.surface.ends_with(boundary_character) {
            return None;
        }
        let target_suffix = resolved_reading.strip_prefix(&prefix.reading)?;
        let lexical_reading = match boundary_character {
            'で' if target_suffix.starts_with("きる") => "できる",
            'と' if target_suffix.starts_with("いう") => "という",
            'も' if target_suffix.starts_with('の') => "もの",
            _ => return None,
        };
        let boundary = prefix.reading.len();
        let conversion = self.dictionary.convert_n_best(resolved_reading, 1).pop()?;
        conversion_has_literal_segment_crossing_boundary(&conversion, boundary, lexical_reading)
            .then_some(conversion.surface)
    }

    fn live_full_lattice_suffix_repair(
        &self,
        resolved_reading: &str,
        prefix: &LiveStablePrefix,
        target_reading: &str,
        stable_target_surface: Option<&str>,
        local_decision: &LiveConversionDecision,
    ) -> Option<LiveConversionDecision> {
        let local_surface = live_decision_surface(local_decision, target_reading);
        if !surface_has_fragmented_literal_leading_edge(target_reading, local_surface) {
            return None;
        }
        let local_conversion = self
            .dictionary
            .convert_n_best(target_reading, LIVE_FRAGMENTED_SUFFIX_PATH_LIMIT)
            .into_iter()
            .find(|conversion| conversion.surface == local_surface)?;
        if !conversion_has_fragmented_literal_leading_edge(&local_conversion) {
            return None;
        }

        // A stable surface boundary is only a display boundary; resetting the
        // lattice there also resets the connection cost. That can make an
        // ordinary inflected word look like a literal fragment followed by
        // tiny ideographs (`あらしたなに` -> `あら下無に`). Re-evaluate the
        // complete reading only for that characteristic broken shape, then
        // accept the result only when an actual full-reading path preserves
        // the stable boundary exactly.
        let stable_full_surface = stable_target_surface.map(|target| {
            let mut surface = String::with_capacity(prefix.surface.len() + target.len());
            surface.push_str(&prefix.surface);
            surface.push_str(target);
            surface
        });
        let full_decision =
            self.live_conversion_decision(resolved_reading, stable_full_surface.as_deref(), None);
        let full_surface = live_decision_candidate_surface(&full_decision)?;
        let boundary_surface = live_conversion::surface_prefix_at_reading_boundary(
            &self.dictionary,
            resolved_reading,
            full_surface,
            &prefix.reading,
        )?;
        if boundary_surface != prefix.surface {
            return None;
        }
        let repaired_target = full_surface.strip_prefix(&prefix.surface)?.to_owned();
        if repaired_target == local_surface {
            return None;
        }

        Some(match full_decision {
            LiveConversionDecision::Confident(mut surface) => {
                surface.text = repaired_target;
                LiveConversionDecision::Confident(surface)
            }
            LiveConversionDecision::StableExtension(mut surface) => {
                surface.text = repaired_target;
                LiveConversionDecision::StableExtension(surface)
            }
            LiveConversionDecision::LatticeFallback(mut surface) => {
                surface.text = repaired_target;
                LiveConversionDecision::LatticeFallback(surface)
            }
            LiveConversionDecision::ProtectedLiteral(_) => {
                LiveConversionDecision::ProtectedLiteral(repaired_target)
            }
            LiveConversionDecision::Continuity(_) => {
                LiveConversionDecision::Continuity(repaired_target)
            }
            LiveConversionDecision::DeferredFragile(_) => {
                LiveConversionDecision::LatticeFallback(live_conversion::Surface {
                    text: repaired_target,
                    ends_bunsetsu: false,
                    prefix_fragile: true,
                })
            }
            LiveConversionDecision::Ambiguous(_) => {
                LiveConversionDecision::LatticeFallback(live_conversion::Surface {
                    text: repaired_target,
                    ends_bunsetsu: false,
                    prefix_fragile: false,
                })
            }
            LiveConversionDecision::Literal => return None,
        })
    }

    fn installed_contextual_surface(
        &self,
        previous_surface: &str,
        reading: &str,
    ) -> Option<String> {
        if self.preferences.private_mode {
            return None;
        }
        let mut selected = None;
        self.installed_packs
            .visit_contextual_surfaces(previous_surface, reading, |surface| {
                selected = Some(surface.to_owned());
                false
            });
        selected
    }

    /// Returns only complete dictionary candidates whose whole reading or
    /// first lattice segment matches an installed left-context rule. The
    /// latter lets a token-level public rule rank the beginning of a longer
    /// phrase without locking a segment or synthesizing the unobserved suffix.
    fn installed_contextual_conversion_surfaces(
        &self,
        previous_surface: &str,
        reading: &str,
        allowed_surfaces: &[String],
    ) -> Vec<String> {
        if self.preferences.private_mode || !self.installed_packs.has_contextual_rules() {
            return Vec::new();
        }

        let mut promoted = Vec::new();
        if !previous_surface.is_empty() {
            self.installed_packs
                .visit_contextual_surfaces(previous_surface, reading, |surface| {
                    if allowed_surfaces
                        .iter()
                        .any(|candidate| candidate == surface)
                    {
                        push_unique(&mut promoted, surface.to_owned());
                    }
                    promoted.len() < CONTEXT_RULE_PROMOTION_LIMIT
                });
        }
        // Every whole-reading rule was visited above, so a single-segment
        // path cannot gain support below. Skip the second N-best search
        // unless a rule exists for a shorter reading inside this one.
        if promoted.len() == CONTEXT_RULE_PROMOTION_LIMIT
            || !self.installed_packs.has_contextual_rules_inside(reading)
        {
            return promoted;
        }

        let conversions = self
            .dictionary
            .convert_n_best(reading, EXPLICIT_RANKING_CANDIDATE_LIMIT);
        let Some(baseline) = allowed_surfaces
            .first()
            .and_then(|surface| {
                conversions
                    .iter()
                    .find(|conversion| conversion.surface == *surface)
            })
            .or_else(|| conversions.first())
        else {
            return promoted;
        };
        let mut supported = Vec::new();
        for conversion in &conversions {
            if promoted.len() == CONTEXT_RULE_PROMOTION_LIMIT {
                break;
            }
            if !allowed_surfaces
                .iter()
                .any(|candidate| candidate == &conversion.surface)
                || promoted
                    .iter()
                    .any(|candidate| candidate == &conversion.surface)
            {
                continue;
            }
            let mut support = 0_u16;
            let mut reading_start = 0;
            for (index, segment) in conversion.segments.iter().enumerate() {
                let reading_end = reading_start + segment.reading.len();
                let left_context = if index == 0 {
                    previous_surface
                } else {
                    &conversion.segments[index - 1].surface
                };
                if !left_context.is_empty()
                    && self.installed_context_rule_matches(
                        left_context,
                        &segment.reading,
                        &segment.surface,
                    )
                    && aligned_segment_surface(baseline, reading_start, reading_end)
                        .is_some_and(|surface| surface != segment.surface)
                {
                    support = support.saturating_add(1);
                }
                reading_start = reading_end;
            }
            if support > 0 {
                supported.push((support, conversion.surface.clone()));
            }
        }
        supported.sort_by_key(|(support, _)| std::cmp::Reverse(*support));
        for (_, surface) in supported {
            if promoted.len() == CONTEXT_RULE_PROMOTION_LIMIT {
                break;
            }
            push_unique(&mut promoted, surface);
        }
        promoted
    }

    fn installed_context_rule_matches(
        &self,
        previous_surface: &str,
        reading: &str,
        surface: &str,
    ) -> bool {
        let mut matched = false;
        self.installed_packs
            .visit_contextual_surfaces(previous_surface, reading, |candidate| {
                if candidate == surface {
                    matched = true;
                    false
                } else {
                    true
                }
            });
        matched
    }

    fn refresh_completion_actions(&mut self, include_preedit: bool) -> Vec<SlimeAction> {
        let had_completions = self.candidate_kind == Some(CandidateKind::Completion);
        if self.phase() == Phase::Converting {
            return Vec::new();
        }

        let mut suggestions = Vec::with_capacity(9);
        let mut reverse_target = self.reading.clone();
        reverse_target.push_str(self.romaji.pending());
        for surface in self.english_reverse_surfaces(&reverse_target) {
            push_unique(&mut suggestions, surface);
        }
        if self.history_is_available() && self.reading.chars().count() >= 2 {
            if let Some((previous_reading, previous_surface)) =
                self.session_history.previous_commit()
            {
                for surface in self.user_data.contextual_completion_surfaces(
                    previous_reading,
                    previous_surface,
                    &self.reading,
                    9,
                ) {
                    push_unique(&mut suggestions, surface.to_owned());
                }
            }
            for surface in self.user_data.completion_surfaces(&self.reading, 9) {
                push_unique(&mut suggestions, surface);
                if suggestions.len() == 9 {
                    break;
                }
            }
        }

        let mut actions = Vec::with_capacity(2);
        if suggestions.is_empty() {
            if had_completions {
                self.clear_candidates();
                actions.push(SlimeAction::HideCandidates);
            }
        } else {
            self.candidates = suggestions;
            self.selected = 0;
            self.candidate_kind = Some(CandidateKind::Completion);
            self.completion_selected = false;
            actions.push(SlimeAction::ShowCandidates {
                candidates: self.candidates.clone(),
                details: self.candidate_details(),
                selected: self.selected,
            });
        }
        if include_preedit && (!self.reading.is_empty() || !self.romaji.pending().is_empty()) {
            actions.insert(0, SlimeAction::UpdatePreedit(self.preedit()));
        }
        actions
    }

    fn record_history(&mut self, reading: &str, surface: &str) {
        if self.preferences.private_mode {
            self.session_history.reset_context();
            return;
        }
        if !user_data::is_useful_context_anchor(reading, surface) {
            self.session_history.reset_context();
            return;
        }

        let previous = self
            .session_history
            .previous_commit()
            .map(|(reading, surface)| (reading.to_owned(), surface.to_owned()));
        if self.preferences.history_learning {
            if let Some((previous_reading, previous_surface)) = previous {
                self.user_data.record_context(
                    &previous_reading,
                    &previous_surface,
                    reading,
                    surface,
                );
            }
            if should_record_history(reading, surface) {
                self.user_data.record(reading, surface);
            }
        }
        self.session_history.record_commit(reading, surface);
    }

    fn recent_live_selection_surface(&self, reading: &str) -> Option<&str> {
        self.recent_live_selections
            .iter()
            .rev()
            .find(|selection| selection.reading == reading)
            .map(|selection| selection.surface.as_str())
    }

    fn live_neural_extension_surface(&self, reading: &str) -> Option<String> {
        let selection = self.live_neural_selection.as_ref()?;
        let suffix = reading.strip_prefix(&selection.reading)?;
        let mut surface = String::with_capacity(selection.surface.len() + suffix.len());
        surface.push_str(&selection.surface);
        surface.push_str(suffix);
        Some(surface)
    }

    fn record_recent_live_selection(&mut self, reading: &str, surface: &str) {
        if !self.preferences.history_learning
            || self.preferences.private_mode
            || reading.chars().count() < live_conversion::MINIMUM_PERSONALIZED_READING_CHARACTERS
            || !user_data::is_useful_history(reading, surface)
        {
            return;
        }

        self.recent_live_selections
            .retain(|selection| selection.reading != reading);
        if self.recent_live_selections.len() == MAX_RECENT_LIVE_SELECTIONS {
            self.recent_live_selections.remove(0);
        }
        self.recent_live_selections.push(RecentLiveSelection {
            reading: reading.to_owned(),
            surface: surface.to_owned(),
        });
    }

    fn record_conversion_history(&mut self, reading: &str, surface: &str) {
        if self.preferences.history_learning && !self.preferences.private_mode {
            let selected_segments: Vec<_> = self
                .segments
                .iter()
                .enumerate()
                .filter(|(_, segment)| segment.explicitly_selected)
                .map(|(index, segment)| {
                    (
                        segment.reading.clone(),
                        segment.surface.clone(),
                        self.segment_context_before(index),
                    )
                })
                .collect();
            let mut recorded = Vec::with_capacity(selected_segments.len());
            let mut recorded_contexts = Vec::with_capacity(selected_segments.len());
            for (segment_reading, segment_surface, context) in selected_segments {
                if let Some((previous_reading, previous_surface)) = context
                    && !recorded_contexts.iter().any(
                        |(
                            recorded_previous_reading,
                            recorded_previous_surface,
                            recorded_reading,
                            recorded_surface,
                        )| {
                            recorded_previous_reading == &previous_reading
                                && recorded_previous_surface == &previous_surface
                                && recorded_reading == &segment_reading
                                && recorded_surface == &segment_surface
                        },
                    )
                {
                    self.user_data.record_context(
                        &previous_reading,
                        &previous_surface,
                        &segment_reading,
                        &segment_surface,
                    );
                    recorded_contexts.push((
                        previous_reading,
                        previous_surface,
                        segment_reading.clone(),
                        segment_surface.clone(),
                    ));
                }
                if (segment_reading == reading && segment_surface == surface)
                    || recorded.iter().any(|(recorded_reading, recorded_surface)| {
                        recorded_reading == &segment_reading && recorded_surface == &segment_surface
                    })
                    || !should_record_history(&segment_reading, &segment_surface)
                {
                    continue;
                }
                self.user_data.record(&segment_reading, &segment_surface);
                recorded.push((segment_reading, segment_surface));
            }
        }
        self.record_history(reading, surface);
    }

    fn segment_context_before(&self, index: usize) -> Option<(String, String)> {
        // Prefer the shortest useful suffix. A particle-only segment such as
        // `の` is not an anchor by itself, so include the preceding converted
        // segment and retain the discriminating surface `日本の`.
        for start in (0..index).rev() {
            let reading: String = self.segments[start..index]
                .iter()
                .map(|segment| segment.reading.as_str())
                .collect();
            let surface: String = self.segments[start..index]
                .iter()
                .map(|segment| segment.surface.as_str())
                .collect();
            if user_data::is_useful_context_anchor(&reading, &surface) {
                return Some((reading, surface));
            }
        }
        None
    }

    fn record_completion_history(&mut self, prefix: &str, surface: &str) {
        if !self.preferences.history_learning || self.preferences.private_mode {
            self.session_history.reset_context();
            return;
        }
        let Some(reading) = self.user_data.promote_completion(prefix, surface) else {
            // English reverse matches have no history entry yet; learn the
            // mangled reading so plain history completion covers it next time.
            if let Some(key) = english_reverse::surface_key(surface)
                && english_reverse::reverse_match(prefix, &key).is_some()
            {
                self.user_data.record(prefix, surface);
                self.session_history.record_commit(prefix, surface);
            } else {
                self.session_history.reset_context();
            }
            return;
        };
        let previous = self
            .session_history
            .previous_commit()
            .map(|(reading, surface)| (reading.to_owned(), surface.to_owned()));
        if let Some((previous_reading, previous_surface)) = previous {
            self.user_data
                .record_context(&previous_reading, &previous_surface, &reading, surface);
        }
        self.session_history.record_commit(&reading, surface);
    }
}

#[cfg(test)]
fn bundled_dictionary(dictionary_packs: u32, user_data: &UserData) -> Dictionary {
    bundled_dictionary_with_packs(dictionary_packs, user_data, &DictionaryPackStore::default())
}

fn bundled_dictionary_with_packs(
    dictionary_packs: u32,
    user_data: &UserData,
    installed_packs: &DictionaryPackStore,
) -> Dictionary {
    let mut layers = domain_dictionaries::layers(dictionary_packs);
    layers.extend(installed_packs.layers());
    if let Some(user_layer) = domain_dictionaries::user_layer(user_data.dictionary_entries()) {
        layers.push(user_layer);
    }
    Dictionary::bundled_with_layers(layers)
}

fn select_candidate_corrections(
    ranked: Vec<(CandidateCorrection, (u8, i32))>,
    limit: usize,
) -> Vec<CandidateCorrection> {
    let mut selected = Vec::with_capacity(limit);
    for (correction, _) in &ranked {
        let same_reading = selected
            .iter()
            .filter(|existing: &&CandidateCorrection| existing.reading == correction.reading)
            .count();
        if same_reading < 2 {
            selected.push(correction.clone());
        }
        if selected.len() == limit {
            return selected;
        }
    }
    for (correction, _) in ranked {
        if selected
            .iter()
            .all(|existing| existing.surface != correction.surface)
        {
            selected.push(correction);
        }
        if selected.len() == limit {
            break;
        }
    }
    selected
}

fn aligned_segment_surface(
    conversion: &Conversion,
    target_start: usize,
    target_end: usize,
) -> Option<&str> {
    let mut start = 0;
    for segment in &conversion.segments {
        let end = start + segment.reading.len();
        if start == target_start && end == target_end {
            return Some(&segment.surface);
        }
        if end > target_end {
            return None;
        }
        start = end;
    }
    None
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn insert_unique_candidates_after_first(values: &mut Vec<String>, additions: Vec<String>) {
    let mut index = usize::from(!values.is_empty());
    for value in additions {
        if values.contains(&value) {
            continue;
        }
        values.insert(index, value);
        index += 1;
    }
}

fn should_record_history(reading: &str, surface: &str) -> bool {
    user_data::is_useful_history(reading, surface)
}

fn editable_segment(segment: Segment) -> EditableSegment {
    EditableSegment {
        reading: segment.reading,
        surface: segment.surface,
        explicitly_selected: false,
    }
}

fn transform_text(style: TransformStyle, reading: &str, raw: Option<&str>) -> String {
    match style {
        TransformStyle::Hiragana => text_transform::hiragana(reading),
        TransformStyle::FullKatakana => text_transform::full_katakana(reading),
        TransformStyle::HalfKatakana => text_transform::half_katakana(reading),
        TransformStyle::FullAlphanumeric => {
            let romanized;
            let source = if let Some(raw) = raw {
                raw
            } else {
                romanized = text_transform::romanize(reading);
                &romanized
            };
            text_transform::full_alphanumeric(source)
        }
        TransformStyle::HalfAlphanumeric => raw.map_or_else(
            || text_transform::romanize(reading),
            text_transform::half_alphanumeric,
        ),
    }
}

fn is_hiragana_or_mark(character: char) -> bool {
    matches!(character, '\u{3041}'..='\u{3096}' | 'ー' | 'ゝ' | 'ゞ')
}

fn live_ranked_target_is_safe(
    has_stable_prefix: bool,
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    if rewrites_katakana_word_tail_as_particle(base_surface, selected_surface) {
        return false;
    }
    let implicit_numeric_repair = !contains_decimal_digit(target_reading)
        && contains_decimal_digit(base_surface)
        && !contains_decimal_digit(selected_surface);
    let specific_word_repair =
        rewrites_literal_tail_as_specific_compound(target_reading, base_surface, selected_surface)
            || rewrites_literal_tail_as_inflected_word(
                target_reading,
                base_surface,
                selected_surface,
                has_stable_prefix,
            )
            || rewrites_literal_tail_as_katakana_list(
                target_reading,
                base_surface,
                selected_surface,
            );
    if !implicit_numeric_repair
        && !specific_word_repair
        && rewrites_live_inflection_suffix(target_reading, base_surface, selected_surface)
    {
        return false;
    }
    let protects_short_suffix = has_stable_prefix
        && target_reading.chars().count() <= MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS;
    if protects_short_suffix
        && !implicit_numeric_repair
        && !specific_word_repair
        && selected_surface.chars().count() != base_surface.chars().count()
    {
        return false;
    }

    // A short literal tail after a stable converted prefix is usually a
    // particle, auxiliary, inflection, or conjunction. Automatically
    // replacing it with an ambiguous kanji is visually disruptive. Longer
    // targets and whole-reading tasks still benefit from semantic reranking,
    // while implicit numeric repair remains independently allowed.
    implicit_numeric_repair
        || specific_word_repair
        || !protects_short_suffix
        || base_surface != target_reading
        || selected_surface == base_surface
}

fn rewrites_katakana_word_tail_as_particle(base_surface: &str, selected_surface: &str) -> bool {
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    if base.len() != selected.len() {
        return false;
    }
    let differences: Vec<_> = base
        .iter()
        .zip(&selected)
        .enumerate()
        .filter(|(_, (base, selected))| base != selected)
        .collect();
    let [(index, (base_character, selected_character))] = differences.as_slice() else {
        return false;
    };
    let index = *index;
    index >= 2
        && matches!(base_character, 'ァ'..='ヺ')
        && is_likely_particle_character(**selected_character)
        && char::from_u32(u32::from(**selected_character) + 0x60) == Some(**base_character)
        && matches!(base[index - 1], 'ァ'..='ヺ')
        && matches!(base[index - 2], 'ァ'..='ヺ')
        && base.get(index + 1) == selected.get(index + 1)
        && base
            .get(index + 1)
            .is_some_and(|character| is_likely_particle_character(*character))
}

fn rewrites_reopened_two_kana_inflected_target(
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    let reading: Vec<_> = target_reading.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    reading.len() == 2
        && base_surface == target_reading
        && selected.len() == 2
        && is_hiragana_or_mark(reading[0])
        && is_kanji_character(selected[0])
        && selected[1] == reading[1]
        && is_hiragana_or_mark(selected[1])
}

fn rewrites_particle_looking_suffix_as_two_kanji_word(
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    let reading: Vec<_> = target_reading.chars().collect();
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    (3..=4).contains(&reading.len())
        && base.len() == 2
        && selected.len() == 2
        && is_kanji_character(base[0])
        && reading.last().is_some_and(|last| base[1] == *last)
        && is_likely_particle_character(base[1])
        && selected.iter().copied().all(is_kanji_character)
        && selected != base
}

fn rewrites_literal_tail_as_specific_compound(
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    let reading: Vec<_> = target_reading.chars().collect();
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    let maximum = MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS
        .min(reading.len())
        .min(base.len());
    (1..=maximum)
        .take_while(|length| {
            let base_character = base[base.len() - length];
            base_character == reading[reading.len() - length] && is_hiragana_or_mark(base_character)
        })
        .any(|literal_suffix_length| {
            let prefix_length = base.len() - literal_suffix_length;
            let Some(selected_prefix) = selected.get(..prefix_length) else {
                return false;
            };
            let Some(selected_tail) = selected.get(prefix_length..) else {
                return false;
            };
            selected_prefix == &base[..prefix_length]
                && selected_tail.len() >= 2
                && selected_tail.len() <= literal_suffix_length
                && selected_tail.iter().copied().all(is_kanji_character)
        })
}

fn rewrites_literal_tail_as_inflected_word(
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
    has_stable_prefix: bool,
) -> bool {
    let reading: Vec<_> = target_reading.chars().collect();
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    let maximum = MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS
        .min(reading.len())
        .min(base.len());
    let literal_suffix_length = (1..=maximum)
        .take_while(|length| {
            let base_character = base[base.len() - length];
            base_character == reading[reading.len() - length] && is_hiragana_or_mark(base_character)
        })
        .last()
        .unwrap_or(0);
    if literal_suffix_length == 0 {
        return false;
    }

    let prefix_length = base.len() - literal_suffix_length;
    let Some(selected_tail) = selected.get(prefix_length..) else {
        return false;
    };
    if selected.get(..prefix_length) != Some(&base[..prefix_length]) {
        return false;
    }
    let literal_tail = &base[prefix_length..];
    let unchanged_prefix_length = literal_tail
        .iter()
        .zip(selected_tail)
        .take_while(|(literal, selected)| literal == selected)
        .count();
    let maximum_inflection_length = literal_tail
        .len()
        .saturating_sub(unchanged_prefix_length)
        .min(selected_tail.len().saturating_sub(unchanged_prefix_length));
    let inflection_length = literal_tail
        .iter()
        .rev()
        .zip(selected_tail.iter().rev())
        .take_while(|(literal, selected)| literal == selected && is_hiragana_or_mark(**literal))
        .take(maximum_inflection_length)
        .count();
    if inflection_length == 0 {
        return false;
    }
    if inflection_length == 1
        && literal_tail
            .last()
            .is_some_and(|character| is_likely_particle_character(*character))
    {
        return false;
    }

    let literal_stem_length = literal_tail.len() - unchanged_prefix_length - inflection_length;
    let selected_stem =
        &selected_tail[unchanged_prefix_length..selected_tail.len() - inflection_length];
    !selected_stem.is_empty()
        && (literal_stem_length >= 2
            || (has_stable_prefix && inflection_length >= 3)
            || base[..prefix_length]
                .iter()
                .copied()
                .any(is_live_converted_character))
        && selected_stem.iter().copied().all(is_kanji_character)
}

fn is_likely_particle_character(character: char) -> bool {
    matches!(
        character,
        'は' | 'が' | 'を' | 'に' | 'で' | 'と' | 'も' | 'へ' | 'の'
    )
}

fn is_live_boundary_particle_character(character: char) -> bool {
    matches!(
        character,
        'は' | 'が' | 'や' | 'を' | 'に' | 'へ' | 'と' | 'で' | 'も' | 'の'
    )
}

fn live_prefix_ends_particle(prefix: &LiveStablePrefix) -> bool {
    prefix
        .reading
        .chars()
        .next_back()
        .zip(prefix.surface.chars().next_back())
        .is_some_and(|(reading, surface)| {
            reading == surface && is_live_boundary_particle_character(reading)
        })
}

fn is_kanji_character(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '々' | '〆'
    )
}

fn rewrites_live_inflection_suffix(
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    let reading: Vec<_> = target_reading.chars().collect();
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    let maximum = MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS
        .min(reading.len())
        .min(base.len());
    let suffix_length = (1..=maximum)
        .take_while(|length| {
            let base_character = base[base.len() - length];
            base_character == reading[reading.len() - length] && is_hiragana_or_mark(base_character)
        })
        .last()
        .unwrap_or(0);
    if suffix_length == 0 {
        return false;
    }

    let converted_prefix = &base[..base.len() - suffix_length];
    !converted_prefix.is_empty()
        && converted_prefix
            .iter()
            .copied()
            .any(is_live_converted_character)
        && (reading.len() <= MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS
            || selected.starts_with(converted_prefix))
        && selected.get(selected.len().saturating_sub(suffix_length)..)
            != Some(&base[base.len() - suffix_length..])
}

fn contains_decimal_digit(surface: &str) -> bool {
    surface
        .chars()
        .any(|character| character.is_ascii_digit() || matches!(character, '０'..='９'))
}

fn rewrites_literal_tail_as_katakana_list(
    target_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    let reading: Vec<_> = target_reading.chars().collect();
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    let maximum = MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS
        .min(reading.len())
        .min(base.len());
    let literal_suffix_length = (1..=maximum)
        .take_while(|length| {
            let base_character = base[base.len() - length];
            base_character == reading[reading.len() - length] && is_hiragana_or_mark(base_character)
        })
        .last()
        .unwrap_or(0);
    literal_suffix_length > 0
        && converts_katakana_list_tail(&base, &selected, base.len() - literal_suffix_length)
}

fn rewrites_only_protected_literal_tail(
    resolved_reading: &str,
    base_surface: &str,
    selected_surface: &str,
) -> bool {
    if rewrites_literal_tail_as_specific_compound(resolved_reading, base_surface, selected_surface)
        || rewrites_literal_tail_as_inflected_word(
            resolved_reading,
            base_surface,
            selected_surface,
            false,
        )
        || rewrites_literal_tail_as_katakana_list(resolved_reading, base_surface, selected_surface)
    {
        return false;
    }
    if rewrites_only_hiragana_as_katakana(base_surface, selected_surface) {
        return true;
    }
    let reading: Vec<_> = resolved_reading.chars().collect();
    let base: Vec<_> = base_surface.chars().collect();
    let selected: Vec<_> = selected_surface.chars().collect();
    let maximum = MAX_PROTECTED_LITERAL_LIVE_SUFFIX_CHARACTERS
        .min(reading.len())
        .min(base.len());
    let literal_suffix_length = (1..=maximum)
        .take_while(|length| {
            let base_character = base[base.len() - length];
            base_character == reading[reading.len() - length] && is_hiragana_or_mark(base_character)
        })
        .last()
        .unwrap_or(0);
    if literal_suffix_length == 0 {
        return false;
    }

    let protected_start = base.len() - literal_suffix_length;
    let converted_prefix = &base[..protected_start];
    !converted_prefix.is_empty()
        && converted_prefix
            .iter()
            .copied()
            .any(is_live_converted_character)
        && selected.get(..protected_start) == Some(converted_prefix)
        && selected != base
}

fn safer_literal_tail_alternative<'a>(
    request: &'a CandidateRankingRequest,
    selected_surface: &str,
) -> Option<&'a str> {
    request
        .candidates
        .iter()
        .map(|candidate| candidate.surface.as_str())
        .find(|candidate| {
            *candidate != selected_surface
                && rewrites_short_hiragana_tail_as_katakana(candidate, selected_surface)
        })
}

fn rewrites_short_hiragana_tail_as_katakana(
    hiragana_surface: &str,
    katakana_surface: &str,
) -> bool {
    let hiragana: Vec<_> = hiragana_surface.chars().collect();
    let katakana: Vec<_> = katakana_surface.chars().collect();
    if hiragana.len() != katakana.len() {
        return false;
    }
    let Some(first_difference) = hiragana
        .iter()
        .zip(&katakana)
        .position(|(hiragana, katakana)| hiragana != katakana)
    else {
        return false;
    };
    let suffix_length = hiragana.len().saturating_sub(first_difference);
    first_difference > 0
        && (1..=3).contains(&suffix_length)
        && is_short_hiragana_function_tail(&hiragana[first_difference..])
        && hiragana[..first_difference]
            .iter()
            .copied()
            .any(is_live_converted_character)
        && hiragana[first_difference..]
            .iter()
            .zip(&katakana[first_difference..])
            .all(|(hiragana, katakana)| {
                matches!(hiragana, 'ぁ'..='ゖ' | 'ゝ'..='ゞ')
                    && char::from_u32(u32::from(*hiragana) + 0x60) == Some(*katakana)
            })
}

fn is_short_hiragana_function_tail(tail: &[char]) -> bool {
    matches!(
        tail,
        ['は' | 'が' | 'を' | 'に' | 'で' | 'と' | 'も' | 'へ' | 'の']
            | ['か' | 'な', 'ら']
            | ['ま' | 'の', 'で']
            | ['よ' | 'た' | 'だ', 'り']
            | ['と' | 'し', 'か']
            | ['だ', 'け']
            | ['ほ' | 'な', 'ど']
            | ['こ', 'そ']
            | ['さ', 'え']
            | ['で', 'も']
            | ['の', 'に']
            | ['っ', 'て']
            | ['つ', 'つ']
    )
}

fn rewrites_only_hiragana_as_katakana(base_surface: &str, selected_surface: &str) -> bool {
    let mut changed_characters = 0_usize;
    let mut base = base_surface.chars();
    let mut selected = selected_surface.chars();
    loop {
        match (base.next(), selected.next()) {
            (Some(base), Some(selected)) if base == selected => {}
            (Some(base @ ('ぁ'..='ゖ' | 'ゝ'..='ゞ')), Some(selected))
                if char::from_u32(u32::from(base) + 0x60) == Some(selected) =>
            {
                changed_characters = changed_characters.saturating_add(1);
            }
            (None, None) => return changed_characters >= 4,
            _ => return false,
        }
    }
}

fn converts_katakana_list_tail(base: &[char], selected: &[char], tail_start: usize) -> bool {
    let prefix = &base[..tail_start];
    base.len().saturating_sub(tail_start) >= 4
        && prefix.last() == Some(&'、')
        && prefix[..prefix.len().saturating_sub(1)]
            .iter()
            .rev()
            .take_while(|character| matches!(**character, 'ァ'..='ヺ' | 'ｦ'..='ﾟ' | 'ー'))
            .take(3)
            .count()
            == 3
        && selected.get(tail_start..).is_some_and(|selected_tail| {
            let literal_tail: String = base[tail_start..].iter().collect();
            let selected_tail: String = selected_tail.iter().collect();
            selected_tail == text_transform::full_katakana(&literal_tail)
        })
}

fn is_live_converted_character(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | 'ァ'..='ヺ'
            | 'ｦ'..='ﾟ'
            | '0'..='9'
            | '０'..='９'
    )
}

fn is_one_kana_extension(prefix: &str, reading: &str) -> bool {
    let Some(suffix) = reading.strip_prefix(prefix) else {
        return false;
    };
    let mut characters = suffix.chars();
    characters.next().is_some_and(is_hiragana_or_mark) && characters.next().is_none()
}

fn converted_surface_run_count(surface: &str) -> usize {
    let mut previous_was_converted = false;
    surface.chars().fold(0, |runs, character| {
        let converted = is_live_converted_character(character);
        let starts_run = converted && !previous_was_converted;
        previous_was_converted = converted;
        runs + usize::from(starts_run)
    })
}

fn katakana_candidate(reading: &str) -> String {
    reading
        .chars()
        .map(|character| match character {
            '\u{3041}'..='\u{3096}' | '\u{309d}'..='\u{309e}' => {
                char::from_u32(u32::from(character) + 0x60)
                    .expect("Hiragana letters have corresponding Katakana letters")
            }
            _ => character,
        })
        .collect()
}

fn insert_visible_katakana_candidate(candidates: &mut Vec<String>, reading: &str) {
    let katakana = katakana_candidate(reading);
    if katakana == reading {
        return;
    }

    if let Some(index) = candidates
        .iter()
        .position(|candidate| candidate == &katakana)
    {
        if index <= 1 {
            return;
        }
        candidates.remove(index);
    }
    candidates.insert(usize::from(!candidates.is_empty()), katakana);
}

fn literal_extension_surface(preview: &LivePreview, reading: &str) -> Option<String> {
    literal_surface_extension(&preview.reading, &preview.surface, reading)
}

fn ambiguous_long_prefix_literal_extension(
    dictionary: &Dictionary,
    preview: &LivePreview,
    reading: &str,
    ambiguous_surface: Option<&str>,
) -> Option<(String, LiveStablePrefix, bool)> {
    if preview
        .reading
        .chars()
        .filter(|character| is_hiragana_or_mark(*character))
        .count()
        < MINIMUM_AMBIGUOUS_CONTINUITY_READING_CHARACTERS
        || preview.surface == preview.reading
        || contains_decimal_digit(&preview.surface)
        || !preview
            .surface
            .chars()
            .next_back()
            .is_some_and(is_live_converted_character)
    {
        return None;
    }
    let suffix = reading.strip_prefix(&preview.reading)?;
    if suffix.chars().count() != 1 {
        return None;
    }
    let supported_by_best =
        ambiguous_surface.is_some_and(|surface| surface.starts_with(&preview.surface));
    let supported_by_close_path = !supported_by_best
        && reading.chars().count() <= MAXIMUM_AMBIGUOUS_CONTINUITY_SEARCH_READING_CHARACTERS
        && {
            let candidates = dictionary.convert_n_best(reading, AMBIGUOUS_CONTINUITY_PATH_LIMIT);
            let best_cost = candidates.first().map(|candidate| candidate.cost)?;
            candidates.into_iter().any(|candidate| {
                candidate.surface.starts_with(&preview.surface)
                    && candidate.cost.saturating_sub(best_cost)
                        <= MAXIMUM_AMBIGUOUS_CONTINUITY_COST_GAP
            })
        };
    if !supported_by_best && !supported_by_close_path {
        return None;
    }
    let surface = literal_extension_surface(preview, reading)?;
    let prefix = LiveStablePrefix {
        reading: preview.reading.clone(),
        surface: preview.surface.clone(),
    };
    Some((surface, prefix, supported_by_close_path))
}

fn sealable_nonfragile_live_rollback_surface(
    preview: &LivePreview,
    reading: &str,
) -> Option<String> {
    if !preview.reliable
        || !preview.sealable_bunsetsu
        || preview.prefix_fragile
        || contains_decimal_digit(&preview.surface)
    {
        return None;
    }
    let suffix = reading.strip_prefix(&preview.reading)?;
    if suffix.chars().count() != 1 {
        return None;
    }
    literal_extension_surface(preview, reading)
}

fn live_continuity_checkpoint<'a>(
    previous: Option<&'a LivePreview>,
    reading: &str,
) -> (Option<String>, Option<&'a str>) {
    let checkpoint = previous
        .filter(|preview| preview.is_continuity_checkpoint())
        .and_then(|preview| {
            preview.fallback_prefix.as_ref().filter(|checkpoint| {
                checkpoint.reading == preview.reading && checkpoint.surface == preview.surface
            })
        });
    let surface = checkpoint
        .and(previous)
        .and_then(|preview| literal_extension_surface(preview, reading));
    (
        surface,
        checkpoint.map(|checkpoint| checkpoint.surface.as_str()),
    )
}

fn full_live_display_surfaces<'a>(
    previous: Option<&'a LivePreview>,
    pending_prefix: Option<&'a LiveStablePrefix>,
    reading: &str,
) -> (Option<String>, Option<&'a str>) {
    let word_checkpoint_surface =
        previous.and_then(|preview| word_checkpoint_extension_surface(preview, reading));
    let (continuity_surface, continuity_checkpoint) = live_continuity_checkpoint(previous, reading);
    let stable_surface = previous
        .filter(|preview| preview.reliable)
        .and_then(|preview| literal_extension_surface(preview, reading))
        .or(word_checkpoint_surface)
        .or(continuity_surface);
    let protected_prefix = previous
        .and_then(|preview| preview.word_checkpoint.as_ref())
        .filter(|checkpoint| reading.starts_with(&checkpoint.reading))
        .map(|checkpoint| checkpoint.surface.as_str())
        .or_else(|| protected_pending_prefix_surface(previous, pending_prefix, reading))
        .or(continuity_checkpoint);
    (stable_surface, protected_prefix)
}

fn word_checkpoint_extension_surface(preview: &LivePreview, reading: &str) -> Option<String> {
    let checkpoint = preview.word_checkpoint.as_ref()?;
    let suffix = reading.strip_prefix(&checkpoint.reading)?;
    if suffix.is_empty()
        || suffix.chars().count() > MAXIMUM_WORD_CHECKPOINT_EXTENSION_CHARACTERS
        || suffix.chars().next().is_some_and(|character| {
            is_likely_particle_character(character) || is_live_fallback_boundary(character)
        })
    {
        return None;
    }
    Some(joined_live_surface(&checkpoint.surface, suffix))
}

fn word_checkpoint_particle_boundary(
    preview: &LivePreview,
    reading: &str,
) -> Option<LiveStablePrefix> {
    let checkpoint = preview.word_checkpoint.as_ref()?;
    let suffix = reading.strip_prefix(&checkpoint.reading)?;
    let mut characters = suffix.chars();
    let particle = characters.next()?;
    if characters.next().is_some() || !is_likely_particle_character(particle) {
        return None;
    }
    Some(LiveStablePrefix {
        reading: reading.to_owned(),
        surface: joined_live_surface(&checkpoint.surface, suffix),
    })
}

fn literal_surface_extension(
    previous_reading: &str,
    previous_surface: &str,
    reading: &str,
) -> Option<String> {
    let suffix = reading.strip_prefix(previous_reading)?;
    if suffix.is_empty() {
        return None;
    }

    let mut surface = String::with_capacity(previous_surface.len() + suffix.len());
    surface.push_str(previous_surface);
    surface.push_str(suffix);
    Some(surface)
}

fn ends_live_fallback_boundary(reading: &str) -> bool {
    reading
        .chars()
        .next_back()
        .is_some_and(is_live_fallback_boundary)
}

fn is_live_fallback_boundary(character: char) -> bool {
    matches!(
        character,
        '、' | '。'
            | '・'
            | '，'
            | '．'
            | '！'
            | '？'
            | '：'
            | '；'
            | '（'
            | '［'
            | '｛'
            | '「'
            | '『'
            | '【'
            | '〈'
            | '《'
            | '）'
            | '］'
            | '｝'
            | '」'
            | '』'
            | '】'
            | '〉'
            | '》'
    )
}

fn is_live_opening_boundary(character: char) -> bool {
    matches!(
        character,
        '（' | '［' | '｛' | '「' | '『' | '【' | '〈' | '《'
    )
}

fn live_stable_prefix(preview: &LivePreview) -> LiveStablePrefix {
    LiveStablePrefix {
        reading: preview.reading.clone(),
        surface: preview.surface.clone(),
    }
}

fn joined_live_surface(prefix: &str, target: &str) -> String {
    let mut surface = String::with_capacity(prefix.len() + target.len());
    surface.push_str(prefix);
    surface.push_str(target);
    surface
}

fn live_suffix_context<'a>(
    previous: &'a LivePreview,
    prefix: &LiveStablePrefix,
    target_reading: &str,
) -> (Option<LiveStablePrefix>, &'a str, Option<String>) {
    let pending_prefix = previous
        .sealable_bunsetsu
        .then(|| eligible_live_pending_prefix(previous))
        .flatten()
        .or_else(|| previous.pending_prefix.clone());
    let previous_target_reading = previous
        .reading
        .strip_prefix(&prefix.reading)
        .unwrap_or_default();
    let previous_target_surface = previous
        .surface
        .strip_prefix(&prefix.surface)
        .unwrap_or_default();
    // A retained display must not become evidence for a stable conversion on
    // the next key. Keep the same unresolved search input as the literal path.
    let decision_surface = if previous.checkpoint_kind == LiveCheckpointKind::SuffixContinuity {
        previous_target_reading
    } else {
        previous_target_surface
    };
    let stable_surface = target_reading
        .strip_prefix(previous_target_reading)
        .filter(|suffix| !suffix.is_empty())
        .map(|suffix| joined_live_surface(decision_surface, suffix));
    (pending_prefix, previous_target_surface, stable_surface)
}

fn live_target_decision(
    decision: LiveConversionDecision,
    target_reading: &str,
) -> LiveTargetDecision {
    match decision {
        LiveConversionDecision::Confident(surface) => LiveTargetDecision {
            surface: surface.text,
            ends_bunsetsu: surface.ends_bunsetsu,
            kind: LiveTargetDecisionKind::Confident,
        },
        LiveConversionDecision::StableExtension(surface) => LiveTargetDecision {
            surface: surface.text,
            ends_bunsetsu: surface.ends_bunsetsu,
            kind: LiveTargetDecisionKind::StableExtension,
        },
        LiveConversionDecision::LatticeFallback(surface) => LiveTargetDecision {
            surface: surface.text,
            ends_bunsetsu: false,
            kind: LiveTargetDecisionKind::LatticeFallback,
        },
        LiveConversionDecision::ProtectedLiteral(surface) => LiveTargetDecision {
            surface,
            ends_bunsetsu: false,
            kind: LiveTargetDecisionKind::ProtectedLiteral,
        },
        LiveConversionDecision::Continuity(surface) => LiveTargetDecision {
            surface,
            ends_bunsetsu: false,
            kind: LiveTargetDecisionKind::LatticeFallback,
        },
        LiveConversionDecision::DeferredFragile(_)
        | LiveConversionDecision::Ambiguous(_)
        | LiveConversionDecision::Literal => LiveTargetDecision {
            surface: target_reading.to_owned(),
            ends_bunsetsu: false,
            kind: LiveTargetDecisionKind::Literal,
        },
    }
}

/// Preserve already displayed text while only the next word is incomplete.
/// This is a display fallback, not a new stable reading boundary: every next
/// key still searches the complete suffix, and a confident correction wins.
fn live_suffix_continuity_decision(
    previous: &LivePreview,
    prefix: &LiveStablePrefix,
    resolved: &str,
    decision: &LiveConversionDecision,
) -> Option<LiveTargetDecision> {
    let LiveConversionDecision::Ambiguous(best) = decision else {
        return None;
    };
    Some(LiveTargetDecision {
        surface: supported_live_suffix_extension(previous, prefix, resolved, best)?,
        ends_bunsetsu: false,
        kind: LiveTargetDecisionKind::SuffixContinuity,
    })
}

fn supported_live_suffix_extension(
    previous: &LivePreview,
    prefix: &LiveStablePrefix,
    resolved: &str,
    ambiguous_best: &str,
) -> Option<String> {
    let previous_reading = previous.reading.strip_prefix(&prefix.reading)?;
    let previous_surface = previous.surface.strip_prefix(&prefix.surface)?;
    if previous_reading.chars().count() < MINIMUM_SUFFIX_CONTINUITY_READING_CHARACTERS
        || previous.prefix_fragile
        || contains_decimal_digit(previous_surface)
        || !is_one_kana_extension(&previous.reading, resolved)
    {
        return None;
    }
    let converted_prefix = previous_surface.trim_end_matches(is_hiragana_or_mark);
    if converted_prefix
        .chars()
        .filter(|character| is_live_converted_character(*character))
        .count()
        < MINIMUM_LIVE_STABLE_PREFIX_CHARACTERS
        || !ambiguous_best.starts_with(converted_prefix)
    {
        return None;
    }
    let extension = resolved.strip_prefix(&previous.reading)?;
    Some(joined_live_surface(previous_surface, extension))
}

fn conflicting_live_fallback<'a>(
    previous: &'a LivePreview,
    resolved: &str,
    surface: &str,
    confident: bool,
) -> Option<&'a LiveStablePrefix> {
    (!confident)
        .then_some(previous.fallback_prefix.as_ref())
        .flatten()
        .filter(|fallback| {
            resolved.starts_with(&fallback.reading) && !surface.starts_with(&fallback.surface)
        })
}

fn pending_boundary_checkpoint(previous: &LivePreview, resolved: &str) -> Option<LiveStablePrefix> {
    (ends_live_fallback_boundary(resolved) && previous.pending_prefix.is_some())
        .then(|| literal_extension_surface(previous, resolved))
        .flatten()
        .map(|surface| LiveStablePrefix {
            reading: resolved.to_owned(),
            surface,
        })
}

fn has_mixed_converted_tail_before_boundary(surface: &str, boundary: char) -> bool {
    let Some(stem) = surface.strip_suffix(boundary) else {
        return false;
    };
    let mut has_kanji = false;
    let mut has_katakana = false;
    for character in stem.chars().rev() {
        if is_kanji_character(character) {
            has_kanji = true;
        } else if matches!(character, 'ァ'..='ヺ' | 'ｦ'..='ﾟ' | 'ー') {
            has_katakana = true;
        } else {
            break;
        }
    }
    has_kanji && has_katakana
}

fn converted_katakana_tail_before_boundary(surface: &str, boundary: char) -> usize {
    surface
        .strip_suffix(boundary)
        .map(|stem| {
            stem.chars()
                .rev()
                .take_while(|character| matches!(character, 'ァ'..='ヺ' | 'ｦ'..='ﾟ' | 'ー'))
                .count()
        })
        .unwrap_or_default()
}

fn crossing_katakana_run_prefix_characters(
    conversion: &Conversion,
    boundary: usize,
) -> Option<usize> {
    let mut segment_start = 0;
    let mut run_start = 0;
    let mut run_reading_characters = 0;
    let mut run_prefix_characters = 0;

    for segment in &conversion.segments {
        let segment_end = segment_start + segment.reading.len();
        let is_katakana = !segment.surface.is_empty()
            && segment
                .surface
                .chars()
                .all(|character| matches!(character, 'ァ'..='ヺ' | 'ｦ'..='ﾟ' | 'ー'));
        if !is_katakana {
            run_start = segment_end;
            run_reading_characters = 0;
            run_prefix_characters = 0;
            segment_start = segment_end;
            continue;
        }

        if run_reading_characters == 0 {
            run_start = segment_start;
        }
        let segment_reading_characters = segment.reading.chars().count();
        run_reading_characters += segment_reading_characters;
        if segment_end <= boundary {
            run_prefix_characters += segment_reading_characters;
        } else if segment_start < boundary {
            run_prefix_characters += segment
                .reading
                .get(..boundary - segment_start)?
                .chars()
                .count();
        }
        if run_start < boundary
            && boundary < segment_end
            && run_reading_characters >= 3
            && run_prefix_characters > 0
        {
            return Some(run_prefix_characters);
        }
        segment_start = segment_end;
    }
    None
}

fn conversion_has_literal_segment_crossing_boundary(
    conversion: &Conversion,
    boundary: usize,
    lexical_reading: &str,
) -> bool {
    let mut segment_start = 0;
    conversion.segments.iter().any(|segment| {
        let segment_end = segment_start + segment.reading.len();
        let crosses_boundary = segment_start < boundary
            && boundary < segment_end
            && segment.reading == lexical_reading
            && segment.surface == lexical_reading;
        segment_start = segment_end;
        crosses_boundary
    })
}

fn conversion_has_long_phrase_segment_crossing_boundary(
    conversion: &Conversion,
    boundary: usize,
) -> bool {
    let mut segment_start = 0;
    conversion.segments.iter().any(|segment| {
        let segment_end = segment_start + segment.reading.len();
        let relative_boundary = boundary.saturating_sub(segment_start);
        let crosses_boundary = segment_start < boundary
            && boundary < segment_end
            && segment
                .reading
                .get(..relative_boundary)
                .is_some_and(|prefix| prefix.chars().count() >= 3)
            && segment
                .reading
                .get(relative_boundary..)
                .is_some_and(|suffix| suffix.chars().count() >= 2)
            && segment
                .surface
                .chars()
                .filter(|&character| is_kanji_character(character))
                .count()
                >= 2
            && segment.surface.chars().any(is_hiragana_or_mark);
        segment_start = segment_end;
        crosses_boundary
    })
}

fn conversion_has_kanji_word_crossing_particle_boundary(
    conversion: &Conversion,
    boundary: usize,
) -> bool {
    let mut segment_start = 0;
    conversion.segments.iter().any(|segment| {
        let segment_end = segment_start + segment.reading.len();
        let relative_boundary = boundary.saturating_sub(segment_start);
        let crosses_boundary = segment_start < boundary
            && boundary < segment_end
            && segment
                .reading
                .get(..relative_boundary)
                .is_some_and(|prefix| prefix.chars().count() == 1)
            && segment
                .reading
                .get(relative_boundary..)
                .is_some_and(|suffix| suffix.chars().count() >= 3)
            && segment.reading.chars().count() >= 4
            && segment.surface.chars().count() >= 2
            && segment.surface.chars().all(is_kanji_character);
        segment_start = segment_end;
        crosses_boundary
    })
}

fn live_decision_surface<'a>(decision: &'a LiveConversionDecision, literal: &'a str) -> &'a str {
    match decision {
        LiveConversionDecision::Confident(surface)
        | LiveConversionDecision::StableExtension(surface)
        | LiveConversionDecision::LatticeFallback(surface) => &surface.text,
        LiveConversionDecision::ProtectedLiteral(surface)
        | LiveConversionDecision::Continuity(surface)
        | LiveConversionDecision::DeferredFragile(surface)
        | LiveConversionDecision::Ambiguous(surface) => surface,
        LiveConversionDecision::Literal => literal,
    }
}

fn live_decision_candidate_surface(decision: &LiveConversionDecision) -> Option<&str> {
    match decision {
        LiveConversionDecision::Confident(surface)
        | LiveConversionDecision::StableExtension(surface)
        | LiveConversionDecision::LatticeFallback(surface) => Some(&surface.text),
        LiveConversionDecision::ProtectedLiteral(surface)
        | LiveConversionDecision::Continuity(surface)
        | LiveConversionDecision::DeferredFragile(surface)
        | LiveConversionDecision::Ambiguous(surface) => Some(surface),
        LiveConversionDecision::Literal => None,
    }
}

fn surface_has_fragmented_literal_leading_edge(reading: &str, surface: &str) -> bool {
    let mut literal_prefix_characters = 0;
    let mut reading = reading.chars();
    let mut surface = surface.chars();
    loop {
        match (reading.next(), surface.next()) {
            (Some(reading), Some(surface))
                if reading == surface && is_hiragana_or_mark(surface) =>
            {
                literal_prefix_characters += 1;
            }
            (Some(reading), Some(surface)) => {
                return literal_prefix_characters >= 1
                    && reading != surface
                    && is_kanji_character(surface);
            }
            _ => return false,
        }
    }
}

fn conversion_has_fragmented_literal_leading_edge(conversion: &Conversion) -> bool {
    let mut literal_prefix_characters = 0;
    let mut segments = conversion.segments.iter();
    for segment in segments.by_ref() {
        if segment.reading == segment.surface && segment.surface.chars().all(is_hiragana_or_mark) {
            literal_prefix_characters += segment.reading.chars().count();
            continue;
        }
        let segment_reading_characters = segment.reading.chars().count();
        let segment_surface_characters = segment.surface.chars().count();
        let is_all_kanji =
            segment_surface_characters > 0 && segment.surface.chars().all(is_kanji_character);
        return is_all_kanji
            && ((literal_prefix_characters >= 2 && segment_reading_characters <= 2)
                || (literal_prefix_characters == 1
                    && segment_reading_characters >= 3
                    && segment_surface_characters >= 2));
    }
    false
}

fn live_boundary_may_continue_word(preview: &LivePreview, resolved_reading: &str) -> bool {
    // Particle-looking edges can still belong to a longer word: とこ may
    // continue ところ, and でき may continue できない. Keep the boundary
    // open without forcing either surface.
    let Some(suffix) = resolved_reading.strip_prefix(&preview.reading) else {
        return false;
    };
    (preview.reading.ends_with('と')
        && preview.surface.ends_with('と')
        && (suffix.starts_with('ら') || suffix.starts_with('こ')))
        || (preview.reading.ends_with('や')
            && preview.surface.ends_with('や')
            && suffix.chars().count() == 1)
        || (preview.reading.ends_with('で')
            && preview.surface.ends_with('で')
            && suffix.starts_with('き'))
}

fn live_pending_boundary_can_start_katakana_word(
    resolved_reading: &str,
    full_surface: &str,
    prefix: &LiveStablePrefix,
) -> bool {
    if !prefix.reading.ends_with('や') || !prefix.surface.ends_with('や') {
        return false;
    }
    let Some(target_reading) = resolved_reading.strip_prefix(&prefix.reading) else {
        return false;
    };
    let Some(target_surface) = full_surface.strip_prefix(&prefix.surface) else {
        return false;
    };
    target_reading.chars().count() >= 2
        && !target_surface.is_empty()
        && target_surface
            .chars()
            .all(|character| matches!(character, 'ァ'..='ヺ' | 'ｦ'..='ﾟ' | 'ー'))
}

fn reopen_stable_prefix_before_sokuon(preview: &mut LivePreview, resolved_reading: &str) {
    let is_followed_by_sokuon = preview
        .stable_prefix
        .as_ref()
        .and_then(|prefix| resolved_reading.strip_prefix(&prefix.reading))
        .is_some_and(|target| target.starts_with('っ'));
    if is_followed_by_sokuon {
        // A small tsu cannot begin an independent bunsetsu. The previous
        // particle-looking edge may instead be inside a compound, such as
        // とっけん or がっしょう. Reopen the full marked text so bounded
        // generation and delayed ranking can reconsider that boundary.
        preview.stable_prefix = None;
        preview.sealable_bunsetsu = false;
    }
}

fn eligible_live_stable_prefix(preview: &LivePreview) -> Option<LiveStablePrefix> {
    (preview.surface.chars().count() >= MINIMUM_LIVE_STABLE_PREFIX_CHARACTERS)
        .then(|| live_stable_prefix(preview))
}

fn eligible_live_pending_prefix(preview: &LivePreview) -> Option<LiveStablePrefix> {
    (preview.surface.chars().count() >= MINIMUM_LIVE_STABLE_PREFIX_CHARACTERS
        && !preview
            .reading
            .chars()
            .next()
            .is_some_and(is_live_fallback_boundary)
        && (!contains_decimal_digit(&preview.surface) || contains_decimal_digit(&preview.reading)))
    .then(|| live_stable_prefix(preview))
}

fn pending_live_prefix(previous: Option<&LivePreview>, resolved: &str) -> Option<LiveStablePrefix> {
    previous
        .and_then(|preview| {
            preview
                .sealable_bunsetsu
                .then(|| eligible_live_pending_prefix(preview))
                .flatten()
                .or_else(|| preview.pending_prefix.clone())
        })
        .filter(|prefix| {
            resolved
                .strip_prefix(&prefix.reading)
                .is_some_and(|suffix| {
                    !suffix.is_empty() && !suffix.chars().any(is_live_fallback_boundary)
                })
        })
}

fn live_boundary_fallback(
    previous: Option<&LivePreview>,
    resolved: &str,
) -> Option<LiveStablePrefix> {
    previous
        .filter(|preview| {
            ends_live_fallback_boundary(resolved)
                && (preview.pending_prefix.is_some()
                    || resolved
                        .chars()
                        .next_back()
                        .is_some_and(is_live_opening_boundary))
        })
        .and_then(|preview| literal_extension_surface(preview, resolved))
        .map(|surface| LiveStablePrefix {
            reading: resolved.to_owned(),
            surface,
        })
}

fn protected_pending_prefix_surface<'a>(
    previous: Option<&LivePreview>,
    pending_prefix: Option<&'a LiveStablePrefix>,
    resolved: &str,
) -> Option<&'a str> {
    let prefix = pending_prefix.filter(|prefix| {
        previous.is_some_and(|preview| preview.sealable_bunsetsu)
            && prefix.reading.chars().count() >= MINIMUM_PENDING_PREFIX_REWRITE_READING_CHARACTERS
            && resolved
                .strip_prefix(&prefix.reading)
                .is_some_and(|target| {
                    target.chars().count() == 1
                        && !(prefix.reading.ends_with('に') && target == "あ")
                })
    })?;
    Some(&prefix.surface)
}

fn unresolved_pending_suffix_is_short(prefix: &LiveStablePrefix, resolved: &str) -> bool {
    resolved
        .strip_prefix(&prefix.reading)
        .is_some_and(|suffix| suffix.chars().count() < 3)
}

fn normalize_ascii_character(character: char) -> char {
    match character {
        '-' => 'ー',
        '~' => '〜',
        ',' => '、',
        '.' => '。',
        // Japanese input commonly maps this key to the middle dot; it has no
        // other key on US layouts, while ／ stays reachable through
        // conversion candidates or ABC mode.
        '/' => '・',
        '[' => '「',
        ']' => '」',
        character @ '!'..='~' => char::from_u32(u32::from(character) + 0xFEE0)
            .expect("ASCII graphic characters have full-width forms"),
        character => character,
    }
}

impl Default for SlimeEngine {
    fn default() -> Self {
        Self::bundled()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_DATE_FORMATS, ALL_DOMAIN_DICTIONARIES, CandidateAnnotation, CandidateDetail,
        CandidateKind, CandidateRankingItem, CandidateRankingRequest, ConversionSearch,
        DictionaryPackTrust, DictionaryPackVerificationKey, DictionaryPackVersionFloor,
        DictionaryPackWord, EXPLICIT_RANKING_CANDIDATE_LIMIT, EnginePreferences, InputEvent,
        LIVE_RANKING_CANDIDATE_LIMIT, LiveCheckpointKind, LiveConversionDecision, LivePreview,
        MAX_EXPANDED_READING_CHARACTERS, MAX_RECENT_LIVE_SELECTIONS, Phase, SlimeAction,
        SlimeEngine, TECHNOLOGY_DICTIONARY, UserData, ambiguous_long_prefix_literal_extension,
        bundled_dictionary, date_time_candidates, diversify_implicit_numeric_live_candidates,
        diversify_recombined_live_candidates, katakana_candidate, live_ranked_target_is_safe,
        rewrites_only_protected_literal_tail,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use slime_converter::{Candidate, Dictionary, DictionaryEntry};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn current_live_neural_surface_requires_the_unchanged_applied_generation() {
        fn approved() -> SlimeEngine {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                ..EnginePreferences::default()
            });
            type_text(&mut engine, "ようしにはじしんがあるのか");
            assert!(engine.current_live_neural_surface().is_none());
            let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
            let request = snapshot.candidate_ranking_request().unwrap();
            let mut ranked: Vec<_> = request
                .candidates
                .iter()
                .map(|c| c.surface.clone())
                .collect();
            let position = ranked
                .iter()
                .position(|s| s == "容姿には自信があるのか")
                .unwrap();
            ranked[..=position].rotate_right(1);
            snapshot.prepare_ranked_prefix_validation(&request, &ranked);
            assert!(
                engine
                    .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                    .is_some()
            );
            assert_eq!(
                engine.current_live_neural_surface(),
                Some("容姿には自信があるのか")
            );
            engine
        }
        for event in [
            InputEvent::Character('あ'),
            InputEvent::Backspace,
            InputEvent::Space,
            InputEvent::Escape,
            InputEvent::TransformHiragana,
            InputEvent::Enter,
        ] {
            let mut engine = approved();
            engine.handle(event);
            assert!(engine.current_live_neural_surface().is_none(), "{event:?}");
        }
        let mut engine = approved();
        engine.set_external_left_context("別の文脈");
        assert!(engine.current_live_neural_surface().is_none());
        let mut engine = approved();
        engine.reset_context();
        assert!(engine.current_live_neural_surface().is_none());
        let mut engine = approved();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            private_mode: true,
            ..EnginePreferences::default()
        });
        assert!(engine.current_live_neural_surface().is_none());
    }

    #[test]
    fn guided_live_joint_proof_is_worker_prepared_and_scope_bound() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "ようしにはじしんがあるのか");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let selected = "容姿には自信があるのか";
        assert!(
            snapshot
                .selected_surface_for_application(&request, selected)
                .is_none()
        );
        assert!(snapshot.guided_joint_repair_approval.get().is_none());
        snapshot.prepare_ranked_prefix_validation(&request, &[selected.to_owned()]);
        assert!(snapshot.guided_joint_repair_approval.get().is_some());
        assert!(
            !snapshot
                .joint_repair_paths
                .get()
                .unwrap()
                .iter()
                .any(|path| path.surface == selected)
        );
        assert_eq!(
            snapshot
                .selected_surface_for_application(&request, selected)
                .as_deref(),
            Some(selected)
        );
        let mut missing_candidate = request.clone();
        missing_candidate
            .candidates
            .retain(|c| c.surface != selected);
        assert!(!snapshot.reopened_request_repairs_two_aligned_words(&missing_candidate, selected));
        let mut wrong_context = request.clone();
        wrong_context.left_context.push('別');
        assert!(!snapshot.reopened_request_repairs_two_aligned_words(&wrong_context, selected));
        let mut wrong_reading = request.clone();
        wrong_reading.reading.push('あ');
        assert!(!snapshot.reopened_request_repairs_two_aligned_words(&wrong_reading, selected));
        assert!(
            !snapshot
                .reopened_request_repairs_two_aligned_words(&request, "用紙には自信があるのか")
        );
        assert!(
            !snapshot
                .reopened_request_repairs_two_aligned_words(&request, "容姿には自信があるのが")
        );
    }

    #[test]
    fn guided_live_joint_proof_rejects_candidates_outside_the_request() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "ようしにはじしんがあるのか");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let mut request = snapshot.candidate_ranking_request().unwrap();
        request
            .candidates
            .retain(|c| c.surface != "容姿には自信があるのか");
        snapshot.prepare_ranked_prefix_validation(&request, &["容姿には自信があるのか".to_owned()]);
        assert!(snapshot.guided_joint_repair_approval.get().is_none());
    }

    #[test]
    fn live_two_word_repair_preserves_inflected_target_and_punctuation() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            ..EnginePreferences::default()
        });
        type_text(
            &mut engine,
            "がしさくされたが、せいしきさいようされなかった",
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let selected = "が試作されたが、制式採用されなかった";
        assert_eq!(
            snapshot.base_surface,
            "が施策されたが、正式採用されなかった"
        );
        assert!(snapshot.joint_repair_paths.get().is_none());
        snapshot.prepare_ranked_prefix_validation(&request, &[selected.to_owned()]);
        assert!(snapshot.joint_repair_paths.get().is_some());
        assert_eq!(
            snapshot
                .selected_surface_for_application(&request, selected)
                .as_deref(),
            Some(selected)
        );
        for unsafe_surface in [
            "が試作されたが制式採用されなかった",
            "が試作されたが、制式採用されなかつた",
            "が試作されたが、制式採用されなかっ太",
        ] {
            assert!(!snapshot.reopened_request_repairs_two_aligned_words(&request, unsafe_surface));
        }
    }

    fn type_text(engine: &mut SlimeEngine, input: &str) {
        for character in input.chars() {
            engine.handle(InputEvent::Character(character));
        }
    }

    fn ambiguous_precision_engine(user_data: UserData) -> SlimeEngine {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("へんかんせいど", "変換制度", 0),
            DictionaryEntry::new("へんかんせいど", "変換精度", 666),
        ]);
        SlimeEngine::with_user_data(dictionary, user_data)
    }

    fn shown_candidate_details(actions: &[SlimeAction]) -> &[CandidateDetail] {
        actions
            .iter()
            .find_map(|action| match action {
                SlimeAction::ShowCandidates { details, .. } => Some(details.as_slice()),
                _ => None,
            })
            .expect("show candidates action")
    }

    fn lower_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }

    fn write_context_pack(directory: &std::path::Path) {
        write_context_pack_with_rules(
            directory,
            "文章\tかんじ\t漢字\t0\n\
文章\tかんじ\t架空候補\t1\n",
        );
    }

    fn write_context_pack_with_rules(directory: &std::path::Path, rules: &str) {
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let payload = format!(
            "てすとようご\t試験用語\n\
ながいぶんしょう\tこれは長い文章\n\
# context-rules\n\
{rules}"
        );
        let digest = lower_hex(&Sha256::digest(payload.as_bytes()));
        fs::write(
            pack_directory.join("sample-context.slime-dict"),
            format!(
                "# slime-dictionary-pack-v3\n\
                 # id: sample-context\n\
                 # name: 文脈サンプル\n\
                 # version: 2026.08.1\n\
                 # license: Example-Test-Only\n\
                 # minimum-slime-version: 0.1.0\n\
                 # published-at: 2026-08-08\n\
                 # provenance: fixture/generated/sample-context\n\
                 # payload-sha256: {digest}\n\
                 # entries\n\
                 {payload}"
            ),
        )
        .unwrap();
    }

    fn sign_context_pack(directory: &std::path::Path, key_id: &str, signing_key: &SigningKey) {
        let pack_path = directory
            .join("dictionary-packs")
            .join("sample-context.slime-dict");
        let pack_bytes = fs::read(&pack_path).unwrap();
        let signature = signing_key.sign(&pack_bytes).to_bytes();
        let encoded = lower_hex(&signature);
        fs::write(
            pack_path.with_extension("slime-dict.sig"),
            format!(
                "# slime-dictionary-signature-v1\n\
                 # key-id: {key_id}\n\
                 # signature-ed25519: {encoded}\n"
            ),
        )
        .unwrap();
    }

    /// Closes the candidate window, then discards the reading.
    fn cancel_composition(engine: &mut SlimeEngine) {
        engine.handle(InputEvent::Escape);
        engine.handle(InputEvent::Escape);
        assert_eq!(engine.snapshot().preedit, "");
    }

    fn convert_and_commit(engine: &mut SlimeEngine, input: &str, surface: &str) {
        type_text(engine, input);
        engine.handle(InputEvent::Space);
        let index = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == surface)
            .unwrap_or_else(|| panic!("missing candidate {surface} for {input}"));
        engine.handle(InputEvent::SelectCandidate(u32::try_from(index).unwrap()));
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit(surface.to_owned())));
    }

    fn accept_completion(engine: &mut SlimeEngine, input: &str, surface: &str) {
        type_text(engine, input);
        let index = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == surface)
            .unwrap_or_else(|| panic!("missing completion {surface} for {input}"));
        engine.handle(InputEvent::SelectCandidate(u32::try_from(index).unwrap()));
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit(surface.to_owned())));
    }

    fn test_directory(name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "slime-core-{name}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn ambiguous_trailing_n_remains_literal_in_preedit() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");

        assert_eq!(engine.snapshot().preedit, "にほn");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
    }

    #[test]
    fn ambiguous_n_stays_editable_and_double_n_is_one_syllabic_n() {
        let mut engine = SlimeEngine::bundled();

        type_text(&mut engine, "n");
        assert_eq!(engine.snapshot().preedit, "n");

        type_text(&mut engine, "n");
        assert_eq!(engine.snapshot().preedit, "ん");

        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("ん".to_owned())));
    }

    #[test]
    fn double_n_spends_both_keys_on_one_syllabic_n() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "sennyou");
        assert_eq!(engine.snapshot().preedit, "せんよう");

        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "annnai");
        assert_eq!(engine.snapshot().preedit, "あんない");
    }

    #[test]
    fn ascii_numbers_and_symbols_are_normalized_for_japanese_input() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "123,.!?()[]+-~/@#'");

        assert_eq!(
            engine.snapshot().preedit,
            "１２３、。！？（）「」＋ー〜・＠＃＇"
        );
    }

    #[test]
    fn arrow_shortcuts_are_composed_in_preedit() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "zhzm");

        assert_eq!(engine.snapshot().preedit, "←→");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("←→".to_owned())));
    }

    #[test]
    fn foreign_word_with_long_vowel_converts_to_dictionary_candidate() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "pafo-mansu");

        assert_eq!(engine.snapshot().preedit, "ぱふぉーまんす");

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "パフォーマンス");
    }

    #[test]
    fn live_conversion_updates_preedit_and_enter_commits_preview() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihongo");
        assert_eq!(engine.snapshot().preedit, "日本語");
        assert_eq!(engine.snapshot().phase, Phase::Composing);

        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("日本語".to_owned())));

        type_text(&mut engine, "iikanji");
        assert_eq!(engine.snapshot().preedit, "いい感じ");
    }

    #[test]
    fn live_conversion_leaves_single_kana_literal() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "mi");
        assert_eq!(engine.snapshot().preedit, "み");

        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("み".to_owned())));
    }

    #[test]
    fn live_conversion_defers_unfinished_or_ambiguous_input() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "sou");
        assert_eq!(engine.snapshot().preedit, "そう");

        type_text(&mut engine, "s");
        assert_eq!(engine.snapshot().preedit, "そうs");

        type_text(&mut engine, "hima");
        assert_eq!(engine.snapshot().preedit, "そうしま");

        type_text(&mut engine, "shou");
        assert_eq!(engine.snapshot().preedit, "そうしましょう");
        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("a literal or ambiguous LIVE state must still be eligible for delayed ranking");
        assert!(snapshot.target_is_literal());
    }

    #[test]
    fn live_conversion_defers_unstable_incomplete_word_paths() {
        for reading in ["とにかくめちゃくちゃ", "めちゃくちゃ"] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });

            let mut expected = String::new();
            for character in reading.chars() {
                expected.push(character);
                engine.handle(InputEvent::Character(character));
                assert_eq!(engine.snapshot().preedit, expected, "{expected}");
            }
        }

        let mut rare_katakana_prefix = SlimeEngine::bundled();
        rare_katakana_prefix.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut rare_katakana_prefix, "しんせ");
        assert_eq!(rare_katakana_prefix.snapshot().preedit, "しんせ");
        type_text(&mut rare_katakana_prefix, "ん");
        assert_eq!(rare_katakana_prefix.snapshot().preedit, "新鮮");

        for (reading, expected) in [
            ("にほん", "日本"),
            ("てすと", "テスト"),
            ("まじ", "マジ"),
            ("ふぁん", "ファン"),
            ("らーめん", "ラーメン"),
            ("いいかんじ", "いい感じ"),
            ("やきもの", "焼き物"),
            ("かいへん", "改変"),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });
            for character in reading.chars() {
                engine.handle(InputEvent::Character(character));
            }
            assert_eq!(engine.snapshot().preedit, expected, "{reading}");
        }
    }

    #[test]
    fn live_conversion_does_not_repeat_exact_word_flashes_after_a_kana_rollback() {
        let preferences = EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };

        let mut extending = SlimeEngine::bundled();
        extending.set_preferences(preferences);
        type_text(&mut extending, "しゅ");
        assert_eq!(extending.snapshot().preedit, "種");

        type_text(&mut extending, "う");
        assert_eq!(extending.snapshot().preedit, "しゅう");
        type_text(&mut extending, "か");
        assert_eq!(extending.snapshot().preedit, "しゅうか");
        extending.handle(InputEvent::Space);
        assert_eq!(extending.snapshot().preedit, "集荷");

        let mut extending = SlimeEngine::bundled();
        extending.set_preferences(preferences);
        type_text(&mut extending, "しゅうか");
        assert_eq!(extending.snapshot().preedit, "しゅうか");
        type_text(&mut extending, "んし");
        assert_eq!(extending.snapshot().preedit, "しゅうかんし");

        type_text(&mut extending, "ょうねんさんでー");
        assert_eq!(extending.snapshot().preedit, "週刊少年サンデー");
    }

    #[test]
    fn delayed_ranking_defers_only_prefix_fragile_exact_words() {
        let preferences = EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        };

        let mut synchronous_only = SlimeEngine::bundled();
        synchronous_only.set_preferences(preferences);
        type_text(&mut synchronous_only, "けい");
        assert_eq!(synchronous_only.snapshot().preedit, "系");

        let mut delayed = SlimeEngine::bundled();
        delayed.set_preferences(preferences);
        delayed.set_delayed_live_ranking_available(true);
        type_text(&mut delayed, "けい");
        assert_eq!(delayed.snapshot().preedit, "けい");
        assert!(delayed.live_candidate_ranking_snapshot().is_some());
        type_text(&mut delayed, "け");
        assert_eq!(delayed.snapshot().preedit, "けいけ");

        let mut stable_word = SlimeEngine::bundled();
        stable_word.set_preferences(preferences);
        stable_word.set_delayed_live_ranking_available(true);
        type_text(&mut stable_word, "にほん");
        assert_eq!(stable_word.snapshot().preedit, "日本");

        let mut katakana_word = SlimeEngine::bundled();
        katakana_word.set_preferences(preferences);
        katakana_word.set_delayed_live_ranking_available(true);
        type_text(&mut katakana_word, "しゃ");
        assert_eq!(katakana_word.snapshot().preedit, "しゃ");
        type_text(&mut katakana_word, "と");
        assert_eq!(katakana_word.snapshot().preedit, "者と");
        assert!(katakana_word.evaluation_live_sealable_bunsetsu().is_none());
        type_text(&mut katakana_word, "るば");
        assert_eq!(katakana_word.snapshot().preedit, "シャトルば");
        type_text(&mut katakana_word, "す");
        assert_eq!(katakana_word.snapshot().preedit, "シャトルバス");

        let mut compound = SlimeEngine::bundled();
        compound.set_preferences(preferences);
        compound.set_delayed_live_ranking_available(true);
        type_text(&mut compound, "きん");
        assert_eq!(compound.snapshot().preedit, "きん");
        type_text(&mut compound, "が");
        assert_eq!(compound.snapshot().preedit, "金が");
        assert!(compound.evaluation_live_sealable_bunsetsu().is_none());
        type_text(&mut compound, "く");
        assert_eq!(compound.snapshot().preedit, "金額");
    }

    #[test]
    fn live_conversion_keeps_a_sealable_nonfragile_bunsetsu_during_literal_extension() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "、にっぽんは");
        assert_eq!(engine.snapshot().preedit, "、日本は");
        assert!(engine.evaluation_live_sealable_bunsetsu().is_some());

        type_text(&mut engine, "つ");
        assert_eq!(engine.snapshot().preedit, "、日本はつ");
        assert!(engine.evaluation_live_literal_extension_checkpoint());
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(
            snapshot.evaluation_base_target_surface_for_request(&request),
            Some("、にっぽんはつ"),
            "the continuity display must not become the worker's logical base"
        );

        type_text(&mut engine, "のすいじょうじぇっとこーすたー");
        assert_eq!(
            engine.snapshot().preedit,
            "、日本初の水上ジェットコースター"
        );
    }

    #[test]
    fn live_conversion_uses_a_stable_runner_up_before_rewriting_a_long_literal() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let prefix = "はぜんしゅうまつ）のしあいをふりかえるほか、たんとうあなうんさーがちゅうけ";
        type_text(&mut engine, prefix);
        assert_eq!(engine.snapshot().preedit, prefix);

        type_text(&mut engine, "い");
        assert_eq!(engine.snapshot().preedit, format!("{prefix}い"));

        type_text(&mut engine, "さき");
        assert_eq!(
            engine.snapshot().preedit,
            "は前週末）の試合を振り返る他、担当アナウンサーが中継先"
        );
    }

    #[test]
    fn live_conversion_defers_a_short_converted_fragment_before_a_literal_tail() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let prefix = "た。」とこじんてきなかんそうをのこしたが、あいんしゅた";
        type_text(&mut engine, prefix);
        assert_eq!(engine.snapshot().preedit, prefix);

        type_text(&mut engine, "い");
        assert_eq!(engine.snapshot().preedit, format!("{prefix}い"));
    }

    #[test]
    fn live_conversion_checks_local_confidence_for_a_long_unmarked_implicit_number() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let prefix = "（たいせいよくさんかいさんかのにほんいどうえんげきれんめ";
        type_text(&mut engine, prefix);
        assert_eq!(engine.snapshot().preedit, prefix);

        type_text(&mut engine, "い");
        assert_eq!(engine.snapshot().preedit, format!("{prefix}い"));
    }

    #[test]
    fn live_conversion_keeps_a_long_compound_while_its_suffix_completes() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let prefix = "ぜんこくこうとうがっこうやきゅうせんしゅ";
        type_text(&mut engine, prefix);
        assert_eq!(engine.snapshot().preedit, "全国高等学校野球選手");

        type_text(&mut engine, "け");
        type_text(&mut engine, "ん");
        assert_eq!(engine.snapshot().preedit, "全国高等学校野球選手権");

        type_text(&mut engine, "か");
        assert_eq!(engine.snapshot().preedit, "全国高等学校野球選手権か");

        type_text(&mut engine, "ながわたいかい");
        assert_eq!(
            engine.snapshot().preedit,
            "全国高等学校野球選手権神奈川大会"
        );
    }

    #[test]
    fn live_conversion_rejects_weak_local_suffixes_during_long_path_continuity() {
        for (reading, expected) in [
            (
                "へんしんぶーむ」とよばれるしゃかいげんしょうをもひ",
                "返信ブーム」と呼ばれる社会現象をもひ",
            ),
            (
                "こうえんはきゅうでんのこうほうふぁさーどにはじまり、じんこ",
                "公園は宮殿の広報ファサードにはじまり、じんこ",
            ),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });

            type_text(&mut engine, reading);
            assert_eq!(engine.snapshot().preedit, expected, "{reading}");
        }
    }

    #[test]
    fn live_conversion_reuses_an_established_rare_katakana_surface() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let input = "とらんぷをてわたし、まかおとじょまのちきゅうせいふくけいかくをそしするようたのむが、まかおとじょまの";
        for character in input.chars() {
            engine.handle(InputEvent::Character(character));
        }
        assert_eq!(
            engine.snapshot().preedit,
            "トランプを手渡し、マカオとジョマの地球制服計画を阻止するよう頼むが、マカオとジョマの"
        );
    }

    #[test]
    fn live_conversion_uses_an_existing_lattice_path_for_an_ambiguous_suffix() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "seinenshideka");
        assert_eq!(engine.snapshot().preedit, "青年誌デカ");
        type_text(&mut engine, "ku");
        assert_eq!(engine.snapshot().preedit, "青年誌でかく");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| { !preview.reliable && preview.stable_prefix.is_none() })
        );
        assert!(
            engine
                .dictionary
                .convert_n_best("せいねんしでかく", 10)
                .iter()
                .any(|conversion| conversion.surface == "青年誌でかく"),
            "the display fallback must be an existing complete lattice path"
        );
    }

    #[test]
    fn live_conversion_preserves_a_surface_only_during_pending_romaji() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihon");
        assert_eq!(engine.snapshot().preedit, "日本");
        type_text(&mut engine, "g");
        assert_eq!(engine.snapshot().preedit, "日本g");
        type_text(&mut engine, "o");
        assert_eq!(engine.snapshot().preedit, "日本語");
        engine.handle(InputEvent::Enter);

        type_text(&mut engine, "tashika");
        assert_eq!(engine.snapshot().preedit, "確か");
        type_text(&mut engine, "n");
        assert_eq!(engine.snapshot().preedit, "確かn");
        type_text(&mut engine, "a");
        assert_eq!(engine.snapshot().preedit, "確かな");
        engine.handle(InputEvent::Enter);

        type_text(&mut engine, "kyouha");
        assert_eq!(engine.snapshot().preedit, "今日は");
        type_text(&mut engine, "ii");
        assert_eq!(engine.snapshot().preedit, "今日はいい");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("今日はいい".to_owned())));
    }

    #[test]
    fn live_conversion_keeps_lattice_confirmed_suffix_extensions() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        assert!(matches!(
            engine.live_conversion_decision("らいぶへんかん", None, None),
            LiveConversionDecision::Confident(surface)
                if surface.text == "ライブ変換" && !surface.ends_bunsetsu
        ));
        assert_eq!(
            engine.live_conversion_decision("らいぶへんかんで", None, None),
            LiveConversionDecision::Ambiguous("ライブ変換で".to_owned())
        );
        assert_eq!(
            engine.live_conversion_decision("らいぶへんかんの", None, None),
            LiveConversionDecision::Ambiguous("ライブ変換の".to_owned())
        );

        type_text(&mut engine, "raibuhenkan");
        assert_eq!(engine.snapshot().preedit, "ライブ変換");
        type_text(&mut engine, "d");
        assert_eq!(engine.snapshot().preedit, "ライブ変換d");
        type_text(&mut engine, "e");
        assert_eq!(engine.snapshot().preedit, "ライブ変換で");
        engine.handle(InputEvent::Enter);

        type_text(&mut engine, "raibuhenkannno");
        assert_eq!(engine.snapshot().preedit, "ライブ変換の");
    }

    #[test]
    fn live_conversion_moves_a_stable_bunsetsu_out_of_the_active_target() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkanga");
        assert_eq!(engine.snapshot().preedit, "変換が");

        type_text(&mut engine, "t");
        assert_eq!(engine.snapshot().preedit, "変換がt");
        type_text(&mut engine, "s");
        assert_eq!(engine.snapshot().preedit, "変換がts");
        type_text(&mut engine, "u");
        assert_eq!(engine.snapshot().preedit, "変換がつ");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| {
                    preview
                        .stable_prefix
                        .as_ref()
                        .or(preview.pending_prefix.as_ref())
                })
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("へんかんが", "変換が")),
            "the left edge stays protected while the one-kana suffix is not independently rankable"
        );

        type_text(&mut engine, "duku");
        assert_eq!(engine.snapshot().preedit, "変換が続く");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| {
                    preview
                        .stable_prefix
                        .as_ref()
                        .or(preview.pending_prefix.as_ref())
                })
                .map(|prefix| prefix.reading.as_str()),
            Some("へんかんが")
        );

        type_text(&mut engine, "to");
        assert_eq!(engine.snapshot().preedit, "変換が続くと");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| {
                    preview
                        .stable_prefix
                        .as_ref()
                        .or(preview.pending_prefix.as_ref())
                })
                .map(|prefix| prefix.reading.as_str()),
            Some("へんかんが")
        );

        type_text(&mut engine, "o");
        assert_eq!(engine.snapshot().preedit, "変換が続くとお");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| {
                    preview
                        .stable_prefix
                        .as_ref()
                        .or(preview.pending_prefix.as_ref())
                })
                .map(|prefix| prefix.reading.as_str()),
            Some("へんかんがつづくと")
        );
    }

    #[test]
    fn live_conversion_keeps_a_sealable_left_bunsetsu_when_the_suffix_is_ambiguous() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "ばすが");
        assert_eq!(engine.snapshot().preedit, "バスが");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu),
            "バスが is already safe to move out of the active target"
        );

        type_text(&mut engine, "くしろ");
        assert_eq!(engine.snapshot().preedit, "バスが釧路");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| {
                    preview
                        .stable_prefix
                        .as_ref()
                        .or(preview.pending_prefix.as_ref())
                })
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("ばすが", "バスが"))
        );

        type_text(&mut engine, "え");
        assert_eq!(engine.snapshot().preedit, "バスがくしろえ");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| {
                    preview
                        .stable_prefix
                        .as_ref()
                        .or(preview.pending_prefix.as_ref())
                })
                .map(|prefix| prefix.surface.as_str()),
            Some("バスが")
        );
        type_text(&mut engine, "きから");
        assert_eq!(engine.snapshot().preedit, "バスが釧路駅から");
    }

    #[test]
    fn live_conversion_does_not_flip_a_sealable_bunsetsu_on_the_next_kana() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "ふらんすは");
        assert_eq!(engine.snapshot().preedit, "フランスは");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu)
        );

        type_text(&mut engine, "い");
        assert_eq!(engine.snapshot().preedit, "フランスはい");
        assert_eq!(
            engine.evaluation_live_prefixes(),
            (None, Some(("ふらんすは", "フランスは")))
        );

        type_text(&mut engine, "ぎりす");
        assert_eq!(engine.snapshot().preedit, "フランスはイギリス");
    }

    #[test]
    fn live_conversion_does_not_delay_a_suffix_only_conversion() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "こうみょうに");
        assert_eq!(engine.snapshot().preedit, "巧妙に");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu)
        );

        type_text(&mut engine, "え");
        assert_eq!(engine.snapshot().preedit, "巧妙に得");
    }

    #[test]
    fn live_conversion_keeps_a_strong_right_word_separate_from_the_pending_bunsetsu() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(&mut engine, "へんかんせいどもへんかん");
        assert_eq!(engine.snapshot().preedit, "変換精度も変換");
        let preview = engine.live_preview.as_ref().unwrap();
        assert_eq!(
            preview
                .pending_prefix
                .as_ref()
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("へんかんせいども", "変換精度も"))
        );
        assert_eq!(
            preview
                .word_checkpoint
                .as_ref()
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("へんかんせいどもへんかん", "変換精度も変換"))
        );

        type_text(&mut engine, "せい");
        assert_eq!(engine.snapshot().preedit, "変換精度も変換せい");
        type_text(&mut engine, "ど");
        assert_eq!(engine.snapshot().preedit, "変換精度も変換精度");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.word_checkpoint.is_none())
        );
    }

    #[test]
    fn live_conversion_keeps_a_long_particle_boundary_while_the_next_word_is_incomplete() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "しゅうかんしょうねんさんでーや");
        assert_eq!(engine.snapshot().preedit, "週刊少年サンデーや");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu),
            "particle boundary should be sealable: {:?}",
            engine.live_preview
        );
        type_text(&mut engine, "しゅ");
        assert!(
            engine.snapshot().preedit.starts_with("週刊少年サンデーや"),
            "a completed particle boundary must not roll the whole marked phrase back to kana"
        );
    }

    #[test]
    fn live_conversion_advances_a_nested_boundary_after_punctuation() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(
            &mut engine,
            "だりつやしょうりすうといったぽぴゅらーなものから、へいさつだりつや",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "打率や勝利数といったポピュラーなものから、併殺打率や"
        );
        type_text(&mut engine, "ひほん");
        assert!(
            engine
                .snapshot()
                .preedit
                .starts_with("打率や勝利数といったポピュラーなものから、併殺打率や"),
            "an ambiguous suffix must not roll the completed nested bunsetsu back to kana: {:?}",
            engine.live_preview,
        );

        type_text(&mut engine, "るいだ");
        assert_eq!(
            engine.evaluation_live_prefixes().0,
            Some((
                "だりつやしょうりすうといったぽぴゅらーなものから、へいさつだりつや",
                "打率や勝利数といったポピュラーなものから、併殺打率や",
            )),
            "a later confident target must advance the existing stable prefix"
        );

        type_text(&mut engine, "り");
        assert!(
            engine
                .snapshot()
                .preedit
                .starts_with("打率や勝利数といったポピュラーなものから、併殺打率や"),
            "later ambiguity must remain confined to the active suffix"
        );
    }

    #[test]
    fn live_conversion_does_not_preserve_an_inflected_stem_as_a_nested_boundary() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(
            &mut engine,
            "これいこうのぞくへんをかくことをなんどかいらいされるも、「ごじらをころすのがかわいそうだから",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "これ以降の続編を書くことを何度か依頼されるも、「ゴジラを殺すのがかわいそうだから"
        );
    }

    #[test]
    fn live_conversion_reopens_ya_before_a_crossing_katakana_word() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "ちゅうがくせいとうじしゅうかんや");
        type_text(&mut engine, "ん");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.stable_prefix.is_none()),
            "the incomplete katakana continuation must remain reopenable: {:?}",
            engine.live_preview
        );
        type_text(&mut engine, "ぐ");
        assert!(
            engine.snapshot().preedit.ends_with("ヤング")
                && engine
                    .live_preview
                    .as_ref()
                    .is_some_and(|preview| preview.stable_prefix.is_none()),
            "the crossing word may remain provisional but must not seal the particle split: {:?}",
            engine.live_preview,
        );
        type_text(&mut engine, "じゃんぷででびゅー");
        assert_eq!(
            engine.snapshot().preedit,
            "中学生当時週刊ヤングジャンプでデビュー"
        );
    }

    #[test]
    fn live_conversion_replaces_a_false_particle_with_a_katakana_checkpoint() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "まんねるへいむ");
        assert_eq!(engine.snapshot().preedit, "マンネルヘイム");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|checkpoint| (checkpoint.reading.as_str(), checkpoint.surface.as_str())),
            Some(("まんねるへいむ", "マンネルヘイム")),
            "the crossing katakana segment must replace the false へ boundary"
        );

        type_text(&mut engine, "じ");
        assert_eq!(
            engine.snapshot().preedit,
            "マンネルヘイムじ",
            "an incomplete next word must not reopen the katakana name as kanji and kana"
        );
        type_text(&mut engine, "ゅうじ");
        assert_eq!(engine.snapshot().preedit, "マンネルヘイム従事");
    }

    #[test]
    fn live_conversion_repairs_stable_boundaries_inside_katakana_words() {
        for (reading, expected) in [
            ("ふっかつしたゔぉるでもーと", "復活したヴォルデモート"),
            (
                "うえぽんしすてむ）とよばれるがんだむ",
                "ウエポンシステム）と呼ばれるガンダム",
            ),
            (
                "かんきょうほぜん、ふぇあとれーど",
                "環境保全、フェアトレード",
            ),
            ("おもにはーどえなじー", "主にハードエナジー"),
            ("はいぱーもーるめるくす", "ハイパーモールメルクス"),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });

            type_text(&mut engine, reading);
            assert_eq!(engine.snapshot().preedit, expected, "reading: {reading}");
        }
    }

    #[test]
    fn live_conversion_repairs_stable_boundaries_inside_lexicalized_hiragana() {
        for (reading, expected) in [
            (
                "とざんぐちちかくでりようできるこうきょうこうつうきかんはなく",
                "登山口近くで利用できる公共交通機関はなく",
            ),
            (
                "てんぷくしたごうかきゃくせんからだっしゅつする」というぶたいせっていだけであり",
                "転覆した豪華客船から脱出する」という舞台設定だけであり",
            ),
            (
                "らっかなどでぐうぜんそうなっただけというものは、かくりつぶんぷであるのでぷろふぁい",
                "落下などで偶然そうなっただけというものは、確率分布であるのでプロファイ",
            ),
            (
                "だりつやしょうりすうといったぽぴゅらーなものから、へいさつだりつやひほんるいだりつなどまにあっくなものまで",
                "打率や勝利数といったポピュラーなものから、併殺打率や被本塁打率などマニアックなものまで",
            ),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });

            type_text(&mut engine, reading);
            assert_eq!(engine.snapshot().preedit, expected, "reading: {reading}");
        }
    }

    #[test]
    fn live_conversion_uses_full_lattice_context_for_fragmented_suffixes() {
        for (reading, expected, stable) in [
            (
                "くようとうをあらしたなにものか",
                "供養塔を荒らした何者か",
                ("くようとうを", "供養塔を"),
            ),
            (
                "はんしょくきはやせいかでは",
                "繁殖期は野生下では",
                ("はんしょくきは", "繁殖期は"),
            ),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });

            type_text(&mut engine, reading);
            assert_eq!(engine.snapshot().preedit, expected, "reading: {reading}");
            assert_eq!(
                engine.evaluation_live_prefixes().0,
                Some(stable),
                "the repaired target must remain active instead of being sealed early"
            );
        }
    }

    #[test]
    fn live_conversion_reopens_a_stable_boundary_inside_a_dictionary_phrase() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(
            &mut engine,
            "しかしれーすは、いつものにげにせいさいをかいた",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "しかしレースは、いつもの逃げに精彩を欠いた"
        );
        assert!(
            engine.evaluation_live_prefixes().0.is_none(),
            "the particle-looking boundary inside the phrase must remain reopenable"
        );
    }

    #[test]
    fn live_conversion_reopens_a_particle_inside_a_kanji_word() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(
            &mut engine,
            "こっこうりつだいがくはいしゅつしゃすうはごじゅっぽひゃっぽ",
        );
        assert_eq!(engine.snapshot().preedit, "国公立大学排出者数は五十歩百歩");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(
            snapshot.request_reopens_stable_prefix(&request),
            "the repaired word must keep its full-reading homophone rankable"
        );
        assert_eq!(
            request.reading,
            "こっこうりつだいがくはいしゅつしゃすうはごじゅっぽひゃっぽ"
        );
        assert!(
            request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "国公立大学輩出者数は五十歩百歩")
        );
    }

    #[test]
    fn live_conversion_reopens_a_stable_edge_inside_a_two_kana_inflected_word() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(&mut engine, "ちょうはつてきなたいどをとる");
        assert_eq!(engine.snapshot().preedit, "挑発的な態度を取る");
        assert!(
            engine.evaluation_live_prefixes().0.is_none(),
            "an ambiguous literal stem must reopen the full ranking scope"
        );

        type_text(&mut engine, "あいてにたんかをきることもめずらしくない");
        assert!(
            engine
                .live_candidate_ranking_snapshot()
                .is_some_and(|snapshot| !snapshot.evaluation_has_stable_prefix()),
            "the completed phrase must remain available to the delayed ranker"
        );
    }

    #[test]
    fn live_conversion_does_not_checkpoint_a_katakana_word_after_a_real_particle() {
        for reading in ["ぐるーぷにわか", "べいこくはんど"] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });

            type_text(&mut engine, reading);
            assert!(
                engine
                    .live_preview
                    .as_ref()
                    .is_none_or(|preview| preview.fallback_prefix.is_none()),
                "a katakana parse beginning at the particle is not a crossing word: {reading} -> {:?}",
                engine.live_preview
            );
        }
    }

    #[test]
    fn live_conversion_does_not_let_a_lattice_fallback_reopen_a_sealable_bunsetsu() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "ひさびさのきょうえんに");
        assert_eq!(engine.snapshot().preedit, "久々の共演に");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu)
        );

        type_text(&mut engine, "く");
        assert_eq!(engine.snapshot().preedit, "久々の共演にく");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.pending_prefix.as_ref())
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("ひさびさのきょうえんに", "久々の共演に"))
        );
    }

    #[test]
    fn live_conversion_keeps_a_pending_bunsetsu_at_a_punctuation_boundary() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "ひさびさのきょうえんにく");
        assert_eq!(engine.snapshot().preedit, "久々の共演にく");
        type_text(&mut engine, "、");
        assert_eq!(engine.snapshot().preedit, "久々の共演にく、");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|prefix| prefix.surface.as_str()),
            Some("久々の共演にく、")
        );
    }

    #[test]
    fn live_conversion_does_not_lock_a_particle_looking_edge_inside_a_word() {
        for (input, expected) in [
            (
                "さいあくのじたいをかいひするため",
                "最悪の事態を回避するため",
            ),
            ("をはいてたんきとうえんじん", "を履いて単気筒エンジン"),
            (
                "させるしゅほうをはじめ、しゃしんのうえからこうみょうにえをかき、いふくを",
                "させる手法を始め、写真の上から巧妙に絵を書き、衣服を",
            ),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });
            type_text(&mut engine, input);
            assert_eq!(engine.snapshot().preedit, expected, "{input}");
        }

        let mut contextual_ordinal = SlimeEngine::bundled();
        contextual_ordinal.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut contextual_ordinal, "ねつりきがくだいに");
        assert!(
            !contextual_ordinal
                .snapshot()
                .preedit
                .chars()
                .any(|character| character.is_ascii_digit()),
            "a contextual ordinal must wait for its unit"
        );
    }

    #[test]
    fn live_conversion_keeps_a_close_homophone_rankable_for_right_context() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "konokyousouha");
        assert_eq!(engine.snapshot().preedit, "この競争は");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| !preview.sealable_bunsetsu),
            "a close 競争/競走 pair must remain in the active target"
        );

        type_text(&mut engine, "handekyappudeokonaware");
        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("the complete marked phrase should remain rankable");
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(
            request.reading,
            "このきょうそうははんできゃっぷでおこなわれ"
        );
        assert!(
            (EXPLICIT_RANKING_CANDIDATE_LIMIT + 1..=LIVE_RANKING_CANDIDATE_LIMIT)
                .contains(&request.candidates.len()),
            "long delayed LIVE ranking should use the wider worker-only search"
        );

        let mut compound_particle = SlimeEngine::bundled();
        compound_particle.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut compound_particle, "しょうねんいんには");
        assert!(
            compound_particle
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu),
            "には should retain the ordinary stable-prefix threshold"
        );
    }

    #[test]
    fn live_conversion_keeps_a_long_topic_homophone_rankable() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let reading = "きこうはねんかんをつうじてひじょうにかんれいであり、なつでもひょうてんをこえることはなく";
        type_text(&mut engine, reading);
        assert_eq!(
            engine.snapshot().preedit,
            "機構は年間を通じて非常に寒冷であり、夏でも評点を超えることはなく"
        );
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.stable_prefix.is_some()),
            "the synchronous display should keep its already stable prefix"
        );
        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("the ambiguous topic must remain available to delayed LIVE ranking");
        assert_eq!(
            snapshot.candidate_ranking_request().unwrap().reading,
            reading
        );

        let mut ordinary = SlimeEngine::bundled();
        ordinary.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut ordinary, "ばすがくしろえきから");
        let ordinary_request = ordinary
            .live_candidate_ranking_snapshot()
            .and_then(|snapshot| snapshot.candidate_ranking_request())
            .expect("ordinary stable-prefix suffix should remain rankable");
        assert_eq!(ordinary_request.reading, "くしろえきから");
    }

    #[test]
    fn live_conversion_reopens_only_a_bounded_stable_prefix_repair() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        let reading = "らーめんにはじしんがあるが";
        type_text(&mut engine, reading);
        assert_eq!(engine.snapshot().preedit, "ラーメンには自身があるが");
        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("the stable homophone should remain available after debounce");
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, reading);
        assert!(
            request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "ラーメンには自信があるが")
        );
        assert!(
            snapshot.reopened_request_changes_are_bounded(&request, "ラーメンには自信があるが")
        );
        assert!(
            !snapshot.reopened_request_changes_are_bounded(&request, "ラーメンには自信が有るが")
        );

        let mut detention = SlimeEngine::bundled();
        detention.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        let reading = "また、べっけんたいほはひぎしゃをそうきにこうりゅうするために";
        let expected = "また、別件逮捕は被疑者を早期に勾留するために";
        type_text(&mut detention, reading);
        let snapshot = detention.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "そうきにこうりゅうするために");
        let expected_target = "早期に勾留するために";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == expected_target)
            .expect("the contextual detention homophone remains rankable");
        ranked.swap(0, position);
        assert_eq!(
            detention.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(expected.to_owned())])
        );
    }

    #[test]
    fn reopened_live_request_can_repair_a_target_behind_an_unchanged_prefix() {
        let mut performance = SlimeEngine::bundled();
        performance.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        performance.set_external_left_context("レッド・ツェッペリンの");
        let reading = "だいふぁんであり、ぶどうかんこうえんで";
        type_text(&mut performance, reading);
        let snapshot = performance.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, reading);
        assert_eq!(snapshot.prefix.as_ref().unwrap().surface, "大ファンで");
        let expected = "大ファンであり、武道館公演で";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == expected)
            .expect("the performance homophone remains rankable");
        ranked.swap(0, position);
        assert_eq!(
            performance.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(expected.to_owned())])
        );

        let mut museum = SlimeEngine::bundled();
        museum.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        let reading = "せつりつかんれんじぎょうとして「でんわもうのなかのみえないみゅーじあむ」がでんわもうのなかにかいせつさ";
        type_text(&mut museum, reading);
        assert_eq!(
            museum.snapshot().preedit,
            "設立関連事業として「電話網の中の見えないミュージアム」が電話網の中に解説さ"
        );
        let snapshot = museum.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, reading);
        let expected = "設立関連事業として「電話網の中の見えないミュージアム」が電話網の中に開設さ";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == expected)
            .expect("the museum homophone remains rankable");
        ranked.swap(0, position);
        assert_eq!(
            museum.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(expected.to_owned())])
        );

        let mut panel = SlimeEngine::bundled();
        panel.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        let reading =
            "はいちし、それをかくさんばんにてぱねるじょうにかくさんしてちょくせつしょうしゃ";
        type_text(&mut panel, reading);
        assert_eq!(
            panel.snapshot().preedit,
            "配置し、それを拡散版にてパネル上に拡散して直接しょうしゃ"
        );
        let snapshot = panel.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, reading);
        let suffix_request = snapshot
            .candidate_ranking_request_without_reopening()
            .expect("a rejected full repair must retain the original target scope");
        assert_eq!(suffix_request.reading, "かくさんしてちょくせつしょうしゃ");
        assert!(!snapshot.request_reopens_stable_prefix(&suffix_request));
        let expected = "配置し、それを拡散板にてパネル上に拡散して直接照射";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == expected)
            .expect("a bounded prefix repair may accompany literal target conversion");
        ranked.swap(0, position);
        assert_eq!(
            panel.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(expected.to_owned())])
        );
    }

    #[test]
    fn live_conversion_reopens_a_stable_prefix_before_sokuon() {
        for (input, expected_reading, expected_target) in [
            (
                "では、ぎいんのふたいほとっけん",
                "では、ぎいんのふたいほとっけん",
                "ふたいほとっけん",
            ),
            (
                "にこんせいがっしょう",
                "にこんせいがっしょう",
                "にこんせいがっしょう",
            ),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });

            type_text(&mut engine, input);
            let snapshot = engine
                .live_candidate_ranking_snapshot()
                .expect("the reopened full reading should remain rankable");
            assert_eq!(snapshot.resolved_reading(), expected_reading);
            let target = snapshot.candidate_ranking_request().unwrap().reading;
            assert!(
                target == expected_target || target == expected_reading,
                "the sokuon compound must remain whole in the active ranking target: {target}"
            );
        }
    }

    #[test]
    fn live_conversion_keeps_to_boundary_reopenable_for_a_katakana_word() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        let prefix = "、げーむきゅーぶへのいしょくをみすえてとらい";
        type_text(&mut engine, prefix);
        assert_eq!(
            engine.snapshot().preedit,
            "、ゲームキューブへの移植を見据えてトライ"
        );
        type_text(&mut engine, "ふ");
        assert_eq!(
            engine.snapshot().preedit,
            "、ゲームキューブへの移植を見据えてトライふ",
            "an ambiguous partial suffix must preserve the cross-boundary katakana prefix"
        );
        type_text(&mut engine, "ぉーすきばんがさいようされた");
        assert_eq!(
            engine.snapshot().preedit,
            "、ゲームキューブへの移植を見据えてトライフォース基盤が採用された"
        );

        let mut ordinary_particle = SlimeEngine::bundled();
        ordinary_particle.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut ordinary_particle, "おんがくとらいぶをたのしむ");
        assert_eq!(
            ordinary_particle.snapshot().preedit,
            "音楽とライブを楽しむ",
            "keeping と reopenable must not merge an ordinary particle boundary"
        );
    }

    #[test]
    fn live_conversion_uses_punctuation_prefix_only_as_an_ambiguous_display_fallback() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihongo,");
        let completed_clause = engine.snapshot().preedit;
        assert_eq!(completed_clause, "日本語、");

        type_text(&mut engine, "sou");
        assert_eq!(engine.snapshot().preedit, "日本語、そう");
        let preview = engine.live_preview.as_ref().expect("fallback preview");
        assert!(preview.stable_prefix.is_none());
        assert_eq!(
            preview
                .fallback_prefix
                .as_ref()
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("にほんご、", "日本語、"))
        );

        type_text(&mut engine, "shimasu");
        assert_eq!(engine.snapshot().preedit, "日本語、そうします");
    }

    #[test]
    fn live_conversion_keeps_a_middle_dot_checkpoint() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "しんにほんせいてつ・");
        assert_eq!(engine.snapshot().preedit, "新日本製鐵・");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|checkpoint| checkpoint.surface.as_str()),
            Some("新日本製鐵・")
        );

        type_text(&mut engine, "すみ");
        assert_eq!(
            engine.snapshot().preedit,
            "新日本製鐵・すみ",
            "an incomplete name after a middle dot must not reopen the completed name"
        );
        type_text(&mut engine, "ともきんぞくこうぎょう");
        assert_eq!(engine.snapshot().preedit, "新日本製鐵・住友金属工業");
    }

    #[test]
    fn live_conversion_keeps_an_opening_quote_checkpoint() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(
            &mut engine,
            "ほきょうしつつもげんざいけいたいをほぜんするべきという「",
        );
        let checkpoint = "補強しつつも現在携帯を保全するべきという「";
        assert_eq!(engine.snapshot().preedit, checkpoint);
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|prefix| prefix.surface.as_str()),
            Some(checkpoint)
        );

        type_text(&mut engine, "とうきょ");
        assert_eq!(
            engine.snapshot().preedit,
            "補強しつつも現在携帯を保全するべきという「とうきょ",
            "an incomplete quotation must not reopen text before its opening quote"
        );
        type_text(&mut engine, "うえき");
        assert_eq!(
            engine.snapshot().preedit,
            "補強しつつも現在携帯を保全するべきという「東京駅"
        );
    }

    #[test]
    fn live_conversion_retains_a_corrected_punctuation_checkpoint() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(
            &mut engine,
            "こうえんはきゅうでんのこうほうふぁさーどにはじまり、",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "公園は宮殿の広報ファサードにはじまり、"
        );

        type_text(&mut engine, "じんこうのふんすいとたきのあるながいこ");
        assert_eq!(
            engine.snapshot().preedit,
            "公園は宮殿の広報ファサードに始まり、人口の噴水と滝のある長い子"
        );
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some((
                "こうえんはきゅうでんのこうほうふぁさーどにはじまり、",
                "公園は宮殿の広報ファサードに始まり、"
            )),
            "a confident correction may update the clause, but must not erase its checkpoint"
        );

        type_text(&mut engine, "み");
        assert_eq!(
            engine.snapshot().preedit,
            "公園は宮殿の広報ファサードに始まり、人口の噴水と滝のある長い子み",
            "a one-key ambiguity must not roll the corrected clause back to kana"
        );
    }

    #[test]
    fn live_conversion_keeps_a_long_supported_prefix_during_ambiguity() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });

        type_text(&mut engine, "ゆうぎだいせんたく");
        assert_eq!(engine.snapshot().preedit, "遊技台選択");

        type_text(&mut engine, "じ");
        assert_eq!(
            engine.snapshot().preedit,
            "遊技台選択じ",
            "a close suffix homophone must not roll a long supported prefix back to kana"
        );
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("ゆうぎだいせんたく", "遊技台選択"))
        );

        type_text(&mut engine, "のはんだんざいりょうとして");
        assert!(
            engine.snapshot().preedit.starts_with("遊技台選択"),
            "continued ambiguity must preserve the already supported prefix"
        );

        let mut close_path = SlimeEngine::bundled();
        close_path.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut close_path, "ないかく");
        assert_eq!(close_path.snapshot().preedit, "内閣");
        type_text(&mut close_path, "し");
        assert_eq!(close_path.snapshot().preedit, "内閣し");
        assert!(
            close_path.live_candidate_ranking_snapshot().is_none(),
            "a close-path display fallback must not trigger whole-reading ranking"
        );
    }

    #[test]
    fn ambiguous_continuity_requires_a_completed_converted_edge() {
        let dictionary = bundled_dictionary(0, &UserData::default());
        let preview = |surface: &str| LivePreview {
            reading: "いるのかをついきゅうし、また、ゆうせいせだ".to_owned(),
            surface: surface.to_owned(),
            reliable: false,
            prefix_fragile: false,
            sealable_bunsetsu: false,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: None,
            checkpoint_kind: LiveCheckpointKind::None,
        };
        assert!(
            ambiguous_long_prefix_literal_extension(
                &dictionary,
                &preview("いるのかを追求し、また、優勢せだ"),
                "いるのかをついきゅうし、また、ゆうせいせだい",
                Some("いるのかを追求し、また、優勢世代"),
            )
            .is_none(),
            "an incomplete hiragana tail must remain free to be reanalysed"
        );

        let supported = LivePreview {
            reading: "ゆうぎだいせんたく".to_owned(),
            surface: "遊技台選択".to_owned(),
            ..preview("遊技台選択")
        };
        assert!(
            ambiguous_long_prefix_literal_extension(
                &dictionary,
                &supported,
                "ゆうぎだいせんたくじ",
                Some("遊技台選択時"),
            )
            .is_some_and(|(_, _, close_path)| !close_path)
        );

        let four_kana = LivePreview {
            reading: "ないかく".to_owned(),
            surface: "内閣".to_owned(),
            ..preview("内閣")
        };
        assert!(
            ambiguous_long_prefix_literal_extension(
                &dictionary,
                &four_kana,
                "ないかくし",
                Some("ない隠し"),
            )
            .is_some_and(|(_, _, close_path)| close_path),
            "a close bounded path can support a four-kana completed word"
        );

        let punctuated_three_kana = LivePreview {
            reading: "、ふぇい".to_owned(),
            surface: "、フェイ".to_owned(),
            ..preview("、フェイ")
        };
        assert!(
            ambiguous_long_prefix_literal_extension(
                &dictionary,
                &punctuated_three_kana,
                "、ふぇいざ",
                Some("、フェイザー"),
            )
            .is_none(),
            "punctuation must not count toward the four-kana continuity floor"
        );
    }

    #[test]
    fn live_conversion_keeps_a_punctuation_checkpoint_inside_a_stable_target() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(
            &mut engine,
            "をちかってじょうまえをかけるばしょがよういされており、",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "を誓って錠前をかける場所が用意されており、"
        );
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.fallback_prefix.as_ref())
                .map(|checkpoint| checkpoint.surface.as_str()),
            Some("を誓って錠前をかける場所が用意されており、")
        );

        type_text(&mut engine, "おおいがわてつど");
        assert_eq!(
            engine.snapshot().preedit,
            "を誓って錠前をかける場所が用意されており、大井川てつど",
            "an incomplete next word must not erase the completed clause"
        );

        type_text(&mut engine, "う");
        assert_eq!(
            engine.snapshot().preedit,
            "を誓って錠前をかける場所が用意されており、大井川鉄道"
        );
    }

    #[test]
    fn live_conversion_keeps_a_deeper_lattice_backed_literal_extension() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "toukyou");
        assert_eq!(engine.snapshot().preedit, "東京");

        type_text(&mut engine, "ko");
        assert_eq!(engine.snapshot().preedit, "東京こ");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| { !preview.reliable && preview.stable_prefix.is_none() })
        );

        type_text(&mut engine, "kusai");
        assert_eq!(engine.snapshot().preedit, "東京国際");
    }

    #[test]
    fn live_conversion_protects_a_sealable_bunsetsu_after_punctuation() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "toha,seitouga");
        assert_eq!(engine.snapshot().preedit, "とは、政党が");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_some_and(|preview| preview.sealable_bunsetsu)
        );

        type_text(&mut engine, "ha");
        assert_eq!(engine.snapshot().preedit, "とは、政党がは");
        assert_eq!(
            engine
                .live_preview
                .as_ref()
                .and_then(|preview| preview.pending_prefix.as_ref())
                .map(|prefix| (prefix.reading.as_str(), prefix.surface.as_str())),
            Some(("とは、せいとうが", "とは、政党が"))
        );
    }

    #[test]
    fn leading_punctuation_never_becomes_a_pending_live_prefix() {
        let preview = super::LivePreview {
            reading: "、ふぇいざーや".to_owned(),
            surface: "、フェイザーや".to_owned(),
            reliable: true,
            prefix_fragile: false,
            sealable_bunsetsu: true,
            stable_prefix: None,
            pending_prefix: None,
            word_checkpoint: None,
            fallback_prefix: None,
            checkpoint_kind: super::LiveCheckpointKind::None,
        };

        assert!(super::eligible_live_pending_prefix(&preview).is_none());
    }

    #[test]
    fn backspace_reopens_a_stable_live_prefix_when_the_reading_reaches_it() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkangatsu");
        assert_eq!(engine.snapshot().preedit, "変換がつ");

        engine.handle(InputEvent::Backspace);
        assert_eq!(engine.snapshot().preedit, "変換が");
        engine.handle(InputEvent::Backspace);
        assert_eq!(engine.snapshot().preedit, "変換");
        assert!(
            engine
                .live_preview
                .as_ref()
                .is_none_or(|preview| preview.stable_prefix.is_none()),
            "editing into a stable prefix must return it to the live target"
        );
    }

    #[test]
    fn live_conversion_handles_terms_with_particles_and_inflections() {
        let cases = [
            ("henkande", "変換で"),
            ("de-tahenkande", "データ変換で"),
            ("nihongonyuuryokude", "日本語入力で"),
            ("kouhosentakude", "候補選択で"),
            ("pafo-mansuwotakameru", "パフォーマンスを高める"),
        ];

        for (input, expected) in cases {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });
            type_text(&mut engine, input);
            assert_eq!(engine.snapshot().preedit, expected, "{input}");
        }
    }

    #[test]
    fn live_conversion_defers_implicit_numbers_before_katakana_words() {
        for input in [
            "sanfan",
            "niwikipedia",
            "shinniho",
            "shihoushounitsu",
            "dainihou",
            "nikoniko",
            "senkoukai",
            "gojira",
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });
            type_text(&mut engine, input);
            assert!(
                !engine
                    .snapshot()
                    .preedit
                    .chars()
                    .any(|character| character.is_ascii_digit()),
                "{input} must not retain an implicit number"
            );
        }

        for (input, expected) in [
            ("ichi", "1"),
            ("daini", "第2"),
            ("ichinichi", "1日"),
            ("ichiiga", "1位が"),
            ("2fan", "２ファン"),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: false,
                history_learning: false,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });
            type_text(&mut engine, input);
            assert_eq!(engine.snapshot().preedit, expected, "{input}");
        }
    }

    #[test]
    fn live_conversion_never_combines_a_stale_preview_with_a_new_suffix() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        // Spell the small tsu explicitly so `もっ` is evaluated before the
        // following `と`, matching insertion/editing paths that expose the
        // stale-prefix bug.
        type_text(&mut engine, "moxtuto");
        assert_eq!(engine.snapshot().preedit, "もっと");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("もっと".to_owned())));

        type_text(&mut engine, "kouiuno");
        assert_eq!(engine.snapshot().preedit, "こういうの");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("こういうの".to_owned())));

        type_text(&mut engine, "ichigachigau");
        assert_eq!(engine.snapshot().preedit, "いちがちがう");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("いちがちがう".to_owned())));
    }

    #[test]
    fn escape_suppresses_live_conversion_until_composition_ends() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihongo");
        assert_eq!(engine.snapshot().preedit, "日本語");
        engine.handle(InputEvent::Escape);
        assert_eq!(engine.snapshot().preedit, "にほんご");

        type_text(&mut engine, "wo");
        assert_eq!(engine.snapshot().preedit, "にほんごを");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("にほんごを".to_owned())));
    }

    #[test]
    fn implicit_live_conversion_is_not_learned() {
        let directory = test_directory("implicit-live");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihongo");
        assert_eq!(engine.snapshot().preedit, "日本語");
        engine.handle(InputEvent::Enter);

        assert!(!directory.join("history.tsv").exists());
        assert!(engine.session_history.previous_commit().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bundled_public_language_correction_resolves_conversion_precision() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換精度");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "変換精度");
        assert_eq!(engine.snapshot().candidates[0], "変換精度");
    }

    #[test]
    fn learned_history_cannot_bypass_live_confidence() {
        let directory = test_directory("live-history-confidence");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nそうしま\t総島\t10\t20\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "soushima");
        assert_eq!(engine.snapshot().preedit, "そうしま");

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "総島");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_exact_phrase_correction_personalizes_live_conversion() {
        for (count, expected) in [(1, "変換制度"), (2, "変換精度")] {
            let directory = test_directory(&format!("live-phrase-history-{count}"));
            fs::write(
                directory.join("history.tsv"),
                format!("# slime-history-v1\nへんかんせいど\t変換精度\t{count}\t20\n"),
            )
            .unwrap();
            let mut engine = ambiguous_precision_engine(UserData::load(&directory));
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: true,
                history_learning: true,
                dictionary_packs: 0,
                private_mode: false,
                date_format_mask: ALL_DATE_FORMATS,
            });

            type_text(&mut engine, "henkanseido");
            assert_eq!(engine.snapshot().preedit, expected, "history count {count}");
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn explicit_phrase_correction_personalizes_live_conversion_immediately_in_session() {
        let directory = test_directory("session-live-phrase-correction");
        let mut engine = ambiguous_precision_engine(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換制度");
        engine.handle(InputEvent::Space);
        let corrected_index = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == "変換精度")
            .expect("the correction remains a normal dictionary candidate");
        engine.handle(InputEvent::SelectCandidate(
            u32::try_from(corrected_index).unwrap(),
        ));
        engine.handle(InputEvent::Enter);

        // A caret or document-context boundary does not erase an explicit
        // spelling choice made in this engine session.
        engine.reset_context();
        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換精度");

        // The persisted one-off entry stays below the existing two-use gate;
        // a new engine does not globally revive a context-specific choice.
        let mut reloaded = ambiguous_precision_engine(UserData::load(&directory));
        reloaded.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut reloaded, "henkanseido");
        assert_eq!(reloaded.snapshot().preedit, "変換制度");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_phrase_selection_does_not_seed_session_live_conversion() {
        let directory = test_directory("private-session-live-phrase-correction");
        let mut engine = ambiguous_precision_engine(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: true,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkanseido");
        engine.handle(InputEvent::Space);
        let corrected_index = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == "変換精度")
            .unwrap();
        engine.handle(InputEvent::SelectCandidate(
            u32::try_from(corrected_index).unwrap(),
        ));
        engine.handle(InputEvent::Enter);
        engine.set_preferences(EnginePreferences {
            private_mode: false,
            ..engine.preferences
        });

        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換制度");
        assert!(!directory.join("history.tsv").exists());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recent_live_selections_are_bounded_and_replace_the_same_reading() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        for index in 0..=MAX_RECENT_LIVE_SELECTIONS {
            engine.record_recent_live_selection(
                &format!("てすとよみ{index}"),
                &format!("表記{index}"),
            );
        }

        assert_eq!(
            engine.recent_live_selections.len(),
            MAX_RECENT_LIVE_SELECTIONS
        );
        assert!(
            engine
                .recent_live_selection_surface("てすとよみ0")
                .is_none()
        );
        assert_eq!(
            engine.recent_live_selection_surface("てすとよみ64"),
            Some("表記64")
        );

        engine.record_recent_live_selection("てすとよみ64", "更新表記");
        assert_eq!(
            engine.recent_live_selections.len(),
            MAX_RECENT_LIVE_SELECTIONS
        );
        assert_eq!(
            engine.recent_live_selection_surface("てすとよみ64"),
            Some("更新表記")
        );
    }

    #[test]
    fn repeated_exact_phrase_can_resolve_live_ambiguity() {
        let directory = test_directory("live-phrase-resolves-ambiguity");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nらいぶへんかんの\tライブ変換の\t2\t20\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "raibuhenkannno");
        assert_eq!(engine.snapshot().preedit, "ライブ変換の");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_phrase_history_cannot_invent_a_live_surface() {
        let directory = test_directory("live-phrase-must-be-recalled");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nへんかんせいど\t架空の変換\t20\t20\n",
        )
        .unwrap();
        let mut engine = ambiguous_precision_engine(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換制度");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_phrase_history_can_select_a_deeper_normal_candidate() {
        let directory = test_directory("live-phrase-deeper-candidate");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nあいうえお\t第五候補\t2\t20\n",
        )
        .unwrap();
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あいうえお", "第一候補", 0),
            DictionaryEntry::new("あいうえお", "第二候補", 100),
            DictionaryEntry::new("あいうえお", "第三候補", 200),
            DictionaryEntry::new("あいうえお", "第四候補", 300),
            DictionaryEntry::new("あいうえお", "第五候補", 400),
        ]);
        let mut engine = SlimeEngine::with_user_data(dictionary, UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "あいうえお");
        assert_eq!(engine.snapshot().preedit, "第五候補");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_mode_ignores_repeated_live_phrase_history() {
        let directory = test_directory("private-live-phrase-history");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nへんかんせいど\t変換精度\t20\t20\n",
        )
        .unwrap();
        let mut engine = ambiguous_precision_engine(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: true,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換制度");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enter_commits_exactly_what_the_live_preview_shows() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        // The trailing n is still pending romaji; the preview must already
        // account for it so Enter cannot commit something else.
        type_text(&mut engine, "hon");
        let displayed = engine.snapshot().preedit;
        let actions = engine.handle(InputEvent::Enter);
        assert!(
            actions.contains(&SlimeAction::Commit(displayed.clone())),
            "displayed {displayed:?} but committed {actions:?}"
        );
    }

    #[test]
    fn single_kana_conversion_offers_the_literal_hiragana() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "mi");
        engine.handle(InputEvent::Space);

        assert!(
            engine.snapshot().candidates.contains(&"み".to_owned()),
            "{:?}",
            engine.snapshot().candidates
        );
    }

    #[test]
    fn escape_restores_reading_before_clearing_live_conversion() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut engine, "nihongo");

        engine.handle(InputEvent::Escape);
        assert_eq!(engine.snapshot().preedit, "にほんご");

        engine.handle(InputEvent::Escape);
        assert_eq!(engine.snapshot().preedit, "");
    }

    #[test]
    fn user_dictionary_candidate_is_ranked_first() {
        let directory = test_directory("dictionary");
        fs::write(
            directory.join("user_dictionary.tsv"),
            "# slime-user-dictionary-v1\nほげ\tHOGE\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        type_text(&mut engine, "hoge");
        let actions = engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "HOGE");
        assert_eq!(
            shown_candidate_details(&actions)[0].annotation,
            CandidateAnnotation::UserDictionary
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn candidate_metadata_separates_generated_and_corrected_values() {
        let mut number_engine = SlimeEngine::bundled();
        type_text(&mut number_engine, "senkyuuhyakukyuujuuichi");
        let number_actions = number_engine.handle(InputEvent::Space);
        let number_details = shown_candidate_details(&number_actions);
        assert!(number_details.iter().any(|detail| {
            detail.value == "1991" && detail.annotation == CandidateAnnotation::Number
        }));

        let mut date_engine = SlimeEngine::bundled();
        type_text(&mut date_engine, "kyou");
        let date_actions = date_engine.handle(InputEvent::Space);
        assert!(
            shown_candidate_details(&date_actions)
                .iter()
                .any(|detail| detail.annotation == CandidateAnnotation::DateTime)
        );

        let dictionary = Dictionary::new(vec![DictionaryEntry::new("にほん", "日本", 10)]);
        let mut correction_engine = SlimeEngine::new(dictionary);
        type_text(&mut correction_engine, "nihpn");
        let correction_actions = correction_engine.handle(InputEvent::Space);
        let correction = shown_candidate_details(&correction_actions)
            .iter()
            .find(|detail| detail.value == "日本")
            .expect("corrected candidate");
        assert_eq!(correction.annotation, CandidateAnnotation::Correction);
        assert_eq!(correction.detail.as_deref(), Some("にほん"));
    }

    #[test]
    fn domain_dictionary_can_be_enabled_independently() {
        let user_data = UserData::default();
        let basic = bundled_dictionary(0, &user_data);
        let technology = bundled_dictionary(TECHNOLOGY_DICTIONARY, &user_data);

        assert!(
            !basic
                .candidates("すうぃふとゆーあい")
                .iter()
                .any(|candidate| { candidate.surface == "SwiftUI" })
        );
        assert_eq!(
            technology.candidates("すうぃふとゆーあい")[0].surface,
            "SwiftUI"
        );
    }

    #[test]
    fn installed_dictionary_pack_is_loaded_from_user_data_directory() {
        let directory = test_directory("installed-pack");
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        fs::write(
            pack_directory.join("sample.slime-dict"),
            "\
# slime-dictionary-pack-v1
# id: sample-general
# name: 一般語彙サンプル
# version: 2026.07.1
# license: Example-Test-Only
てすとようご\t試験用語
こまわり\t専門小回り\t6000
",
        )
        .unwrap();

        let engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        assert_eq!(
            engine.dictionary.candidates("てすとようご")[0].surface,
            "試験用語"
        );
        assert_eq!(
            engine.dictionary.candidates("こまわり")[0].surface,
            "小回り"
        );
        assert!(
            engine
                .dictionary
                .candidates("こまわり")
                .iter()
                .any(|candidate| candidate.surface == "専門小回り")
        );
        let infos: Vec<_> = engine.installed_dictionary_packs().collect();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "sample-general");
        assert_eq!(
            engine
                .installed_dictionary_pack_words("sample-general")
                .unwrap(),
            [
                DictionaryPackWord {
                    reading: "てすとようご".to_owned(),
                    surface: "試験用語".to_owned(),
                },
                DictionaryPackWord {
                    reading: "こまわり".to_owned(),
                    surface: "専門小回り".to_owned(),
                },
            ]
        );
        assert!(engine.dictionary_pack_load_errors().is_empty());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installed_context_rules_rank_existing_candidates_without_learning() {
        let directory = test_directory("installed-context-pack");
        write_context_pack(&directory);

        let mut baseline = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        type_text(&mut baseline, "kanji");
        baseline.handle(InputEvent::Space);
        assert_ne!(baseline.snapshot().preedit, "漢字");

        let mut contextual = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        convert_and_commit(&mut contextual, "nagaibunshou", "これは長い文章");
        type_text(&mut contextual, "kanji");
        let contextual_actions = contextual.handle(InputEvent::Space);
        assert_eq!(contextual.snapshot().preedit, "漢字");
        assert_eq!(
            shown_candidate_details(&contextual_actions)
                .iter()
                .find(|detail| detail.value == "漢字")
                .expect("context candidate")
                .annotation,
            CandidateAnnotation::Context
        );
        assert!(
            !contextual
                .snapshot()
                .candidates
                .contains(&"架空候補".to_owned())
        );
        assert!(!directory.join("history.tsv").exists());

        // The first Escape only closes the candidate window.
        contextual.handle(InputEvent::Escape);
        contextual.handle(InputEvent::Escape);
        contextual.reset_context();
        type_text(&mut contextual, "kanji");
        contextual.handle(InputEvent::Space);
        assert_eq!(contextual.snapshot().preedit, baseline.snapshot().preedit);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn context_annotations_follow_navigation_left_context_and_reload() {
        fn kanji_annotation(actions: &[SlimeAction]) -> CandidateAnnotation {
            shown_candidate_details(actions)
                .iter()
                .find(|detail| detail.value == "漢字")
                .expect("漢字 candidate")
                .annotation
        }

        let directory = test_directory("context-annotation-cache");
        write_context_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        engine.set_external_left_context("これは長い文章");
        type_text(&mut engine, "kanji");
        let actions = engine.handle(InputEvent::Space);
        assert_eq!(kanji_annotation(&actions), CandidateAnnotation::Context);
        let actions = engine.handle(InputEvent::NextCandidate);
        assert_eq!(kanji_annotation(&actions), CandidateAnnotation::Context);

        // A different left context must not reuse the previous labels.
        cancel_composition(&mut engine);
        engine.reset_context();
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        let actions = engine.handle(InputEvent::NextCandidate);
        assert_ne!(kanji_annotation(&actions), CandidateAnnotation::Context);

        // Replacing the pack must drop cached labels even for the same key.
        cancel_composition(&mut engine);
        engine.set_external_left_context("これは長い文章");
        type_text(&mut engine, "kanji");
        let actions = engine.handle(InputEvent::Space);
        assert_eq!(kanji_annotation(&actions), CandidateAnnotation::Context);
        cancel_composition(&mut engine);
        write_context_pack_with_rules(&directory, "文章\tかんじ\t感じ\t0\n");
        engine.reload_user_data();
        engine.set_external_left_context("これは長い文章");
        type_text(&mut engine, "kanji");
        let actions = engine.handle(InputEvent::Space);
        assert_ne!(kanji_annotation(&actions), CandidateAnnotation::Context);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installed_context_rules_can_resolve_live_conversion_without_neural_override() {
        let directory = test_directory("installed-live-context-pack");
        write_context_pack(&directory);

        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("これは長い文章");
        type_text(&mut engine, "kanji");

        assert_eq!(engine.snapshot().preedit, "漢字");
        assert!(
            engine.live_candidate_ranking_snapshot().is_none(),
            "a high-confidence installed context rule must not be replaced asynchronously"
        );
        assert!(!directory.join("history.tsv").exists());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_context_query_does_not_mutate_session_context() {
        let directory = test_directory("explicit-context-query");
        write_context_pack(&directory);
        let engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        assert_eq!(
            engine.conversion_candidates_with_left_context("これは長い文章", "かんじ")[0],
            "漢字"
        );
        assert_ne!(engine.conversion_candidates("かんじ")[0], "漢字");
        assert!(!directory.join("history.tsv").exists());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_document_context_ranks_without_becoming_learned_history() {
        let directory = test_directory("external-document-context");
        write_context_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        engine.set_external_left_context("これは長い文章");
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "漢字");
        engine.handle(InputEvent::Enter);

        assert!(!directory.join("history.tsv").exists());
        engine.reset_context();
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_ne!(engine.snapshot().preedit, "漢字");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_document_context_reuses_a_visible_dictionary_surface() {
        let directory = test_directory("external-document-surface-repeat");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        assert_eq!(engine.conversion_candidates("あさの")[0], "朝の");
        engine.set_external_left_context("同社では浅野木材工業の");
        type_text(&mut engine, "asano");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "浅野");
        engine.handle(InputEvent::Enter);

        assert!(!directory.join("history.tsv").exists());
        engine.reset_context();
        assert_eq!(engine.conversion_candidates("あさの")[0], "朝の");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_document_context_reuses_repeated_local_context_history() {
        let directory = test_directory("external-adaptive-context");
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        for _ in 0..2 {
            engine.reset_context();
            convert_and_commit(&mut engine, "heya", "部屋");
            convert_and_commit(&mut engine, "shoumei", "照明");
            engine.reset_context();
            convert_and_commit(&mut engine, "bunshou", "文章");
            convert_and_commit(&mut engine, "shoumei", "証明");
        }
        let context_before = fs::read(directory.join("context_history.tsv")).unwrap();

        let mut baseline = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        baseline.set_preferences(preferences);
        type_text(&mut baseline, "shoumei");
        baseline.handle(InputEvent::Space);
        assert_eq!(baseline.snapshot().preedit, "証明");

        let mut room = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        room.set_preferences(preferences);
        room.set_external_left_context("既存文書の部屋");
        type_text(&mut room, "shoumei");
        let actions = room.handle(InputEvent::Space);
        assert_eq!(room.snapshot().preedit, "照明");
        assert_eq!(
            shown_candidate_details(&actions)[0].annotation,
            CandidateAnnotation::History
        );
        room.handle(InputEvent::Enter);
        assert_eq!(
            fs::read(directory.join("context_history.tsv")).unwrap(),
            context_before,
            "external document text must not be persisted as a learned edge"
        );

        let mut document = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        document.set_preferences(preferences);
        document.set_external_left_context("既存文書の文章");
        type_text(&mut document, "shoumei");
        document.handle(InputEvent::Space);
        assert_eq!(document.snapshot().preedit, "証明");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_mode_discards_external_document_context() {
        let directory = test_directory("private-external-document-context");
        write_context_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            private_mode: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("これは長い文章");
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_ne!(engine.snapshot().preedit, "漢字");
        assert_eq!(
            engine.conversion_candidates_with_left_context("既存文書の浅野", "あさの")[0],
            "朝の"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_mode_ignores_external_adaptive_context_history() {
        let directory = test_directory("private-external-adaptive-context");
        fs::write(
            directory.join("context_history.tsv"),
            "# slime-context-history-v1\nへや\t部屋\tしょうめい\t照明\t10\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            private_mode: true,
            ..EnginePreferences::default()
        });

        assert_eq!(
            engine.conversion_candidates_with_left_context("既存文書の部屋", "しょうめい"),
            engine.conversion_candidates("しょうめい")
        );
        engine.set_external_left_context("既存文書の部屋");
        type_text(&mut engine, "shoumei");
        engine.handle(InputEvent::Space);
        assert_ne!(engine.snapshot().preedit, "照明");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn context_only_pack_ranks_bundled_candidates_without_a_word_layer() {
        let directory = test_directory("context-only-pack");
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let payload = "# context-rules\n文章\tかんじ\t漢字\t0\n";
        let digest = lower_hex(&Sha256::digest(payload.as_bytes()));
        fs::write(
            pack_directory.join("sample-context-only.slime-dict"),
            format!(
                "# slime-dictionary-pack-v3\n\
                 # id: sample-context-only\n\
                 # name: 文脈のみのサンプル\n\
                 # version: 2026.08.1\n\
                 # license: Example-Test-Only\n\
                 # minimum-slime-version: 0.1.0\n\
                 # published-at: 2026-08-08\n\
                 # provenance: fixture/generated/sample-context-only\n\
                 # payload-sha256: {digest}\n\
                 # entries\n\
                 {payload}"
            ),
        )
        .unwrap();

        let engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        let info = engine.installed_dictionary_packs().next().unwrap();
        assert_eq!(info.entry_count, 0);
        assert_eq!(info.context_rule_count, 1);
        assert_eq!(
            engine.conversion_candidates_with_left_context("文章", "かんじ")[0],
            "漢字"
        );
        assert!(engine.dictionary_pack_load_errors().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn context_rules_protect_candidates_added_by_long_explicit_search() {
        let directory = test_directory("long-context-candidate-protection");
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let reading = "きかんしゃからおくられたこうあつこうねつのじょうきをもちいるきゅうしゅうしきれいきゃくほうしきであった";
        let surface = "機関車から送られた高圧高熱の蒸気を用いる吸収式冷却方式であった";
        let context = "検証用文脈";
        let payload = format!("# context-rules\n{context}\t{reading}\t{surface}\t0\n");
        let digest = lower_hex(&Sha256::digest(payload.as_bytes()));
        fs::write(pack_directory.join("long-context.slime-dict"), format!(
            "# slime-dictionary-pack-v3\n# id: long-context\n# name: Long context test\n# version: 2026.09.1\n# license: Example-Test-Only\n# minimum-slime-version: 0.1.0\n# published-at: 2026-09-09\n# provenance: fixture/generated/long-context\n# payload-sha256: {digest}\n# entries\n{payload}"
        )).unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        assert!(engine.dictionary_pack_load_errors().is_empty());
        assert!(
            !engine
                .dictionary
                .candidates_with_context_limit(reading, context, 10)
                .iter()
                .any(|candidate| candidate.surface == surface)
        );
        engine.set_external_left_context(context);
        type_text(&mut engine, reading);
        let actions = engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, surface);
        assert_eq!(
            shown_candidate_details(&actions)
                .iter()
                .find(|detail| detail.value == surface)
                .unwrap()
                .annotation,
            CandidateAnnotation::Context
        );
        assert!(
            !engine
                .candidate_ranking_request()
                .unwrap()
                .candidates
                .iter()
                .any(|candidate| candidate.surface == surface)
        );
        let mut reordered: Vec<_> = engine
            .candidate_ranking_request()
            .unwrap()
            .candidates
            .into_iter()
            .map(|candidate| candidate.surface)
            .collect();
        reordered.reverse();
        assert!(engine.apply_candidate_ranking(&reordered).is_some());
        assert_eq!(engine.snapshot().preedit, surface);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installed_context_rules_rank_an_aligned_segment_inside_a_long_candidate() {
        let directory = test_directory("context-rule-inside-long-candidate");
        write_context_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        let candidates = engine.conversion_candidates("ながいぶんしょうかんじ");
        assert_eq!(candidates[0], "これは長い文章漢字");
        type_text(&mut engine, "nagaibunshoukanji");
        let actions = engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "これは長い文章漢字");
        assert_eq!(
            shown_candidate_details(&actions)[0].annotation,
            CandidateAnnotation::Context
        );
        assert!(engine.candidate_ranking_request().is_none_or(|request| {
            request
                .candidates
                .iter()
                .all(|candidate| candidate.surface != "これは長い文章漢字")
        }));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installed_context_rules_are_disabled_in_private_mode() {
        let directory = test_directory("private-installed-context-pack");
        write_context_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        convert_and_commit(&mut engine, "bunshou", "文章");
        engine.set_preferences(EnginePreferences {
            private_mode: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_ne!(engine.snapshot().preedit, "漢字");
        assert!(!directory.join("history.tsv").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn signed_pack_policy_survives_reload_and_rejects_tampering() {
        let directory = test_directory("signed-context-pack-reload");
        write_context_pack(&directory);
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        sign_context_pack(&directory, "fixture-2026-a", &signing_key);
        let key = DictionaryPackVerificationKey::new(
            "fixture-2026-a",
            signing_key.verifying_key().to_bytes(),
        )
        .unwrap();
        let trust = DictionaryPackTrust::signed_only_with_version_floors(
            vec![key],
            vec![DictionaryPackVersionFloor::new("sample-context", "2026.08.1").unwrap()],
        )
        .unwrap();
        let mut engine =
            SlimeEngine::bundled_with_user_data_and_pack_trust(UserData::load(&directory), trust);

        assert_eq!(
            engine.conversion_candidates_with_left_context("文章", "かんじ")[0],
            "漢字"
        );
        let pack_path = directory
            .join("dictionary-packs")
            .join("sample-context.slime-dict");
        let mut tampered = fs::read_to_string(&pack_path).unwrap();
        tampered.push('\n');
        fs::write(pack_path, tampered).unwrap();

        engine.reload_user_data();
        assert_eq!(engine.dictionary_pack_load_errors().len(), 1);
        assert_eq!(
            engine.dictionary_pack_load_errors()[0].message,
            "dictionary pack signature is invalid"
        );
        assert_ne!(
            engine.conversion_candidates_with_left_context("文章", "かんじ")[0],
            "漢字"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installed_context_rules_do_not_override_user_history() {
        let directory = test_directory("history-before-installed-context");
        write_context_pack(&directory);
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t感じ\t5\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            history_completion: true,
            ..EnginePreferences::default()
        });
        convert_and_commit(&mut engine, "bunshou", "文章");
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "感じ");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn domain_dictionary_must_pass_vocabulary() {
        let user_data = UserData::default();
        let dictionary = bundled_dictionary(ALL_DOMAIN_DICTIONARIES, &user_data);

        for (reading, surface) in [
            ("すうぃふとゆーあい", "SwiftUI"),
            ("たいぷすくりぷと", "TypeScript"),
            ("くーばねてす", "Kubernetes"),
            ("えるえるえむ", "LLM"),
            ("ぎっとはぶあくしょんず", "GitHub Actions"),
            ("おーぷんあいでぃーこねくと", "OpenID Connect"),
            ("うぇぶあせんぶり", "WebAssembly"),
            ("らすとげんご", "Rust"),
            ("おぶざーばびりてぃ", "オブザーバビリティ"),
            ("ぷるりくえすと", "プルリクエスト"),
            ("そんえきぶんきてん", "損益分岐点"),
            ("げんかしょうきゃく", "減価償却"),
            ("えむあんどえー", "M&A"),
            ("けーぴーあい", "KPI"),
            (
                "きゃっしゅこんばーじょんさいくる",
                "キャッシュコンバージョンサイクル",
            ),
            ("げんかいりえき", "限界利益"),
            ("ふりーきゃっしゅふろー", "フリーキャッシュフロー"),
            ("てきかくせいきゅうしょ", "適格請求書"),
            ("ひみつほじけいやく", "秘密保持契約"),
            ("りんぎしょ", "稟議書"),
            ("あーとでぃれくしょん", "アートディレクション"),
            ("でざいんしすてむ", "デザインシステム"),
            ("でざいんとーくん", "デザイントークン"),
            ("しーえむわいけー", "CMYK"),
            ("からーぐれーでぃんぐ", "カラーグレーディング"),
            ("ひしゃかいしんど", "被写界深度"),
            ("びじゅあるあいでんてぃてぃ", "ビジュアルアイデンティティ"),
            ("とーんあんどまなー", "トーン＆マナー"),
            ("きーびじゅある", "キービジュアル"),
            ("わいやーふれーむ", "ワイヤーフレーム"),
            ("ちゃっとじーぴーてぃー", "ChatGPT"),
            ("おーぷんえーあい", "OpenAI"),
            ("せいせいえーあい", "生成AI"),
            ("のーどじぇいえす", "Node.js"),
            ("りなっくす", "Linux"),
            ("べきとうせい", "冪等性"),
            ("えすでぃーじーず", "SDGs"),
            ("じーでぃーぴーあーる", "GDPR"),
            ("くりのべぜいきんしさん", "繰延税金資産"),
            ("ふぃぐま", "Figma"),
            ("あふたーえふぇくつ", "After Effects"),
            ("きんそくしょり", "禁則処理"),
        ] {
            assert_eq!(
                dictionary.candidates(reading)[0].surface,
                surface,
                "{reading}"
            );
        }
    }

    #[test]
    fn english_typed_in_kana_mode_surfaces_ascii_words() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: false,
            history_learning: false,
            dictionary_packs: TECHNOLOGY_DICTIONARY,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "github");
        assert_eq!(engine.snapshot().preedit, "ぎてゅb");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
        assert!(
            engine.snapshot().candidates.contains(&"GitHub".to_owned()),
            "{:?}",
            engine.snapshot().candidates
        );

        engine.handle(InputEvent::Space);
        assert!(
            engine.snapshot().candidates.contains(&"GitHub".to_owned()),
            "{:?}",
            engine.snapshot().candidates
        );

        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: false,
            history_learning: false,
            dictionary_packs: TECHNOLOGY_DICTIONARY,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut engine, "python");
        assert!(
            engine.snapshot().candidates.contains(&"Python".to_owned()),
            "{:?}",
            engine.snapshot().candidates
        );
    }

    #[test]
    fn whole_reading_words_suppress_patchwork_candidates() {
        let dictionary = bundled_dictionary(TECHNOLOGY_DICTIONARY, &UserData::default());

        let github = dictionary.candidates("ぎっとはぶ");
        assert_eq!(github[0].surface, "GitHub");
        assert!(
            github.iter().all(|candidate| {
                !candidate.surface.contains("は部") && !candidate.surface.contains("羽生")
            }),
            "patchwork paths should stay hidden: {github:?}"
        );

        // Near-tie patchworks stay available when they are plausible.
        let kyouto = dictionary.candidates("きょうと");
        assert_eq!(kyouto[0].surface, "京都");
        assert!(kyouto.iter().any(|candidate| candidate.surface == "今日と"));
        assert!(kyouto.iter().all(|candidate| candidate.surface != "強と"));

        // Sentence-sized readings keep their multi-segment alternatives.
        let sentence = dictionary.candidates("らすとのきょく");
        assert_eq!(sentence[0].surface, "ラストの曲");
        assert!(
            sentence
                .iter()
                .any(|candidate| candidate.surface == "ラストの極")
        );
    }

    #[test]
    fn domain_dictionaries_do_not_override_common_ambiguous_words() {
        let dictionary = bundled_dictionary(ALL_DOMAIN_DICTIONARIES, &UserData::default());

        assert_eq!(dictionary.candidates("けっさい")[0].surface, "決済");
        assert_eq!(dictionary.candidates("らすと")[0].surface, "ラスト");
        assert_eq!(dictionary.candidates("こまわり")[0].surface, "小回り");
        assert!(
            dictionary
                .candidates("こまわり")
                .iter()
                .any(|candidate| candidate.surface == "コマ割り")
        );
        assert_eq!(
            dictionary.convert_best("らすとのきょく").unwrap().surface,
            "ラストの曲"
        );
        assert_eq!(
            dictionary.convert_best("けっさいほうほう").unwrap().surface,
            "決済方法"
        );
    }

    #[test]
    fn history_completion_stays_composing_until_accepted() {
        let directory = test_directory("completion");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nぱふぉーまんす\tパフォーマンス\t5\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "pafo");
        assert_eq!(engine.snapshot().preedit, "ぱふぉ");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
        assert_eq!(engine.snapshot().candidates, ["パフォーマンス"]);

        let actions = engine.handle(InputEvent::AcceptCandidate);
        assert!(actions.contains(&SlimeAction::Commit("パフォーマンス".to_owned())));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completions_hide_when_reading_stops_matching_history() {
        let directory = test_directory("completion-stale-hide");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nどうじしんこう\t同時進行\t5\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "dou");
        assert_eq!(engine.snapshot().candidates, ["同時進行"]);

        engine.handle(InputEvent::Character('g'));
        assert_eq!(engine.snapshot().candidates, ["同時進行"]);

        let actions = engine.handle(InputEvent::Character('u'));
        assert!(actions.contains(&SlimeAction::HideCandidates));
        assert!(engine.snapshot().candidates.is_empty());
        assert_eq!(engine.snapshot().preedit, "どうぐ");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn accepted_history_completion_is_ranked_first_after_reload() {
        let directory = test_directory("completion-ranking");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nぱふぉーまんす\tパフォーマンス\t6\t20\nぱふぇづくり\tパフェ作り\t5\t10\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        type_text(&mut engine, "pafu");
        assert_eq!(
            engine.snapshot().candidates,
            ["パフォーマンス", "パフェ作り"]
        );
        engine.handle(InputEvent::SelectCandidate(1));
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("パフェ作り".to_owned())));

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        type_text(&mut reloaded, "pafu");
        assert_eq!(
            reloaded.snapshot().candidates,
            ["パフェ作り", "パフォーマンス"]
        );
        let completion_actions = reloaded.handle(InputEvent::NextCandidate);
        assert!(
            shown_candidate_details(&completion_actions)
                .iter()
                .all(|detail| detail.annotation == CandidateAnnotation::Completion)
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enabled_history_records_committed_conversion() {
        let directory = test_directory("learning");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::Enter);

        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(history.contains("にほん\t日本\t1\t"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn history_reorders_exact_conversion_candidates() {
        let directory = test_directory("history-ranking");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t感じ\t1\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "kanji");
        let actions = engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "感じ");
        assert_eq!(
            shown_candidate_details(&actions)[0].annotation,
            CandidateAnnotation::History
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn one_off_exact_candidate_does_not_replace_an_established_candidate() {
        let directory = test_directory("exact-history-learning-strength");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t漢字\t100\t20\nかんじ\t感じ\t1\t10\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        let selected = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == "感じ")
            .unwrap();
        engine.handle(InputEvent::SelectCandidate(
            u32::try_from(selected).unwrap(),
        ));
        engine.handle(InputEvent::Enter);

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        type_text(&mut reloaded, "kanji");
        reloaded.handle(InputEvent::Space);
        assert_eq!(reloaded.snapshot().preedit, "漢字");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_context_beats_global_recency_and_persists() {
        let directory = test_directory("session-context");
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        for _ in 0..2 {
            convert_and_commit(&mut engine, "bunshou", "文章");
            convert_and_commit(&mut engine, "kanji", "漢字");
            convert_and_commit(&mut engine, "kimochi", "気持ち");
            convert_and_commit(&mut engine, "kanji", "感じ");
        }
        convert_and_commit(&mut engine, "bunshou", "文章");

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "漢字");

        let context = fs::read_to_string(directory.join("context_history.tsv")).unwrap();
        assert!(context.contains("ぶんしょう\t文章\tかんじ\t漢字\t2\t"));

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        convert_and_commit(&mut reloaded, "bunshou", "文章");
        type_text(&mut reloaded, "kanji");
        reloaded.handle(InputEvent::Space);
        assert_eq!(reloaded.snapshot().preedit, "漢字");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn one_off_context_does_not_override_global_history() {
        let directory = test_directory("one-off-context");
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        convert_and_commit(&mut engine, "bunshou", "文章");
        convert_and_commit(&mut engine, "kanji", "漢字");
        for _ in 0..5 {
            convert_and_commit(&mut engine, "kimochi", "気持ち");
            convert_and_commit(&mut engine, "kanji", "感じ");
        }
        convert_and_commit(&mut engine, "bunshou", "文章");

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "感じ");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pausing_learning_breaks_left_context_boundary() {
        let directory = test_directory("left-context-pause");
        let learning = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(learning);

        convert_and_commit(&mut engine, "bunshou", "文章");
        convert_and_commit(&mut engine, "kanji", "漢字");
        convert_and_commit(&mut engine, "kimochi", "気持ち");
        convert_and_commit(&mut engine, "kanji", "感じ");
        convert_and_commit(&mut engine, "bunshou", "文章");

        engine.set_preferences(EnginePreferences {
            history_learning: false,
            ..learning
        });
        engine.set_preferences(learning);
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "感じ");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_context_reranks_prefix_completions_and_persists() {
        let directory = test_directory("persistent-completion-context");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじへんかん\t漢字変換\t5\t10\nかんじょうひょうげん\t感情表現\t5\t20\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        for _ in 0..2 {
            convert_and_commit(&mut engine, "bunshou", "文章");
            accept_completion(&mut engine, "kanji", "漢字変換");
            convert_and_commit(&mut engine, "kimochi", "気持ち");
            accept_completion(&mut engine, "kanji", "感情表現");
        }
        convert_and_commit(&mut engine, "bunshou", "文章");

        type_text(&mut engine, "kanji");
        assert_eq!(engine.snapshot().candidates[0], "漢字変換");

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        convert_and_commit(&mut reloaded, "bunshou", "文章");
        type_text(&mut reloaded, "kanji");
        assert_eq!(reloaded.snapshot().candidates[0], "漢字変換");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn history_ignores_short_or_literal_commits() {
        assert!(!super::should_record_history("に", "二"));
        assert!(!super::should_record_history("かな", "かな"));
        assert!(super::should_record_history("にほん", "日本"));
    }

    #[test]
    fn history_can_be_used_without_learning_new_commits() {
        let directory = test_directory("learning-paused");
        let path = directory.join("history.tsv");
        let original = "# slime-history-v1\nかんじ\t感じ\t2\t10\n";
        fs::write(&path, original).unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "感じ");
        engine.handle(InputEvent::Enter);

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn transient_context_does_not_override_an_established_context() {
        let directory = test_directory("established-context");
        fs::write(
            directory.join("context_history.tsv"),
            "# slime-context-history-v1\nぶんしょう\t文章\tかんじ\t漢字\t100\t10\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        for _ in 0..2 {
            convert_and_commit(&mut engine, "bunshou", "文章");
            convert_and_commit(&mut engine, "kanji", "感じ");
        }
        convert_and_commit(&mut engine, "bunshou", "文章");
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "漢字");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn learning_can_continue_while_history_candidates_are_hidden() {
        let directory = test_directory("suggestions-hidden");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: false,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::Enter);

        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(history.contains("にほん\t日本\t1\t"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn apostrophe_spellings_for_foreign_sounds_remain_composable() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "t'id'yu");

        assert_eq!(engine.snapshot().preedit, "てぃでゅ");
    }

    #[test]
    fn typo_correction_is_labeled_keeps_the_original_and_learns_the_corrected_reading() {
        let directory = test_directory("typo-correction");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nihpn");
        let original = "にhpん".to_owned();
        let actions = engine.handle(InputEvent::Space);
        let snapshot = engine.snapshot();
        assert_eq!(snapshot.preedit, original);
        assert_eq!(snapshot.candidates.first(), Some(&original));
        let corrected = snapshot
            .candidates
            .iter()
            .position(|candidate| candidate == "日本")
            .expect("neighbor-key correction should offer 日本");
        assert!(actions.iter().any(|action| {
            matches!(
                action,
                SlimeAction::ShowCandidates { candidates, .. }
                    if candidates.iter().any(|candidate| candidate == "日本　（にほんに訂正）")
            )
        }));

        engine.handle(InputEvent::SelectCandidate(
            u32::try_from(corrected).unwrap(),
        ));
        assert_eq!(engine.snapshot().preedit, "日本");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("日本".to_owned())));

        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(history.contains("にほん\t日本\t1\t"));
        assert!(!history.contains(&format!("{original}\t日本")));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn known_reading_does_not_show_typo_annotations() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        let actions = engine.handle(InputEvent::Space);

        assert!(actions.iter().all(|action| {
            !matches!(
                action,
                SlimeAction::ShowCandidates { candidates, .. }
                    if candidates.iter().any(|candidate| candidate.contains("に訂正）"))
            )
        }));
        assert_eq!(engine.snapshot().preedit, "日本");
    }

    #[test]
    fn typo_correction_labels_a_surface_already_reachable_by_patchwork() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にh", "日", 0),
            DictionaryEntry::new("pん", "本", 0),
            DictionaryEntry::new("にほん", "日本", 0),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, "nihpn");
        let actions = engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().candidates[1], "日本");
        assert!(actions.iter().any(|action| {
            matches!(
                action,
                SlimeAction::ShowCandidates { candidates, .. }
                    if candidates.get(1).is_some_and(|candidate| candidate == "日本　（にほんに訂正）")
            )
        }));
    }

    #[test]
    fn typo_correction_recovers_one_missing_vowel() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihn");

        let actions = engine.handle(InputEvent::Space);
        assert!(engine.snapshot().candidates.contains(&"日本".to_owned()));
        assert!(actions.iter().any(|action| {
            matches!(
                action,
                SlimeAction::ShowCandidates { candidates, .. }
                    if candidates.iter().any(|candidate| candidate == "日本　（にほんに訂正）")
            )
        }));
    }

    #[test]
    fn punctuation_resolves_a_trailing_n_before_it_is_inserted() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "hon,");

        assert_eq!(engine.snapshot().preedit, "ほん、");
    }

    #[test]
    fn space_starts_conversion_and_cycles_candidates() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "日本");
        assert_eq!(engine.snapshot().phase, Phase::Converting);

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "ニホン");
    }

    #[test]
    fn cycling_past_short_reading_candidates_runs_one_wider_search() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "asairi");
        engine.handle(InputEvent::Space);

        let initial_count = engine.snapshot().candidates.len();
        assert!(!engine.snapshot().candidates.contains(&"浅煎り".to_owned()));

        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.len() > initial_count);
        assert!(engine.snapshot().candidates.contains(&"浅煎り".to_owned()));
        assert_eq!(engine.snapshot().selected, Some(initial_count));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn cycling_long_reading_uses_bounded_compounds_without_wide_n_best() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihonjin");
        engine.handle(InputEvent::Space);

        let initial_count = engine.snapshot().candidates.len();
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.len() >= initial_count);
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn cycling_long_reading_adds_fixed_segment_variants() {
        let mut entries = Vec::new();
        for (reading, prefix) in [("あいう", "第一"), ("えおか", "第二"), ("きくけ", "第三")]
        {
            for (index, cost) in [10, 20, 30, 40].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let reading = "あいうえおかきくけ";
        let dictionary = Dictionary::new(entries);
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, reading);
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .fixed_segment_variants(
                reading,
                super::FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
                super::FIXED_SEGMENT_CANDIDATE_LIMIT,
            )
            .into_iter()
            .find(|surface| !initial.contains(surface))
            .expect("fixed-segment recall should add a candidate beyond N-best 10");
        let initial_count = initial.len();
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn cycling_long_reading_adds_recombined_observed_paths() {
        let mut entries = Vec::new();
        let mut reading = String::new();
        for (segment_reading, original, alternative) in [
            ("あい", "第一", "別一"),
            ("うえ", "第二", "別二"),
            ("おか", "第三", "別三"),
            ("きく", "第四", "別四"),
            ("けこ", "第五", "別五"),
            ("さし", "第六", "別六"),
        ] {
            reading.push_str(segment_reading);
            entries.push(DictionaryEntry::new(segment_reading, original, 10));
            entries.push(DictionaryEntry::new(segment_reading, alternative, 11));
        }
        let dictionary = Dictionary::new(entries);
        let initial = dictionary
            .candidates(&reading)
            .into_iter()
            .map(|candidate| candidate.surface)
            .collect::<Vec<_>>();
        let fixed = dictionary.fixed_segment_variants(
            &reading,
            super::FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
            super::FIXED_SEGMENT_CANDIDATE_LIMIT,
        );
        let target = dictionary
            .recombined_n_best_variants(
                &reading,
                super::RECOMBINED_SOURCE_PATHS,
                super::RECOMBINED_CANDIDATE_LIMIT,
            )
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface) && !fixed.contains(surface))
            .expect("recombination should add a path outside the existing candidate stages");
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, &reading);
        engine.handle(InputEvent::Space);
        let initial_count = engine.snapshot().candidates.len();
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn explicit_conversion_recalls_independent_homophones_before_cycling() {
        let reading = "きこうはねんかんをつうじてひじょうにかんれいであり、なつでもひょうてんをこえることはなく";
        let expected =
            "気候は年間を通じて非常に寒冷であり、夏でも氷点を超えることはなく".to_owned();
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, reading);
        engine.handle(InputEvent::Space);
        let initial_count = engine.snapshot().candidates.len();
        assert!(engine.snapshot().candidates.contains(&expected));
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&expected));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn live_suffix_keeps_a_supported_long_conversion_while_a_word_is_incomplete() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        let reading = "しりょうなどをすべてはきしたことがあかされたため、さいはんはふかのうなじ";
        type_text(&mut engine, reading);
        let before = engine.snapshot().preedit;
        assert!(before.contains("破棄したこと"), "{before}");
        engine.handle(InputEvent::Character('ょ'));
        assert!(
            engine.snapshot().preedit.contains("破棄したこと"),
            "{}",
            engine.snapshot().preedit
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        assert!(snapshot.target_is_literal());
        let preview = engine.live_preview.as_ref().unwrap();
        let next_target = format!("{}う", snapshot.target_reading);
        let (_, _, stable_surface) = super::live_suffix_context(
            preview,
            preview.stable_prefix.as_ref().unwrap(),
            &next_target,
        );
        assert_eq!(stable_surface.as_deref(), Some(next_target.as_str()));
    }

    #[test]
    fn live_suffix_continuity_requires_support_and_a_safe_kana_append() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        let reading = "しりょうなどをすべてはきしたことがあかされたため、さいはんはふかのうなじ";
        type_text(&mut engine, reading);
        let previous = engine.live_preview.as_ref().unwrap();
        let prefix = previous.stable_prefix.as_ref().unwrap();
        let resolved = format!("{reading}ょ");
        let supported = previous.surface.strip_prefix(&prefix.surface).unwrap();
        assert!(
            super::supported_live_suffix_extension(previous, prefix, &resolved, supported)
                .is_some()
        );
        assert!(
            super::supported_live_suffix_extension(previous, prefix, &resolved, "異なる候補")
                .is_none()
        );
        for edited in [
            reading.to_owned(),
            format!("{reading}じょ"),
            format!("{reading}1"),
        ] {
            assert!(
                super::supported_live_suffix_extension(previous, prefix, &edited, supported)
                    .is_none()
            );
        }
        let mut fragile = previous.clone();
        fragile.prefix_fragile = true;
        assert!(
            super::supported_live_suffix_extension(&fragile, prefix, &resolved, supported)
                .is_none()
        );
        let mut numeric = previous.clone();
        numeric.surface.push('1');
        assert!(
            super::supported_live_suffix_extension(
                &numeric,
                prefix,
                &resolved,
                &format!("{supported}1")
            )
            .is_none()
        );
    }

    #[test]
    fn bounded_compound_recall_reaches_long_candidates_without_wide_n_best() {
        let mut entries = Vec::new();
        for (surface, cost) in [("左一", 0), ("左二", 1), ("左三", 2), ("左四", 3)] {
            entries.push(DictionaryEntry::new("あいうえお", surface, cost));
        }
        for (surface, cost) in [("右一", 0), ("右二", 1), ("右三", 2), ("右四", 3)] {
            entries.push(DictionaryEntry::new("かきくけこ", surface, cost));
        }
        let mut engine = SlimeEngine::new(Dictionary::new(entries));
        type_text(&mut engine, "あいうえおかきくけこ");
        engine.handle(InputEvent::Space);

        let target = "左四右四".to_owned();
        let initial_count = engine.snapshot().candidates.len();
        assert!(!engine.snapshot().candidates.contains(&target));
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_deeper_component_and_product_candidates() {
        let mut entries = Vec::new();
        for (reading, prefix) in [("あいうえお", "左"), ("かきくけこ", "右")] {
            for index in 0..8 {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{}", index + 1),
                    index * 100,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let reading = "あいうえおかきくけこ";
        let old_bound = dictionary.compound_candidates(reading, 4, 16);
        let wider = dictionary.compound_candidates(reading, 8, 32);
        let deeper_component = "左5右1".to_owned();
        let deeper_product = wider
            .get(20)
            .expect("the wider product beam should contain at least 21 candidates")
            .surface
            .clone();
        assert!(!old_bound.iter().any(|candidate| {
            candidate.surface == deeper_component || candidate.surface == deeper_product
        }));
        assert!(
            wider
                .iter()
                .any(|candidate| candidate.surface == deeper_component)
        );

        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, reading);
        engine.handle(InputEvent::Space);
        let initial = engine.snapshot().candidates;
        assert!(!initial.contains(&deeper_component));
        assert!(!initial.contains(&deeper_product));
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        let expanded = engine.snapshot().candidates;
        assert!(expanded.contains(&deeper_component));
        assert!(expanded.contains(&deeper_product));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_three_part_candidates_without_wide_n_best() {
        let mut entries = Vec::new();
        for (reading, prefix) in [("あいう", "左"), ("えおか", "中"), ("きくけ", "右")]
        {
            for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, "あいうえおかきくけ");
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .compound_candidates("あいうえおかきくけ", 4, 16)
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface))
            .expect("three-part recall should add a candidate beyond N-best 10");
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_a_one_character_reading_segment() {
        let mut entries = Vec::new();
        for (reading, prefix) in [("あい", "左"), ("う", "中"), ("えお", "右")] {
            for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, "あいうえお");
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .compound_candidates("あいうえお", 4, 16)
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface))
            .expect("one-character segment recall should add a candidate beyond N-best 10");
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_a_kana_only_segment_without_wide_n_best() {
        let mut entries = vec![DictionaryEntry::new("の", "の", 5)];
        for (reading, prefix) in [("あいう", "左"), ("えおか", "中"), ("きくけ", "右")]
        {
            for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let reading = "あいうのえおかきくけ";
        assert!(reading.chars().count() > MAX_EXPANDED_READING_CHARACTERS);
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, reading);
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .compound_candidates(reading, 4, 16)
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface))
            .expect("kana-only segment recall should add a candidate beyond N-best 10");
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(target.contains('の'));
        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_four_part_candidates_without_wide_n_best() {
        let mut entries = Vec::new();
        for (reading, prefix) in [
            ("あいう", "一"),
            ("えおか", "二"),
            ("きくけ", "三"),
            ("こさし", "四"),
        ] {
            for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, "あいうえおかきくけこさし");
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .compound_candidates("あいうえおかきくけこさし", 4, 16)
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface))
            .expect("four-part recall should add a candidate beyond N-best 10");
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_five_part_candidates_without_wide_n_best() {
        let mut entries = Vec::new();
        for (reading, prefix) in [
            ("あいう", "一"),
            ("えおか", "二"),
            ("きくけ", "三"),
            ("こさし", "四"),
            ("すせそ", "五"),
        ] {
            for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, "あいうえおかきくけこさしすせそ");
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .compound_candidates("あいうえおかきくけこさしすせそ", 4, 16)
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface))
            .expect("five-part recall should add a candidate beyond N-best 10");
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn bounded_compound_recall_reaches_six_part_candidates_without_wide_n_best() {
        let mut entries = Vec::new();
        for (reading, prefix) in [
            ("あい", "一"),
            ("うえ", "二"),
            ("おか", "三"),
            ("きく", "四"),
            ("けこ", "五"),
            ("さし", "六"),
        ] {
            for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let reading = "あいうえおかきくけこさし";
        let mut engine = SlimeEngine::new(dictionary.clone());
        type_text(&mut engine, reading);
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = dictionary
            .compound_candidates(reading, 4, 16)
            .into_iter()
            .map(|candidate| candidate.surface)
            .find(|surface| !initial.contains(surface))
            .expect("six-part recall should add a candidate beyond N-best 10");
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);
    }

    #[test]
    fn conversion_always_includes_a_unique_full_width_katakana_candidate() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "hogehoge");

        engine.handle(InputEvent::Space);

        assert!(
            engine
                .snapshot()
                .candidates
                .contains(&"ホゲホゲ".to_owned())
        );
        assert_eq!(
            engine
                .snapshot()
                .candidates
                .iter()
                .filter(|candidate| candidate.as_str() == "ホゲホゲ")
                .count(),
            1
        );
        assert!(
            engine.snapshot().candidates[..2].contains(&"ホゲホゲ".to_owned()),
            "katakana candidate stays on the first page: {:?}",
            &engine.snapshot().candidates[..2]
        );
    }

    #[test]
    fn katakana_candidate_preserves_long_vowels_symbols_and_non_hiragana() {
        assert_eq!(
            katakana_candidate("ぱふぉーまんす・１２３"),
            "パフォーマンス・１２３"
        );
        assert_eq!(katakana_candidate("ゔゝゞ"), "ヴヽヾ");
    }

    #[test]
    fn live_neural_ranking_protects_short_literal_suffixes_after_a_stable_prefix() {
        assert!(!live_ranked_target_is_safe(
            true,
            "である",
            "である",
            "で有る"
        ));
        assert!(!live_ranked_target_is_safe(
            true,
            "となり",
            "となり",
            "と成"
        ));
        assert!(live_ranked_target_is_safe(false, "よる", "よる", "夜"));
        assert!(live_ranked_target_is_safe(
            true,
            "しょうさんされた",
            "しょうさんされた",
            "称賛された"
        ));
        assert!(live_ranked_target_is_safe(
            true,
            "しぼうし",
            "しぼうし",
            "死亡し"
        ));
        assert!(live_ranked_target_is_safe(true, "やぶれ", "やぶれ", "敗れ"));
        assert!(live_ranked_target_is_safe(
            true,
            "あげます",
            "あげます",
            "挙げます"
        ));
        assert!(!live_ranked_target_is_safe(
            true,
            "なって",
            "なって",
            "成って"
        ));
        assert!(live_ranked_target_is_safe(
            true,
            "つよくひかれる",
            "強くひかれる",
            "強く惹かれる"
        ));
        assert!(!live_ranked_target_is_safe(false, "かよう", "通う", "火曜"));
        assert!(super::rewrites_particle_looking_suffix_as_two_kanji_word(
            "あさの",
            "朝の",
            "浅野"
        ));
        assert!(!super::rewrites_particle_looking_suffix_as_two_kanji_word(
            "かよう",
            "通う",
            "火曜"
        ));
        assert!(live_ranked_target_is_safe(
            false,
            "つとめる",
            "努める",
            "務める"
        ));
        assert!(!live_ranked_target_is_safe(
            false,
            "かれをぶらいあんとのかわりにばってきした",
            "彼をブライアントの代わりに抜擢した",
            "彼をブライアンとの代わりに抜擢した"
        ));
        assert!(live_ranked_target_is_safe(
            false,
            "あわびをたべる",
            "アワビを食べる",
            "あわびを食べる"
        ));
        assert!(live_ranked_target_is_safe(false, "のめ", "の目", "飲め"));
        assert!(live_ranked_target_is_safe(
            false,
            "せっちのよういせいとけいざいせいから、ちゅうしゃじょうにしようされることがおおい",
            "設置のよういせいとけいざいせいから、ちゅうしゃじょうにしようされることがおおい",
            "設置の容易性と経済性から、駐車場に使用されることが多い"
        ));
        assert!(live_ranked_target_is_safe(
            false,
            "きょうさんとうにたいしとうそうのりねんをもっていた）ときりかえした",
            "共産党に対しとうそうのりねんをもっていた）ときりかえした",
            "共産党に対し闘争の理念を持っていた）と切り返した"
        ));
    }

    #[test]
    fn live_neural_ranking_allows_implicit_numeric_repairs_with_a_length_change() {
        assert!(live_ranked_target_is_safe(true, "いち", "1", "位置"));
        assert!(!live_ranked_target_is_safe(true, "いち", "一", "位置"));
    }

    #[test]
    fn live_neural_request_replaces_numeric_tail_duplicates_with_fixed_segment_repair() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あいう", "1甲", 0),
            DictionaryEntry::new("あいう", "一甲", 1_000),
            DictionaryEntry::new("えおか", "2乙", 0),
            DictionaryEntry::new("えおか", "二乙", 1_000),
            DictionaryEntry::new("きくけ", "3丙", 0),
            DictionaryEntry::new("きくけ", "三丙", 1_000),
        ]);
        let reading = "あいうえおかきくけ";
        let repair = dictionary
            .fixed_segment_candidates(reading, 8, 22)
            .into_iter()
            .find(|candidate| !candidate.surface.chars().any(char::is_numeric))
            .expect("the all-kanji fixed-segment repair should be reachable");
        let mut candidates = (0..15)
            .map(|index| Candidate {
                surface: format!("1候補{index}"),
                cost: index,
            })
            .collect::<Vec<_>>();
        let original_first = candidates[0].clone();

        diversify_implicit_numeric_live_candidates(&dictionary, reading, &mut candidates);

        assert_eq!(candidates.len(), 15);
        assert_eq!(candidates[0], original_first);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.surface == repair.surface)
        );
        assert_eq!(
            candidates
                .iter()
                .find(|candidate| candidate.surface == repair.surface)
                .map(|candidate| candidate.cost),
            Some(original_first.cost + super::LIVE_IMPLICIT_NUMERIC_REPAIR_PRIOR_COST_GAP)
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.surface.chars().any(char::is_numeric))
                .count(),
            14
        );

        let mut already_diverse = (0..15)
            .map(|index| Candidate {
                surface: if index == 1 {
                    "一候補".to_owned()
                } else {
                    format!("1候補{index}")
                },
                cost: index,
            })
            .collect::<Vec<_>>();
        let original = already_diverse.clone();

        diversify_implicit_numeric_live_candidates(&dictionary, reading, &mut already_diverse);

        assert_eq!(already_diverse, original);
    }

    #[test]
    fn live_neural_request_replaces_tail_with_a_recombined_segment_cross_product() {
        let dictionary = Dictionary::bundled();
        let reading = "よってはこうせんじゅうやねっせんじゅうでも";
        let candidates = dictionary.candidates_with_limit(reading, 15);
        let mut request = CandidateRankingRequest {
            reading: reading.to_owned(),
            left_context: "作品に".to_owned(),
            candidates: candidates
                .into_iter()
                .map(|candidate| CandidateRankingItem {
                    surface: candidate.surface,
                    cost: candidate.cost,
                })
                .collect(),
        };
        let original_len = request.candidates.len();
        let original_base = request.candidates[0].clone();
        assert!(
            !request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "よっては光線銃や熱線銃でも")
        );

        diversify_recombined_live_candidates(&dictionary, &mut request, false, false);

        assert_eq!(request.candidates.len(), original_len);
        assert_eq!(request.candidates[0], original_base);
        assert!(
            request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "よっては光線銃や熱線銃でも")
        );

        let guarded = request.clone();
        diversify_recombined_live_candidates(&dictionary, &mut request, true, false);
        assert_eq!(request, guarded);
        diversify_recombined_live_candidates(&dictionary, &mut request, false, true);
        assert_eq!(request, guarded);
    }

    #[test]
    fn live_neural_ranking_protects_an_internal_short_literal_tail() {
        assert!(rewrites_only_protected_literal_tail(
            "えふぇくたーをたようしたはーどかつ",
            "エフェクターを多用したハードかつ",
            "エフェクターを多用したハード勝"
        ));
        assert!(!rewrites_only_protected_literal_tail(
            "では、ぎいんのふたいほとっけん",
            "では、議員の不逮捕とっけん",
            "では、議員の不逮捕特権"
        ));
        assert!(live_ranked_target_is_safe(
            true,
            "とっけん",
            "とっけん",
            "特権"
        ));
        assert!(!live_ranked_target_is_safe(true, "ちち", "ちち", "の乳"));
        assert!(!live_ranked_target_is_safe(true, "かつ", "かつ", "勝"));
        assert!(!live_ranked_target_is_safe(
            true,
            "ともに",
            "ともに",
            "共に"
        ));
        assert!(!live_ranked_target_is_safe(
            true,
            "もとに",
            "もとに",
            "下に"
        ));
        assert!(!rewrites_only_protected_literal_tail("よる", "よる", "夜"));
        assert!(!rewrites_only_protected_literal_tail(
            "しょうさんされた",
            "しょうさんされた",
            "称賛された"
        ));
        assert!(!rewrites_only_protected_literal_tail(
            "をきょうだ、しぼうし",
            "を強打、しぼうし",
            "を強打、死亡し"
        ));
        assert!(!rewrites_only_protected_literal_tail(
            "はぎりしあにやぶれ",
            "はギリシアにやぶれ",
            "はギリシアに敗れ"
        ));
        assert!(!rewrites_only_protected_literal_tail(
            "じょうどきょうにつよくひかれる",
            "浄土教に強くひかれる",
            "浄土教に強く惹かれる"
        ));
        assert!(!rewrites_only_protected_literal_tail(
            "てんたいしょうのいち",
            "点対称の1",
            "点対称の位置"
        ));
        assert!(!rewrites_only_protected_literal_tail(
            "ふぁんた、げろっぱ",
            "ファンタ、げろっぱ",
            "ファンタ、ゲロッパ"
        ));
        assert!(rewrites_only_protected_literal_tail(
            "ぶんしょう、げろっぱ",
            "文章、げろっぱ",
            "文章、ゲロッパ"
        ));
        assert!(rewrites_only_protected_literal_tail(
            "ふぁんた、とか",
            "ファンタ、とか",
            "ファンタ、トカ"
        ));
        assert!(super::rewrites_short_hiragana_tail_as_katakana(
            "戦後裸一貫から",
            "戦後裸一貫カラ"
        ));
        assert!(!super::rewrites_short_hiragana_tail_as_katakana(
            "の復しゅう",
            "の復讐"
        ));
        assert!(!super::rewrites_short_hiragana_tail_as_katakana(
            "ファンタ、げろっぱ",
            "ファンタ、ゲロッパ"
        ));
        assert!(!super::rewrites_short_hiragana_tail_as_katakana(
            "野田芳ひこ",
            "野田芳ヒコ"
        ));
    }

    #[test]
    fn delayed_live_ranking_prefers_an_existing_hiragana_tail_twin() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "せんごはだかいっかんから");
        assert_eq!(engine.snapshot().preedit, "せんごはだかいっかんから");

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let katakana = ranked
            .iter()
            .position(|surface| surface == "戦後裸一貫カラ")
            .expect("the expanded LIVE pool contains the katakana-tail candidate");
        let katakana = ranked.remove(katakana);
        ranked.insert(0, katakana);

        assert_eq!(
            snapshot.safer_literal_tail_for_ranked_winner(&request, &ranked),
            Some("戦後裸一貫から".to_owned())
        );
        assert!(!snapshot.ranked_winner_is_safe(&request, &ranked));

        let hiragana = ranked
            .iter()
            .position(|surface| surface == "戦後裸一貫から")
            .unwrap();
        let hiragana = ranked.remove(hiragana);
        ranked.insert(0, hiragana);
        assert!(snapshot.ranked_winner_is_safe(&request, &ranked));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, "戦後裸一貫から");
    }

    #[test]
    fn delayed_live_ranking_applies_a_shifted_boundary_and_literal_target_repair() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "しろにちかいいろでもろく、うしのちち");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let selected = ranked[0].as_str();

        assert!(snapshot.request_reopens_stable_prefix(&request));
        assert!(
            snapshot.reopened_request_changes_are_bounded(&request, selected),
            "a dictionary-backed one-particle boundary shift may repair its literal target"
        );
        assert!(snapshot.ranked_target_is_safe(&request, selected));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, "白に近い色で脆く、牛の乳");
    }

    #[test]
    fn delayed_live_ranking_reopens_a_particle_inside_a_compound() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("三段変速機、大型で頑丈なスチール");
        type_text(
            &mut engine,
            "ばすけっと、せきさいせいにすぐれるにだいがそうちゃく",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "バスケット、積載性に優れるにだいがそうちゃく"
        );

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let selected = ranked[0].as_str();
        assert_eq!(selected, "バスケット、積載性に優れる荷台が装着");
        assert!(snapshot.request_reopens_stable_prefix(&request));
        assert!(snapshot.reopened_request_changes_are_bounded(&request, selected));
        assert!(snapshot.ranked_target_is_safe(&request, selected));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(
            engine.snapshot().preedit,
            "バスケット、積載性に優れる荷台が装着"
        );
    }

    #[test]
    fn delayed_live_ranking_reopens_one_segment_kanji_length_repair() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(
            &mut engine,
            "させ、あつりょくかくへきかぶをそんしょうするなどし",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "させ、圧力隔壁株を損傷するなどし"
        );

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(snapshot.request_reopens_stable_prefix(&request));
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let expected = "させ、圧力隔壁下部を損傷するなどし";
        let expected_index = ranked
            .iter()
            .position(|candidate| candidate == expected)
            .expect("the one-segment repair remains in the worker pool");
        let expected = ranked.remove(expected_index);
        ranked.insert(0, expected);

        assert!(snapshot.ranked_winner_is_safe(&request, &ranked));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(
            engine.snapshot().preedit,
            "させ、圧力隔壁下部を損傷するなどし"
        );
    }

    #[test]
    fn dictionary_katakana_candidate_is_not_duplicated() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        assert_eq!(
            engine
                .snapshot()
                .candidates
                .iter()
                .filter(|candidate| candidate.as_str() == "ニホン")
                .count(),
            1
        );
        assert_eq!(engine.snapshot().candidates[1], "ニホン");
    }

    #[test]
    fn katakana_is_promoted_into_the_first_candidate_page() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "kikan");
        engine.handle(InputEvent::Space);

        let candidates = engine.snapshot().candidates;
        assert!(candidates.len() > 9);
        assert_eq!(candidates[1], "キカン");
    }

    #[test]
    fn selecting_candidate_by_index_updates_preedit_and_commit() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let candidates = engine.snapshot().candidates;
        let selected = candidates[1].clone();
        let actions = engine.handle(InputEvent::SelectCandidate(1));

        assert_eq!(
            actions,
            vec![
                SlimeAction::UpdatePreedit(selected.clone()),
                SlimeAction::ShowCandidates {
                    candidates: candidates.clone(),
                    details: engine.candidate_details(),
                    selected: 1,
                },
            ]
        );
        assert_eq!(engine.snapshot().selected, Some(1));
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&SlimeAction::Commit(selected))
        );
    }

    #[test]
    fn long_explicit_conversion_retains_additional_dictionary_alternatives() {
        let mut engine = SlimeEngine::bundled();
        type_text(
            &mut engine,
            "きかんしゃからおくられたこうあつこうねつのじょうきをもちいるきゅうしゅうしきれいきゃくほうしきであった",
        );
        engine.handle(InputEvent::Space);
        let request = engine
            .candidate_ranking_request()
            .expect("long conversion is rankable");
        assert!(request.candidates.len() > EXPLICIT_RANKING_CANDIDATE_LIMIT);
        assert!(
            request.candidates.len()
                <= super::EXPLICIT_LONG_RANKING_CANDIDATE_LIMIT
                    + super::EXPLICIT_RECOMBINED_CANDIDATE_LIMIT
        );
        assert!(request.candidates.iter().any(|candidate| candidate.surface
            == "機関車から送られた高圧高熱の蒸気を用いる吸収式冷却方式であった"));
        assert!(
            request
                .candidates
                .iter()
                .all(|candidate| engine.snapshot().candidates.contains(&candidate.surface))
        );
    }

    #[test]
    fn explicit_recombination_does_not_replace_literal_hiragana() {
        let mut engine = SlimeEngine::bundled();
        type_text(
            &mut engine,
            "れさらわれ、ついせきするもいぐなーつにかえりうちにされじゅうしょうをおう",
        );
        engine.handle(InputEvent::Space);
        let request = engine.candidate_ranking_request().unwrap();
        assert!(request.candidates.iter().any(
            |item| item.surface == "れさらわれ、追跡するもイグナーツに返り討ちにされ重傷を負う"
        ));
        assert!(
            !request
                .candidates
                .iter()
                .any(|item| item.surface
                    == "れ攫われ、追跡するもイグナーツに返り討ちにされ重傷を負う")
        );
    }

    #[test]
    fn explicit_recombined_candidate_can_be_ranked_and_committed() {
        let mut engine = SlimeEngine::bundled();
        type_text(
            &mut engine,
            "さくひんによってはこうせんじゅうやねっせんじゅうでも",
        );
        engine.handle(InputEvent::Space);
        let request = engine.candidate_ranking_request().unwrap();
        let expected = "作品によっては光線銃や熱線銃でも";
        let index = request
            .candidates
            .iter()
            .position(|item| item.surface == expected)
            .unwrap();
        assert!(index >= super::EXPLICIT_LONG_RANKING_CANDIDATE_LIMIT);
        assert!(request.candidates.len() <= 20);
        assert!(
            request
                .candidates
                .iter()
                .all(|item| engine.snapshot().candidates.contains(&item.surface))
        );
        let mut ranked: Vec<_> = request
            .candidates
            .into_iter()
            .map(|item| item.surface)
            .collect();
        ranked.swap(0, index);
        assert!(engine.apply_candidate_ranking(&ranked).is_some());
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&SlimeAction::Commit(expected.into()))
        );
    }

    #[test]
    fn explicit_conversion_exposes_only_a_bounded_dictionary_ranking_request() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");

        assert!(engine.candidate_ranking_request().is_none());
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_ranking_request()
            .expect("explicit conversion has rankable dictionary candidates");
        assert_eq!(request.reading, "にほん");
        assert!(request.left_context.is_empty());
        assert!((2..=EXPLICIT_RANKING_CANDIDATE_LIMIT).contains(&request.candidates.len()));
        assert!(
            request
                .candidates
                .iter()
                .all(|candidate| engine.snapshot().candidates.contains(&candidate.surface))
        );
    }

    #[test]
    fn targeted_ranking_transaction_commits_new_dictionary_candidate() {
        let mut engine = SlimeEngine::bundled();
        let reading = "にけいどののうあつこうしんじょうたいがじぞくすると、のうのきのうがしだいにしょうがいされ";
        let baseline = "に軽度の脳圧更新状態が持続すると、脳の機能が次第に障害され";
        let preferred = "に軽度の脳圧亢進状態が持続すると、脳の機能が次第に傷害され";
        let expected = "に軽度の脳圧亢進状態が持続すると、脳の機能が次第に障害され";
        type_text(&mut engine, reading);
        engine.handle(InputEvent::Space);
        let request = engine.candidate_ranking_request().unwrap();
        assert!(!request.candidates.iter().any(|c| c.surface == expected));
        let protected: Vec<_> = engine
            .snapshot()
            .candidates
            .into_iter()
            .enumerate()
            .filter(|(_, s)| !request.candidates.iter().any(|c| &c.surface == s))
            .collect();
        let mut calls = 0;
        assert!(
            engine
                .rank_candidates_with_recombination(|next| {
                    calls += 1;
                    let mut ranked: Vec<_> =
                        next.candidates.iter().map(|c| c.surface.clone()).collect();
                    if calls == 1 {
                        ranked.sort_by_key(|s| s != baseline);
                        Some(super::CandidateRankingPlan {
                            ranked_surfaces: ranked,
                            recombination_sources: Some([
                                baseline.to_owned(),
                                preferred.to_owned(),
                            ]),
                        })
                    } else {
                        assert_eq!(calls, 2);
                        assert_eq!(
                            &next.candidates[..request.candidates.len()],
                            request.candidates.as_slice()
                        );
                        assert!(
                            next.candidates
                                .iter()
                                .any(|c| c.surface == expected && c.cost == 61880)
                        );
                        ranked.sort_by_key(|s| s != expected);
                        Some(super::CandidateRankingPlan {
                            ranked_surfaces: ranked,
                            recombination_sources: Some([
                                expected.to_owned(),
                                preferred.to_owned(),
                            ]),
                        })
                    }
                })
                .is_some()
        );
        assert_eq!(calls, 2);
        for (position, surface) in protected {
            assert_eq!(engine.snapshot().candidates[position], surface);
        }
        assert_eq!(engine.snapshot().preedit, expected);
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&SlimeAction::Commit(expected.to_owned()))
        );
    }

    #[test]
    fn targeted_ranking_transaction_retains_initial_result_after_failed_expansion() {
        for invalid_order in [false, true] {
            let mut engine = SlimeEngine::bundled();
            let reading = "にけいどののうあつこうしんじょうたいがじぞくすると、のうのきのうがしだいにしょうがいされ";
            let baseline = "に軽度の脳圧更新状態が持続すると、脳の機能が次第に障害され";
            let preferred = "に軽度の脳圧亢進状態が持続すると、脳の機能が次第に傷害され";
            type_text(&mut engine, reading);
            engine.handle(InputEvent::Space);
            let old_length = engine.snapshot().candidates.len();
            let mut calls = 0;
            assert!(
                engine
                    .rank_candidates_with_recombination(|request| {
                        calls += 1;
                        if calls == 2 {
                            return invalid_order.then(|| super::CandidateRankingPlan {
                                ranked_surfaces: vec![
                                    "invented".to_owned();
                                    request.candidates.len()
                                ],
                                recombination_sources: None,
                            });
                        }
                        let mut ranked: Vec<_> = request
                            .candidates
                            .iter()
                            .map(|c| c.surface.clone())
                            .collect();
                        ranked.sort_by_key(|s| s != baseline);
                        Some(super::CandidateRankingPlan {
                            ranked_surfaces: ranked,
                            recombination_sources: Some([
                                baseline.to_owned(),
                                preferred.to_owned(),
                            ]),
                        })
                    })
                    .is_some()
            );
            assert_eq!(calls, 2);
            assert_eq!(engine.snapshot().candidates.len(), old_length);
            assert_eq!(engine.snapshot().preedit, baseline);
        }
    }

    #[test]
    fn targeted_ranking_transaction_rejects_unknown_sources_and_context_expansion() {
        for with_context in [false, true] {
            let mut engine = SlimeEngine::bundled();
            if with_context {
                engine.set_external_left_context("前の文。");
            }
            type_text(
                &mut engine,
                "にけいどののうあつこうしんじょうたいがじぞくすると、のうのきのうがしだいにしょうがいされ",
            );
            engine.handle(InputEvent::Space);
            let old_length = engine.snapshot().candidates.len();
            let mut calls = 0;
            assert!(
                engine
                    .rank_candidates_with_recombination(|request| {
                        calls += 1;
                        assert_eq!(calls, 1);
                        let ranked: Vec<_> = request
                            .candidates
                            .iter()
                            .map(|c| c.surface.clone())
                            .collect();
                        let sources = [
                            ranked[0].clone(),
                            if with_context {
                                ranked[1].clone()
                            } else {
                                "invented".to_owned()
                            },
                        ];
                        Some(super::CandidateRankingPlan {
                            ranked_surfaces: ranked,
                            recombination_sources: Some(sources),
                        })
                    })
                    .is_some()
            );
            assert_eq!(engine.snapshot().candidates.len(), old_length);
        }
    }

    #[test]
    fn transactional_ranking_rejects_invalid_results_and_commits_valid_permutations() {
        let mut engine = SlimeEngine::bundled();
        assert!(
            engine
                .rank_candidates_with(|_| panic!("no conversion to rank"))
                .is_none()
        );
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        let before = engine.snapshot();
        assert!(engine.rank_candidates_with(|_| None).is_none());
        assert_eq!(engine.snapshot(), before);
        assert!(
            engine
                .rank_candidates_with(|request| Some(vec![
                    request.candidates[0].surface.clone();
                    request.candidates.len()
                ]))
                .is_none()
        );
        assert_eq!(engine.snapshot(), before);
        assert!(
            engine
                .rank_candidates_with(|request| {
                    let mut ranked: Vec<_> = request
                        .candidates
                        .iter()
                        .map(|item| item.surface.clone())
                        .collect();
                    ranked[0] = "invented candidate".into();
                    Some(ranked)
                })
                .is_none()
        );
        assert_eq!(engine.snapshot(), before);
        let mut expected = String::new();
        assert!(
            engine
                .rank_candidates_with(|request| {
                    let ranked: Vec<_> = request
                        .candidates
                        .iter()
                        .rev()
                        .map(|item| item.surface.clone())
                        .collect();
                    expected.clone_from(&ranked[0]);
                    Some(ranked)
                })
                .is_some()
        );
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&SlimeAction::Commit(expected))
        );
    }

    #[test]
    fn explicit_candidate_ranking_changes_preedit_and_commit_without_inventing_candidates() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        let request = engine.candidate_ranking_request().unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        ranked.reverse();

        let actions = engine
            .apply_candidate_ranking(&ranked)
            .expect("an exact permutation is accepted");
        let first = engine.snapshot().candidates[0].clone();
        assert!(actions.contains(&SlimeAction::UpdatePreedit(first.clone())));
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&SlimeAction::Commit(first))
        );
    }

    #[test]
    fn delayed_live_ranking_updates_only_the_matching_engine_generation() {
        let mut engine = ambiguous_precision_engine(UserData::default());
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.snapshot().preedit, "変換制度");

        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("a reliable LIVE preview should be snapshotable");
        assert!(!snapshot.target_is_literal());
        let request = snapshot
            .candidate_ranking_request()
            .expect("the worker snapshot should generate a bounded N-best");
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let precision = ranked
            .iter()
            .position(|surface| surface == "変換精度")
            .expect("the semantic alternative remains in the normal N-best");
        ranked.swap(0, precision);

        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit("変換精度".to_owned())])
        );
        assert_eq!(engine.snapshot().preedit, "変換精度");

        let stale_snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let stale_request = stale_snapshot.candidate_ranking_request().unwrap();
        let stale_ranked: Vec<_> = stale_request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        engine.handle(InputEvent::Character('w'));
        assert!(
            engine
                .apply_live_candidate_ranking(&stale_snapshot, &stale_request, &stale_ranked)
                .is_none(),
            "a result captured before the next key must not rewrite marked text"
        );
        assert_eq!(engine.snapshot().preedit, "変換精度w");
    }

    #[test]
    fn delayed_live_result_cannot_cross_cancel_and_identical_retyping() {
        let mut engine = ambiguous_precision_engine(UserData::default());
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "henkanseido");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let index = ranked.iter().position(|s| s == "変換精度").unwrap();
        ranked.swap(0, index);
        engine.handle(InputEvent::Escape);
        engine.handle(InputEvent::Escape);
        type_text(&mut engine, "henkanseido");
        assert_eq!(engine.preedit(), snapshot.display_surface);
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
        let fresh = engine.live_candidate_ranking_snapshot().unwrap();
        let fresh_request = fresh.candidate_ranking_request().unwrap();
        assert!(
            engine
                .apply_live_candidate_ranking(&fresh, &fresh_request, &ranked)
                .is_some()
        );
    }

    #[test]
    fn delayed_live_result_cannot_cross_restored_context_or_preferences() {
        for change_preferences in [false, true] {
            let mut engine = ambiguous_precision_engine(UserData::default());
            let preferences = EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            };
            engine.set_preferences(preferences);
            type_text(&mut engine, "henkanseido");
            let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
            let request = snapshot.candidate_ranking_request().unwrap();
            let mut ranked: Vec<_> = request
                .candidates
                .iter()
                .map(|c| c.surface.clone())
                .collect();
            let index = ranked.iter().position(|s| s == "変換精度").unwrap();
            ranked.swap(0, index);
            if change_preferences {
                engine.set_preferences(EnginePreferences {
                    private_mode: true,
                    ..preferences
                });
                engine.set_preferences(preferences);
            } else {
                engine.set_external_left_context("別の文脈");
                engine.set_external_left_context("");
            }
            assert_eq!(engine.preedit(), snapshot.display_surface);
            assert!(
                engine
                    .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                    .is_none()
            );
        }
    }

    #[test]
    fn delayed_live_ranking_survives_a_lattice_valid_suffix_extension() {
        let mut engine = ambiguous_precision_engine(UserData::default());
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "henkanseido");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let precision = ranked
            .iter()
            .position(|surface| surface == "変換精度")
            .unwrap();
        ranked.swap(0, precision);
        engine
            .apply_live_candidate_ranking(&snapshot, &request, &ranked)
            .unwrap();

        type_text(&mut engine, "wo");
        assert_eq!(
            engine.snapshot().preedit,
            "変換精度を",
            "a gated neural surface should remain while the extended surface stays in N-best"
        );
    }

    #[test]
    fn delayed_live_ranking_can_repair_a_protected_numeric_homophone() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "tentaishounoichi");
        assert_eq!(engine.snapshot().preedit, "点対称のいち");

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(snapshot.resolved_reading(), "てんたいしょうのいち");
        assert!(snapshot.target_is_literal());
        assert_eq!(request.reading, "てんたいしょうのいち");
        assert!(request.left_context.is_empty());

        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == "点対称の位置")
            .expect("the contextual homophone remains in the full-reading N-best");
        ranked.swap(0, position);

        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit("点対称の位置".to_owned())])
        );
    }

    #[test]
    fn delayed_live_ranking_converts_a_katakana_list_tail() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(
            &mut engine,
            "いじょうでとうじょう。さいごのなんかんにふさわしく、ふぁんた、げろっぱ",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "以上で登場。最後の難関にふさわしく、ファンタ、げろっぱ"
        );

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let expected_target = "登場。最後の難関にふさわしく、ファンタ、ゲロッパ";
        let position = ranked
            .iter()
            .position(|surface| surface == expected_target)
            .expect("the completed katakana list remains in the suffix N-best");
        ranked.swap(0, position);

        assert!(snapshot.request_matches_ranking_scope(&request));
        assert!(snapshot.reopened_request_changes_are_bounded(&request, expected_target));
        assert!(snapshot.ranked_target_is_safe(&request, expected_target));
        assert!(!rewrites_only_protected_literal_tail(
            snapshot.resolved_reading(),
            &snapshot.base_surface,
            "以上で登場。最後の難関にふさわしく、ファンタ、ゲロッパ"
        ));

        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(
                "以上で登場。最後の難関にふさわしく、ファンタ、ゲロッパ".to_owned()
            )])
        );
    }

    #[test]
    fn delayed_live_ranking_does_not_split_a_displayed_katakana_name_into_a_particle() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context(
            "シーズン中にブライアントが故障し、カンセコにチャンスが訪れたかと思われたが、ファームで中途半端な成績しか挙げていなかったため、近鉄は新",
        );
        type_text(
            &mut engine,
            "がいこくじんじぇしー・りーどをかくとくし、かれをぶらいあんとのかわりにばってきした",
        );
        let expected = "外国人ジェシー・リードを獲得し、彼をブライアントの代わりに抜擢した";
        let unsafe_split = "外国人ジェシー・リードを獲得し、彼をブライアンとの代わりに抜擢した";
        assert_eq!(engine.snapshot().preedit, expected);

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(
            snapshot.evaluation_base_target_surface_for_request(&request),
            Some(
                "がいこくじんじぇしー・りーどをかくとくし、かれをぶらいあんとのかわりにばってきした"
            ),
            "the worker logical base intentionally remains independent from the stable display"
        );
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == unsafe_split)
            .expect("the ambiguous particle split should remain rankable for model diagnostics");
        ranked.swap(0, position);

        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            None,
            "apply must validate the displayed katakana boundary, not only the logical base"
        );
        assert_eq!(engine.snapshot().preedit, expected);
    }

    #[test]
    fn delayed_live_ranking_repairs_a_reopened_two_kana_inflected_target() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "はじめてろじゃーずにあい");
        assert_eq!(engine.snapshot().preedit, "初めてロジャーズにあい");

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(snapshot.request_reopens_stable_prefix(&request));
        assert_ne!(
            request
                .candidates
                .first()
                .map(|candidate| candidate.surface.as_str()),
            Some("初めてロジャーズに会い"),
            "the relaxation is only for a contextual model move beyond the dictionary base"
        );
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let position = ranked
            .iter()
            .position(|surface| surface == "初めてロジャーズに会い")
            .expect("the full-reading request retains the contextual short verb");
        ranked.swap(0, position);

        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(
                "初めてロジャーズに会い".to_owned()
            )])
        );
        assert!(!super::rewrites_reopened_two_kana_inflected_target(
            "よる", "よる", "夜"
        ));
        assert!(!super::rewrites_reopened_two_kana_inflected_target(
            "なって",
            "なって",
            "成って"
        ));
    }

    #[test]
    fn delayed_live_ranking_uses_a_literal_base_behind_a_pending_prefix() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "ばすがくしろえ");
        assert_eq!(engine.snapshot().preedit, "バスがくしろえ");

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        assert!(snapshot.target_is_literal());
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "ばすがくしろえ");
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        assert_eq!(ranked.first().map(String::as_str), Some("バスが釧路絵"));
        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit("バスが釧路絵".to_owned())])
        );
    }

    #[test]
    fn delayed_live_ranking_recovers_a_specific_word_after_a_converted_prefix() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "だびにふされたいたい");
        assert_eq!(engine.snapshot().preedit, "荼毘に付されたいたい");

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        assert_eq!(
            ranked.first().map(String::as_str),
            Some("荼毘に付された遺体")
        );
        assert!(super::rewrites_literal_tail_as_specific_compound(
            "だびにふされたいたい",
            "荼毘に付されたいたい",
            "荼毘に付された遺体"
        ));
        assert!(snapshot.dictionary_base_repairs_specific_literal_tail(&request));
        assert!(
            snapshot
                .ranked_winner_keeps_display_target(&request, &["荼毘に付されたいたい".to_owned()])
        );
        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            Some(vec![SlimeAction::UpdatePreedit(
                "荼毘に付された遺体".to_owned()
            )])
        );
    }

    #[test]
    fn delayed_live_ranking_can_preserve_the_displayed_hiragana_candidate() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "おもろいんよな");
        assert_eq!(engine.snapshot().preedit, "おもろいんよな");

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let base_cost = request.candidates[0].cost;
        let literal = request
            .candidates
            .iter()
            .find(|candidate| candidate.surface == "おもろいんよな")
            .expect("the literal display remains rankable by the LIVE worker");
        assert_eq!(
            literal.cost,
            base_cost + super::LIVE_LITERAL_PRESERVATION_COST_GAP
        );

        let mut standalone_colloquial = SlimeEngine::bundled();
        standalone_colloquial.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut standalone_colloquial, "おもろい");
        let snapshot = standalone_colloquial
            .live_candidate_ranking_snapshot()
            .unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let literal = request
            .candidates
            .iter()
            .find(|candidate| candidate.surface == "おもろい")
            .expect("a literal colloquial form must remain rankable");
        assert_eq!(
            literal.cost,
            request.candidates[0].cost + super::LIVE_LITERAL_PRESERVATION_COST_GAP
        );

        let mut short_particle = SlimeEngine::bundled();
        short_particle.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut short_particle, "とか");
        let snapshot = short_particle.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(
            request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "とか"),
            "the short grammatical form must displace the last content candidate"
        );

        let mut short_content_word = SlimeEngine::bundled();
        short_content_word.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut short_content_word, "ほか");
        let snapshot = short_content_word
            .live_candidate_ranking_snapshot()
            .unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        if let Some(literal) = request
            .candidates
            .iter()
            .find(|candidate| candidate.surface == "ほか")
        {
            assert_ne!(
                literal.cost,
                request.candidates[0].cost + super::LIVE_LITERAL_PRESERVATION_COST_GAP,
                "short content words must keep the existing conversion policy"
            );
        }
    }

    #[test]
    fn delayed_live_ranking_preserves_contextual_hiragana_inside_a_literal_tail() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "らっしゅふぉーどはいたらおもろいんよな");
        assert_eq!(
            engine.snapshot().preedit,
            "ラッシュフォードはいたらおもろいんよな"
        );

        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.left_context, "ラッシュフォードは");
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        assert_eq!(
            ranked.first().map(String::as_str),
            Some("いたらオモロイんよな")
        );
        assert_eq!(
            engine.apply_live_candidate_ranking(&snapshot, &request, &ranked),
            None,
            "a delayed ranker must not katakana-convert the protected colloquial tail"
        );
    }

    #[test]
    fn delayed_live_ranking_scores_contextual_single_kana_safely() {
        let mut contextual_single = SlimeEngine::bundled();
        contextual_single.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        contextual_single.set_external_left_context("東京");
        type_text(&mut contextual_single, "と");
        assert_eq!(contextual_single.snapshot().preedit, "と");
        let snapshot = contextual_single
            .live_candidate_ranking_snapshot()
            .expect("a single kana may be ranked only with left context");
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(
            request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "都")
        );
        let literal = request
            .candidates
            .iter()
            .find(|candidate| candidate.surface == "と")
            .expect("contextual single-kana ranking must preserve hiragana");
        assert_eq!(
            literal.cost,
            request.candidates[0].cost + super::LIVE_LITERAL_PRESERVATION_COST_GAP
        );
        assert!(
            snapshot.dictionary_base_replaces_short_contextual_target(&request),
            "a rejected worker may still use the context-ranked dictionary base"
        );

        for (left_context, reading) in [("ラーメン", "とか"), ("精度が悪く", "なる")] {
            let mut protected = SlimeEngine::bundled();
            protected.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });
            protected.set_external_left_context(left_context);
            type_text(&mut protected, reading);
            let snapshot = protected
                .live_candidate_ranking_snapshot()
                .expect("short contextual target should expose a delayed request");
            let request = snapshot.candidate_ranking_request().unwrap();
            assert!(
                !snapshot.dictionary_base_replaces_short_contextual_target(&request),
                "{reading} must not bypass a delayed ranker rejection"
            );
        }

        let mut standalone_single = SlimeEngine::bundled();
        standalone_single.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        type_text(&mut standalone_single, "と");
        assert!(
            standalone_single
                .live_candidate_ranking_snapshot()
                .is_none(),
            "a standalone single kana must remain outside automatic LIVE ranking"
        );
    }

    #[test]
    fn provisional_romaji_does_not_consume_deferred_live_boundary() {
        let mut requests = Vec::new();
        for input in ["ぎんこうこうざのかいせつ", "ginkoukouzanokaisetsu"] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });
            engine.set_delayed_live_ranking_available(true);
            if input.is_ascii() {
                type_text(&mut engine, "ginkoukouza");
                let deferred = engine.deferred_fragile_reading.clone();
                assert_eq!(deferred.as_deref(), Some("ぎんこうこうざ"));
                type_text(&mut engine, "n");
                assert_eq!(engine.deferred_fragile_reading, deferred);
                type_text(&mut engine, "o");
                assert!(engine.deferred_fragile_reading.is_none());
                type_text(&mut engine, "kaisetsu");
            } else {
                type_text(&mut engine, input);
            }
            let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
            let request = snapshot.candidate_ranking_request().unwrap();
            assert_eq!(request.reading, "ぎんこうこうざのかいせつ");
            assert!(request.left_context.is_empty());
            requests.push(request);
        }
        assert_eq!(requests[0], requests[1]);
    }

    #[test]
    fn delayed_live_ranking_reopens_a_supported_tenran_homophone() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(
            &mut engine,
            "karakan'pakutakatukasamasamitiwotuuzite、koumeiten'nounoten'ran'nikyouserareta",
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(snapshot.request_reopens_stable_prefix(&request));
        assert!(request.candidates.iter().any(|candidate| {
            candidate.surface == "から関白鷹司政通を通じて、孝明天皇の天覧に供せられた"
        }));
    }

    #[test]
    fn experimental_boundary_reopen_preserves_recent_selection_and_reload_generation() {
        let input = "のじどうひきおとし）でてすうりょうをむりょうかするなどのとくてんをもうけます";
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_learning: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(&mut engine, input);
        let mut old = engine.live_candidate_ranking_snapshot().unwrap();
        assert!(old.prepare_particle_boundary_reopen());
        let request = old.candidate_ranking_request().unwrap();
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        engine.record_recent_live_selection("もうけます", "儲けます");
        assert!(engine.recent_live_selection_surface("もうけます").is_some());
        let mut current = engine.live_candidate_ranking_snapshot().unwrap();
        assert!(!current.prepare_particle_boundary_reopen());
        engine.reload_user_data();
        assert!(
            engine
                .apply_live_candidate_ranking(&old, &request, &ranked)
                .is_none()
        );
    }

    #[test]
    fn experimental_boundary_reopen_preserves_learned_readings() {
        for history_completion in [false, true] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion,
                ..EnginePreferences::default()
            });
            engine.set_delayed_live_ranking_available(true);
            type_text(
                &mut engine,
                "のじどうひきおとし）でてすうりょうをむりょうかするなどのとくてんをもうけ",
            );
            engine.user_data.record("もうけ", "儲け");
            let mut snapshot = engine.live_candidate_ranking_snapshot().unwrap();
            let original_target = snapshot.target_reading.clone();
            assert_eq!(
                snapshot.prepare_particle_boundary_reopen(),
                !history_completion
            );
            if history_completion {
                assert_eq!(snapshot.target_reading, original_target);
            }
        }
    }

    #[test]
    fn experimental_boundary_winner_rejects_an_unrelated_later_word() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(&mut engine, "むかしながらのまちなみだけではなく");
        let mut snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        assert!(snapshot.prepare_particle_boundary_reopen());
        assert_eq!(snapshot.target_reading, "がらのまちなみだけではなく");
        let candidate = "がらの街並みだけではなく";
        assert!(!snapshot.boundary_winner_uses_reopened_edge(candidate));
        // Supply the real deeper path to verify rejection is structural,
        // not merely caused by the bounded lookup omitting this candidate.
        let deeper = snapshot
            .dictionary
            .convert_n_best(&snapshot.target_reading, 64);
        let path = deeper
            .into_iter()
            .find(|path| path.surface == candidate)
            .unwrap();
        snapshot.boundary_paths.push(path);
        assert!(!snapshot.boundary_winner_uses_reopened_edge(candidate));
    }

    #[test]
    fn experimental_boundary_apply_diagnostics() {
        for (input, expected) in [
            ("おさとうとおす", "とお酢"),
            (
                "のじどうひきおとし）でてすうりょうをむりょうかするなどのとくてんをもうけ",
                "設け",
            ),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });
            engine.set_delayed_live_ranking_available(true);
            type_text(&mut engine, input);
            let mut snapshot = engine.live_candidate_ranking_snapshot().unwrap();
            assert!(snapshot.prepare_particle_boundary_reopen());
            let request = snapshot.candidate_ranking_request().unwrap();
            let base = snapshot.base_target_surface_for_request(&request).unwrap();
            let surface = format!("{}{}", snapshot.prefix.as_ref().unwrap().surface, expected);
            eprintln!(
                "{expected}: scope={} base={base} identity={} bounded={} target_safe={} protected_tail={}",
                request.reading,
                engine.matches_live_candidate_snapshot(&snapshot),
                snapshot.reopened_request_changes_are_bounded(&request, expected),
                snapshot.ranked_target_is_safe(&request, expected),
                super::rewrites_only_protected_literal_tail(
                    &snapshot.resolved_reading,
                    &snapshot.base_surface,
                    &surface
                )
            );
            assert!(snapshot.boundary_word_repair_is_safe(&request, expected));
            assert!(snapshot.boundary_winner_can_apply(&request, expected));
            let mut already_displayed = snapshot.clone();
            already_displayed.display_surface = surface.clone();
            assert!(!already_displayed.boundary_winner_can_apply(&request, expected));
            let mut ranked: Vec<_> = request
                .candidates
                .iter()
                .map(|c| c.surface.clone())
                .collect();
            let index = ranked.iter().position(|s| s == expected).unwrap();
            ranked.swap(0, index);
            assert!(
                engine
                    .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                    .is_some()
            );
            assert_eq!(engine.snapshot().preedit, surface);
        }
    }

    #[test]
    fn verb_auxiliary_repair_requires_body_support_and_fresh_request() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(&mut engine, "このしょうひんをかいたい");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "かいたい");
        assert_eq!(request.left_context, "この商品を");
        assert!(request.candidates.iter().any(|c| c.surface == "飼いたい"));
        let index = request
            .candidates
            .iter()
            .position(|c| c.surface == "買いたい")
            .unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        ranked.swap(0, index);
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
        let mut full = vec![-10.0; request.candidates.len()];
        full[index] = -1.0;
        let weak_body = vec![-10.0; request.candidates.len()];
        assert!(!snapshot.approve_model_supported_verb_auxiliary(
            &request,
            "買いたい",
            &full,
            &weak_body
        ));
        assert!(!snapshot.approve_model_supported_verb_auxiliary(&request, "買いたい", &[], &full));
        assert!(!snapshot.approve_model_supported_verb_auxiliary(
            &request,
            "買いたい",
            &full,
            &vec![f64::NAN; full.len()]
        ));
        assert!(snapshot.approve_model_supported_verb_auxiliary(
            &request,
            "買いたい",
            &full,
            &full
        ));
        let mut altered = request.clone();
        altered.candidates[index].cost += 1;
        assert!(!snapshot.verb_auxiliary_repair_is_safe(&altered, "買いたい"));
        assert!(!snapshot.verb_auxiliary_repair_is_safe(&request, "飼いたい"));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, "この商品を買いたい");
        type_text(&mut engine, "です");
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
    }

    #[test]
    fn object_inflection_repair_requires_a_dictionary_word_and_fresh_snapshot() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(
            &mut engine,
            "りくじょうきょうぎ、すいそうがくにちからをいれ",
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let selected = if snapshot.request_reopens_stable_prefix(&request) {
            format!("{}入れ", snapshot.prefix.as_ref().unwrap().surface)
        } else {
            "入れ".to_owned()
        };
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let index = ranked.iter().position(|s| s == &selected).unwrap();
        ranked.swap(0, index);
        assert!(snapshot.object_inflection_paths.get().is_none());
        snapshot.prepare_ranked_prefix_validation(&request, &ranked);
        assert!(snapshot.object_inflection_paths.get().is_some());
        assert!(!snapshot.object_inflection_repair_is_safe(&request, &selected));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
        snapshot.approve_model_supported_object_inflection(&request, &selected, &[]);
        let invalid_scores = vec![f64::NAN; request.candidates.len()];
        snapshot.approve_model_supported_object_inflection(&request, &selected, &invalid_scores);
        assert!(!snapshot.object_inflection_repair_is_safe(&request, &selected));
        let mut scores = vec![-10.0; request.candidates.len()];
        scores[index] = -20.0;
        snapshot.approve_model_supported_object_inflection(&request, &selected, &scores);
        assert!(!snapshot.object_inflection_repair_is_safe(&request, &selected));
        scores[index] = -1.0;
        snapshot.approve_model_supported_object_inflection(&request, &selected, &scores);
        assert!(snapshot.object_inflection_repair_is_safe(&request, &selected));
        let mut foreign_context = request.clone();
        foreign_context.left_context.push('別');
        assert!(!snapshot.object_inflection_repair_is_safe(&foreign_context, &selected));
        assert!(!snapshot.object_inflection_repair_is_safe(&request, "偽れ"));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert!(engine.snapshot().preedit.ends_with("力を入れ"));
        type_text(&mut engine, "る");
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
    }

    #[test]
    fn experimental_particle_reopen_preserves_the_original_segmentation() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(&mut engine, "おさとうとおす");
        let mut snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        assert!(snapshot.prepare_particle_boundary_reopen());
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "とおす");
        let candidate = request
            .candidates
            .iter()
            .find(|candidate| candidate.surface == "とお酢")
            .unwrap();
        let paths = snapshot
            .dictionary
            .convert_n_best("とおす", super::LIVE_RANKING_CANDIDATE_LIMIT);
        let path = paths.iter().find(|path| path.surface == "とお酢").unwrap();
        assert_eq!(candidate.cost, path.cost);
        assert!(
            snapshot
                .rankable_output_surfaces(&request)
                .unwrap()
                .contains(&"お砂糖とお酢".to_owned())
        );
    }

    #[test]
    fn experimental_particle_reopen_preserves_snapshot_authority() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(
            &mut engine,
            "のじどうひきおとし）でてすうりょうをむりょうかするなどのとくてんをもうけ",
        );
        let mut snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let original = snapshot.prefix.clone().unwrap();
        assert!(snapshot.prepare_particle_boundary_reopen());
        assert!(!snapshot.prepare_particle_boundary_reopen());
        assert_eq!(
            snapshot.original_prefix_before_boundary_reopen.as_ref(),
            Some(&original)
        );
        assert!(engine.matches_live_candidate_snapshot(&snapshot));
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "もうけ");
        assert!(request.left_context.ends_with("特典を"));
        assert!(
            request
                .candidates
                .iter()
                .any(|candidate| candidate.surface == "設け")
        );
        type_text(&mut engine, "る");
        assert!(!engine.matches_live_candidate_snapshot(&snapshot));
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
    }

    #[test]
    fn reopened_live_ranking_repairs_two_dictionary_words_in_the_prefix() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(
            &mut engine,
            "きこうはねんかんをつうじてひじょうにかんれいであり、なつでもひょうてんをこえることはなく",
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let expected = "気候は年間を通じて非常に寒冷であり、夏でも氷点を超えることはなく";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let position = ranked.iter().position(|s| s == expected).unwrap();
        ranked.swap(0, position);
        assert!(snapshot.prefix_repair_paths.get().is_none());
        snapshot.prepare_ranked_prefix_validation(&request, &ranked);
        assert!(snapshot.prefix_repair_paths.get().is_some());
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, expected);
    }

    #[test]
    fn two_word_prefix_repair_preserves_dictionary_readings_and_kana() {
        let conversion = |parts: &[(&str, &str)]| slime_converter::Conversion {
            surface: parts.iter().map(|(_, surface)| *surface).collect(),
            segments: parts
                .iter()
                .map(|(reading, surface)| slime_converter::Segment {
                    reading: (*reading).to_owned(),
                    surface: (*surface).to_owned(),
                    cost: 0,
                })
                .collect(),
            cost: 0,
        };
        let base = conversion(&[("きこう", "機構"), ("と", "と"), ("ひょうてん", "評点")]);
        let accepts = |parts: &[(&str, &str)]| {
            let candidate = conversion(parts);
            super::live_conversion::bounded_two_word_kanji_difference(
                &base.surface,
                &candidate.surface,
            ) && super::live_conversion::two_word_kanji_surface_difference(
                &[base.clone(), candidate.clone()],
                &base.surface,
                &candidate.surface,
            )
        };
        assert!(accepts(&[
            ("きこう", "気候"),
            ("と", "と"),
            ("ひょうてん", "氷点")
        ]));
        assert!(!accepts(&[
            ("き", "気候"),
            ("こうと", "と"),
            ("ひょうてん", "氷点")
        ]));
        assert!(!accepts(&[
            ("きこう", "気候"),
            ("と", "戸"),
            ("ひょうてん", "氷点")
        ]));
        assert!(!accepts(&[
            ("きこう", "気候"),
            ("と", "と"),
            ("ひょうてん", "氷点下")
        ]));
        assert!(!accepts(&[("きこう", "気候と氷点")]));
    }

    #[test]
    fn joint_repair_requires_one_word_on_each_side_of_a_dictionary_boundary() {
        let conversion = |parts: &[(&str, &str)]| slime_converter::Conversion {
            surface: parts.iter().map(|(_, surface)| *surface).collect(),
            segments: parts
                .iter()
                .map(|(reading, surface)| slime_converter::Segment {
                    reading: (*reading).to_owned(),
                    surface: (*surface).to_owned(),
                    cost: 0,
                })
                .collect(),
            cost: 0,
        };
        let base = conversion(&[("こうざ", "講座"), ("を", "を"), ("かいせつ", "解説")]);
        let valid = conversion(&[("こうざ", "口座"), ("を", "を"), ("かいせつ", "開設")]);
        let prefix_only = conversion(&[("こうざ", "口座"), ("を", "を"), ("かいせつ", "解説")]);
        assert!(super::live_conversion::two_scope_kanji_surface_difference(
            &[base.clone(), valid],
            "講座を解説",
            "口座を開設",
            "こうざを",
            "講座を",
        ));
        assert!(!super::live_conversion::two_scope_kanji_surface_difference(
            &[base, prefix_only],
            "講座を解説",
            "口座を解説",
            "こうざを",
            "講座を",
        ));
        assert!(!super::live_conversion::two_scope_kanji_surface_difference(
            &[
                conversion(&[("こうざをかい", "講座を解"), ("せつ", "説")]),
                conversion(&[("こうざをかい", "口座を開"), ("せつ", "設")]),
            ],
            "講座を解説",
            "口座を開設",
            "こうざを",
            "講座を",
        ));
        assert!(!super::live_conversion::two_scope_kanji_surface_difference(
            &[
                conversion(&[
                    ("とうし", "投資"),
                    ("こうざ", "講座"),
                    ("を", "を"),
                    ("かいせつ", "解説")
                ]),
                conversion(&[
                    ("とうし", "透視"),
                    ("こうざ", "口座"),
                    ("を", "を"),
                    ("かいせつ", "開設")
                ]),
            ],
            "投資講座を解説",
            "透視口座を開設",
            "とうしこうざを",
            "投資講座を",
        ));
    }

    #[test]
    fn reopened_live_ranking_can_repair_one_kanji_word_in_each_scope() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(&mut engine, "とうししんたくこうざをかいせつ");
        assert_eq!(engine.snapshot().preedit, "投資信託講座を解説");
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let expected = "投資信託口座を開設";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let index = ranked
            .iter()
            .position(|surface| surface == expected)
            .unwrap();
        ranked.swap(0, index);
        snapshot.prepare_ranked_prefix_validation(&request, &ranked);
        assert!(snapshot.joint_repair_paths.get().is_some());
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, expected);
    }

    #[test]
    fn inflected_stable_repair_requires_one_dictionary_segment_with_the_same_reading() {
        let conversion = |parts: &[(&str, &str)]| slime_converter::Conversion {
            surface: parts.iter().map(|(_, surface)| *surface).collect(),
            segments: parts
                .iter()
                .map(|(reading, surface)| slime_converter::Segment {
                    reading: (*reading).to_owned(),
                    surface: (*surface).to_owned(),
                    cost: 0,
                })
                .collect(),
            cost: 0,
        };
        let base = conversion(&[("てきし", "適し"), ("し", "し")]);
        let valid = conversion(&[("てきし", "敵視"), ("し", "し")]);
        let shifted = conversion(&[("てき", "敵"), ("しし", "視し")]);
        let accepts = |paths: &[slime_converter::Conversion]| {
            super::live_conversion::single_segment_inflected_surface_difference(
                paths,
                "適しし",
                "敵視し",
            )
        };
        assert!(accepts(&[base.clone(), valid]));
        assert!(!accepts(&[base.clone(), shifted]));
        assert!(!accepts(&[base]));
        assert!(
            !super::live_conversion::single_segment_inflected_surface_difference(
                &[
                    conversion(&[("てきし", "適し"), ("てきし", "適し")]),
                    conversion(&[("てきし", "敵視"), ("てきし", "敵視")]),
                ],
                "適し適し",
                "敵視敵視",
            )
        );
        assert!(
            !super::live_conversion::single_segment_inflected_surface_difference(
                &[
                    conversion(&[("てきし", "てきし")]),
                    conversion(&[("てきし", "敵視")]),
                ],
                "てきし",
                "敵視",
            )
        );
    }

    #[test]
    fn reopened_live_prefix_repairs_a_dictionary_aligned_inflected_homophone() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        type_text(
            &mut engine,
            "じぶんのしそうをりかいできないじんるいをてきしし、せかいせいふくをたくらむ",
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(snapshot.request_reopens_stable_prefix(&request));
        let expected = "自分の思想を理解できない人類を敵視し、世界征服を企む";
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let index = ranked
            .iter()
            .position(|surface| surface == expected)
            .unwrap();
        ranked.swap(0, index);
        assert!(snapshot.prefix_repair_paths.get().is_none());
        snapshot.prepare_ranked_prefix_validation(&request, &ranked);
        assert!(snapshot.prefix_repair_paths.get().is_some());
        let mut restarted = engine.clone();
        restarted.handle(InputEvent::Escape);
        restarted.handle(InputEvent::Escape);
        type_text(
            &mut restarted,
            "じぶんのしそうをりかいできないじんるいをてきしし、せかいせいふくをたくらむ",
        );
        assert_eq!(restarted.snapshot().preedit, engine.snapshot().preedit);
        assert!(
            restarted
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_none()
        );
        assert!(snapshot.reopened_request_changes_are_bounded(&request, expected));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, expected);
    }

    #[test]
    fn live_conversion_keeps_dekinai_in_one_ranking_scope() {
        for input in [
            "よやくできない",
            "yoyakudekinai",
            "へんかんできない",
            "henkandekinai",
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });
            engine.set_delayed_live_ranking_available(true);
            type_text(&mut engine, input);
            assert!(engine.snapshot().preedit.ends_with("できない"), "{input}");
            let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
            let request = snapshot.candidate_ranking_request().unwrap();
            assert!(request.reading.contains("できない"), "{input}: {request:?}");
        }
        for (input, expected) in [
            ("ここできめる", "ここで決める"),
            ("へやできがえる", "部屋で着替える"),
            ("みせできものをかう", "店で着物を買う"),
        ] {
            let mut engine = SlimeEngine::bundled();
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                ..EnginePreferences::default()
            });
            engine.set_delayed_live_ranking_available(true);
            type_text(&mut engine, input);
            assert_eq!(engine.snapshot().preedit, expected, "{input}");
        }
    }

    #[test]
    fn live_conversion_keeps_tokoro_in_one_ranking_scope() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        engine.set_external_left_context("出力１５０万馬力でマッハ２で空を飛び、目からは厚さ２０ｃｍの鉄板を切断するレーザーを出し、口からは火炎放射を吐くのだが、いっぺんに全能力");
        type_text(
            &mut engine,
            "wotukauto、suguniden'tigireninaxtutesimautokorogatamanikizu",
        );
        assert_eq!(
            engine.snapshot().preedit,
            "を使うと、すぐに電池切れになってしまうところが玉に瑕"
        );
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert!(request.reading.contains("ところ"));
    }

    #[test]
    fn delayed_live_ranking_uses_decisive_short_contextual_base_without_model() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("全ては裁判");
        type_text(&mut engine, "かん");
        assert_eq!(engine.snapshot().preedit, "感");
        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("a converted short target should retain its contextual request");
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(
            request
                .candidates
                .first()
                .map(|candidate| candidate.surface.as_str()),
            Some("官")
        );
        assert!(snapshot.dictionary_base_replaces_short_contextual_target(&request));
    }

    #[test]
    fn delayed_live_ranking_applies_a_lexicalized_particle_looking_word() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("同社の不燃木材は浅野木材工業の");
        type_text(&mut engine, "あさの");
        assert_eq!(engine.snapshot().preedit, "朝の");
        let snapshot = engine
            .live_candidate_ranking_snapshot()
            .expect("a lexicalized particle-looking target should remain rankable");
        let request = snapshot.candidate_ranking_request().unwrap();
        let ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        assert_eq!(ranked.first().map(String::as_str), Some("浅野"));
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        assert_eq!(engine.snapshot().preedit, "浅野");
    }

    #[test]
    fn delayed_live_ranking_is_unavailable_in_private_mode() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            private_mode: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "henkanseido");
        assert!(engine.live_candidate_ranking_snapshot().is_none());
    }

    #[test]
    fn explicit_candidate_ranking_rejects_incomplete_duplicate_or_unknown_orders() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        let request = engine.candidate_ranking_request().unwrap();
        let original = engine.snapshot();
        let mut missing: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        missing.pop();
        assert!(engine.apply_candidate_ranking(&missing).is_none());
        let duplicate = vec![request.candidates[0].surface.clone(); request.candidates.len()];
        assert!(engine.apply_candidate_ranking(&duplicate).is_none());
        let mut unknown: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        unknown[0] = "候補にない表記".to_owned();
        assert!(engine.apply_candidate_ranking(&unknown).is_none());
        assert_eq!(engine.snapshot(), original);
    }

    #[test]
    fn explicit_candidate_ranking_keeps_history_in_its_protected_position() {
        let directory = test_directory("ranking-protected-history");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t感じ\t3\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().candidates[0], "感じ");
        let request = engine.candidate_ranking_request().unwrap();
        assert!(
            request
                .candidates
                .iter()
                .all(|candidate| candidate.surface != "感じ")
        );
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        ranked.reverse();
        engine.apply_candidate_ranking(&ranked).unwrap();
        assert_eq!(engine.snapshot().candidates[0], "感じ");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_mode_never_exposes_candidate_ranking_input() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            private_mode: true,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        assert!(engine.candidate_ranking_request().is_none());
    }

    #[test]
    fn selecting_out_of_range_candidate_does_nothing() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let snapshot = engine.snapshot();

        assert!(
            engine
                .handle(InputEvent::SelectCandidate(u32::MAX))
                .is_empty()
        );
        assert_eq!(engine.snapshot(), snapshot);
    }

    #[test]
    fn enter_commits_selected_candidate_and_clears_state() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let actions = engine.handle(InputEvent::Enter);

        assert!(actions.contains(&SlimeAction::Commit("日本".to_owned())));
        assert_eq!(engine.snapshot().preedit, "");
    }

    #[test]
    fn escape_restores_reading_after_conversion() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        engine.handle(InputEvent::Escape);

        assert_eq!(engine.snapshot().preedit, "にほん");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
    }

    #[test]
    fn phrase_uses_segmented_conversion() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihon");

        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "私は日本");
    }

    #[test]
    fn backspace_removes_pending_then_committed_kana() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "kak");
        assert_eq!(engine.snapshot().preedit, "かk");

        engine.handle(InputEvent::Backspace);
        assert_eq!(engine.snapshot().preedit, "か");
        engine.handle(InputEvent::Backspace);
        assert_eq!(engine.snapshot().preedit, "");
    }

    #[test]
    fn standard_function_key_transforms_keep_the_same_composition() {
        let cases = [
            (InputEvent::TransformHiragana, "にほんご"),
            (InputEvent::TransformFullKatakana, "ニホンゴ"),
            (InputEvent::TransformHalfKatakana, "ﾆﾎﾝｺﾞ"),
            (InputEvent::TransformFullAlphanumeric, "ｎｉｈｏｎｇｏ"),
            (InputEvent::TransformHalfAlphanumeric, "nihongo"),
        ];
        for (event, expected) in cases {
            let mut engine = SlimeEngine::bundled();
            type_text(&mut engine, "nihongo");
            let actions = engine.handle(event);
            assert!(actions.contains(&SlimeAction::UpdatePreedit(expected.to_owned())));
            assert!(
                engine
                    .handle(InputEvent::Enter)
                    .contains(&SlimeAction::Commit(expected.to_owned()))
            );
        }

        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "Slime");
        assert!(
            engine
                .handle(InputEvent::TransformFullAlphanumeric)
                .contains(&SlimeAction::UpdatePreedit("Ｓｌｉｍｅ".to_owned()))
        );
    }

    #[test]
    fn raw_keys_after_a_kana_backspace_never_replace_the_whole_reading() {
        // Keys typed after a deleted kana spell only the tail of the reading.
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "aiu");
        engine.handle(InputEvent::Backspace);
        type_text(&mut engine, "e");
        assert!(
            engine
                .handle(InputEvent::TransformHalfAlphanumeric)
                .contains(&SlimeAction::UpdatePreedit("aie".to_owned()))
        );

        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "wata");
        engine.handle(InputEvent::Backspace);
        type_text(&mut engine, "nihpn");
        assert_eq!(engine.snapshot().preedit, "わにhpn");
        let actions = engine.handle(InputEvent::Space);
        assert!(
            shown_candidate_details(&actions)
                .iter()
                .all(|detail| detail.annotation != CandidateAnnotation::Correction),
            "a correction of the raw tail would drop わ"
        );

        // Deleting the whole reading makes the raw keys reliable again.
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "wa");
        engine.handle(InputEvent::Backspace);
        type_text(&mut engine, "sute");
        assert!(
            engine
                .handle(InputEvent::TransformHalfAlphanumeric)
                .contains(&SlimeAction::UpdatePreedit("sute".to_owned()))
        );
    }

    #[test]
    fn segment_navigation_and_resizing_preserve_the_complete_reading() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihon");
        engine.handle(InputEvent::Space);

        let actions = engine.handle(InputEvent::PreviousSegment);
        assert!(matches!(
            actions.first(),
            Some(SlimeAction::UpdateSegmentedPreedit { .. })
        ));
        assert!(
            engine.segments.len() >= 2,
            "segments: {:?}",
            engine.segments
        );
        let reading = engine
            .segments
            .iter()
            .map(|segment| segment.reading.as_str())
            .collect::<String>();
        assert_eq!(reading, "わたしはにほん");

        engine.handle(InputEvent::ShrinkSegment);
        let shrunk = engine
            .segments
            .iter()
            .map(|segment| segment.reading.as_str())
            .collect::<String>();
        assert_eq!(shrunk, "わたしはにほん");
        engine.handle(InputEvent::ExpandSegment);
        let expanded = engine
            .segments
            .iter()
            .map(|segment| segment.reading.as_str())
            .collect::<String>();
        assert_eq!(expanded, "わたしはにほん");
    }

    #[test]
    fn segment_operations_always_show_the_active_segment_candidates() {
        fn assert_active_segment_is_shown(engine: &SlimeEngine, event: InputEvent) {
            let segment = &engine.segments[engine.active_segment];
            let expected = engine.conversion_candidates(&segment.reading);
            assert_eq!(
                engine.candidates[engine.selected], segment.surface,
                "{event:?}: {:?}",
                engine.segments
            );
            assert!(
                engine
                    .candidates
                    .iter()
                    .all(|candidate| expected.contains(candidate) || *candidate == segment.surface),
                "{event:?}: {:?} for {}",
                engine.candidates,
                segment.reading
            );
        }

        use InputEvent::{ExpandSegment, NextSegment, PreviousSegment, ShrinkSegment};
        // Entering segment mode through every operation, including resizes
        // that cannot apply to a one-segment phrase.
        for (input, event) in [
            ("watashihanihon", NextSegment),
            ("watashihanihon", PreviousSegment),
            ("watashihanihon", ExpandSegment),
            ("watashihanihon", ShrinkSegment),
            ("nihon", ExpandSegment),
            ("ha", ShrinkSegment),
            // A one-kana first segment of a longer phrase cannot shrink.
            ("mewomiru", ShrinkSegment),
        ] {
            let mut engine = SlimeEngine::bundled();
            type_text(&mut engine, input);
            engine.handle(InputEvent::Space);
            engine.handle(event);
            assert_eq!(
                engine.candidate_kind,
                Some(CandidateKind::SegmentedConversion)
            );
            assert_active_segment_is_shown(&engine, event);
        }

        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihon");
        engine.handle(InputEvent::Space);
        for event in [
            ExpandSegment,
            NextSegment,
            ShrinkSegment,
            ShrinkSegment,
            ExpandSegment,
            NextSegment,
            NextSegment,
            ExpandSegment,
            PreviousSegment,
            ShrinkSegment,
        ] {
            engine.handle(event);
            assert_active_segment_is_shown(&engine, event);
            assert_eq!(
                engine
                    .segments
                    .iter()
                    .map(|segment| segment.reading.as_str())
                    .collect::<String>(),
                "わたしはにほん"
            );
        }
    }

    #[test]
    fn segmented_candidates_update_only_the_active_segment() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihon");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::PreviousSegment);

        assert_eq!(
            engine.candidate_kind,
            Some(CandidateKind::SegmentedConversion)
        );
        assert!(
            engine.segments.len() >= 2,
            "segments: {:?}",
            engine.segments
        );
        let trailing = engine.segments[1..]
            .iter()
            .map(|segment| segment.surface.as_str())
            .collect::<String>();

        let actions = engine.handle(InputEvent::Space);
        assert!(matches!(
            actions.first(),
            Some(SlimeAction::UpdateSegmentedPreedit { .. })
        ));
        assert_eq!(
            engine.segments[0].surface,
            engine.candidates[engine.selected]
        );
        assert_eq!(
            engine.segments[1..]
                .iter()
                .map(|segment| segment.surface.as_str())
                .collect::<String>(),
            trailing
        );

        let actions = engine.handle(InputEvent::SelectCandidate(0));
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, SlimeAction::ShowCandidates { selected: 0, .. }))
        );

        let actions = engine.handle(InputEvent::TransformFullKatakana);
        assert!(matches!(
            actions.as_slice(),
            [
                SlimeAction::UpdateSegmentedPreedit { .. },
                SlimeAction::ShowCandidates { .. }
            ]
        ));
        assert_eq!(engine.phase(), Phase::Converting);
    }

    #[test]
    fn segmented_selection_is_reused_for_the_word_in_another_phrase() {
        let directory = test_directory("segmented-word-learning");
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };

        let mut baseline = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        baseline.set_preferences(preferences);
        type_text(&mut baseline, "kaitou");
        baseline.handle(InputEvent::Space);
        assert_eq!(baseline.snapshot().preedit, "回答");

        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);
        type_text(&mut engine, "kaitouhanihon");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::PreviousSegment);
        assert_eq!(engine.segments[0].reading, "かいとう");
        let selected = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == "解答")
            .expect("segment candidate 解答");
        engine.handle(InputEvent::SelectCandidate(
            u32::try_from(selected).unwrap(),
        ));
        engine.handle(InputEvent::Enter);

        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(
            history
                .lines()
                .any(|line| line.starts_with("かいとう\t解答\t")),
            "explicit segment selection must be learned: {history}"
        );
        assert!(
            !history
                .lines()
                .any(|line| line.starts_with("にほん\t日本\t")),
            "untouched segments must not be learned: {history}"
        );

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        type_text(&mut reloaded, "kaitou");
        reloaded.handle(InputEvent::Space);
        assert_eq!(reloaded.snapshot().preedit, "解答");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typing_after_segmented_selection_learns_before_starting_new_input() {
        let directory = test_directory("segmented-word-auto-commit");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });
        type_text(&mut engine, "kaitouhanihon");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::PreviousSegment);
        let selected = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == "解答")
            .expect("segment candidate 解答");
        engine.handle(InputEvent::SelectCandidate(
            u32::try_from(selected).unwrap(),
        ));

        let actions = engine.handle(InputEvent::Character('a'));
        assert!(actions.contains(&SlimeAction::Commit("解答は日本".to_owned())));
        assert_eq!(engine.snapshot().preedit, "あ");
        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(
            history
                .lines()
                .any(|line| line.starts_with("かいとう\t解答\t")),
            "auto-commit must retain the explicit segment selection: {history}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_segment_selection_learns_its_local_phrase_context() {
        let directory = test_directory("segmented-context-learning");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかいとう\t回答\t100\t10\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        };
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        for repetition in 0..2 {
            engine.reset_context();
            type_text(&mut engine, "nihonnnokaitouhanihon");
            engine.handle(InputEvent::Space);
            engine.handle(InputEvent::PreviousSegment);
            let segment_index = engine
                .segments
                .iter()
                .position(|segment| segment.reading == "かいとう")
                .unwrap_or_else(|| panic!("かいとう segment: {:?}", engine.segments));
            while engine.active_segment < segment_index {
                engine.handle(InputEvent::NextSegment);
            }
            let selected = engine
                .snapshot()
                .candidates
                .iter()
                .position(|candidate| candidate == "解答")
                .expect("segment candidate 解答");
            engine.handle(InputEvent::SelectCandidate(
                u32::try_from(selected).unwrap(),
            ));
            engine.handle(InputEvent::Enter);

            if repetition == 0 {
                let mut one_off = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
                one_off.set_preferences(preferences);
                one_off.set_external_left_context("これは日本の");
                type_text(&mut one_off, "kaitou");
                one_off.handle(InputEvent::Space);
                assert_eq!(one_off.snapshot().preedit, "回答");
            }
        }

        let context = fs::read_to_string(directory.join("context_history.tsv")).unwrap();
        assert!(
            context
                .lines()
                .any(|line| line.starts_with("にほんの\t日本の\tかいとう\t解答\t2\t")),
            "the shortest useful segment-prefix context must be learned: {context}"
        );

        let mut baseline = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        baseline.set_preferences(preferences);
        type_text(&mut baseline, "kaitou");
        baseline.handle(InputEvent::Space);
        assert_eq!(baseline.snapshot().preedit, "回答");

        let mut contextual = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        contextual.set_preferences(preferences);
        contextual.set_external_left_context("これは日本の");
        type_text(&mut contextual, "kaitou");
        contextual.handle(InputEvent::Space);
        assert_eq!(contextual.snapshot().preedit, "解答");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn escape_leaves_segment_mode_and_restores_the_complete_reading() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihon");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::NextSegment);

        engine.handle(InputEvent::Escape);

        assert_eq!(engine.snapshot().preedit, "わたしはにほん");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
    }

    #[test]
    fn reconversion_uses_the_reverse_dictionary_without_changing_unknown_text() {
        let mut engine = SlimeEngine::bundled();
        let actions = engine.begin_reconversion("日本");
        assert_eq!(engine.reading, "にほん");
        assert!(actions.iter().any(|action| matches!(
            action,
            SlimeAction::ShowCandidates { candidates, .. }
                if candidates.iter().any(|candidate| candidate == "日本")
        )));

        let snapshot = engine.snapshot();
        assert!(engine.begin_reconversion("🫠").is_empty());
        assert_eq!(engine.snapshot(), snapshot);
    }

    #[test]
    fn reconversion_includes_user_dictionary_readings() {
        let directory = test_directory("reconversion-user-dictionary");
        fs::write(
            directory.join("user_dictionary.tsv"),
            "# slime-user-dictionary-v1\nすらいむてすと\tSlimeTest\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        let actions = engine.begin_reconversion("SlimeTest");

        assert_eq!(engine.reading, "すらいむてすと");
        assert!(actions.iter().any(|action| matches!(
            action,
            SlimeAction::ShowCandidates { candidates, .. }
                if candidates.iter().any(|candidate| candidate == "SlimeTest")
        )));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reconversion_does_not_attach_a_selection_elsewhere_to_the_previous_commit() {
        let directory = test_directory("reconversion-context-boundary");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        convert_and_commit(&mut engine, "bunshou", "文章");
        assert!(!engine.begin_reconversion("漢字").is_empty());
        engine.handle(InputEvent::Enter);

        assert!(!directory.join("context_history.tsv").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_reconversion_still_breaks_the_previous_commit_boundary() {
        let directory = test_directory("failed-reconversion-context-boundary");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        convert_and_commit(&mut engine, "bunshou", "文章");
        assert!(engine.begin_reconversion("🫠").is_empty());
        convert_and_commit(&mut engine, "kanji", "漢字");

        assert!(!directory.join("context_history.tsv").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reloading_user_data_breaks_the_in_memory_left_context() {
        let directory = test_directory("reload-context-boundary");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        convert_and_commit(&mut engine, "bunshou", "文章");
        fs::remove_file(directory.join("history.tsv")).unwrap();
        engine.reload_user_data();
        convert_and_commit(&mut engine, "kanji", "漢字");

        assert!(!directory.join("context_history.tsv").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_context_reset_prevents_learning_across_an_external_caret_move() {
        let directory = test_directory("explicit-context-boundary");
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        });

        convert_and_commit(&mut engine, "bunshou", "文章");
        engine.reset_context();
        convert_and_commit(&mut engine, "kanji", "漢字");

        assert!(!directory.join("context_history.tsv").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn date_and_time_are_additional_candidates_not_new_first_choices() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "kyou");
        engine.handle(InputEvent::Space);
        let candidates = engine.snapshot().candidates;
        assert_eq!(candidates[0], "今日");
        let expected = date_time_candidates::candidates("きょう", ALL_DATE_FORMATS);
        assert_eq!(&candidates[1..=expected.len()], expected, "{candidates:?}");

        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "ima");
        engine.handle(InputEvent::Space);
        assert!(engine.snapshot().candidates.iter().any(|candidate| {
            candidate.len() == 5 && candidate.as_bytes().get(2) == Some(&b':')
        }));
    }

    #[test]
    fn date_candidate_formats_follow_the_enabled_mask() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            date_format_mask: date_time_candidates::SHORT_REIWA | date_time_candidates::WEEKDAY,
            ..EnginePreferences::default()
        });
        type_text(&mut engine, "kyou");
        engine.handle(InputEvent::Space);
        let candidates = engine.snapshot().candidates;

        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.starts_with('R') && candidate.contains('/')),
            "{candidates:?}"
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.ends_with("曜日")),
            "{candidates:?}"
        );
        assert!(
            !candidates.iter().any(|candidate| {
                candidate.len() == 10
                    && candidate.as_bytes().get(4) == Some(&b'/')
                    && candidate.as_bytes().get(7) == Some(&b'/')
            }),
            "{candidates:?}"
        );
    }

    #[test]
    fn repeated_context_history_guides_live_and_protects_it_from_model_replacement() {
        let directory = test_directory("live-short-context");
        let mut data = UserData::load(&directory);
        for _ in 0..2 {
            data.record_context("しょくじ", "食事", "はし", "箸");
        }
        let mut engine = SlimeEngine::bundled_with_user_data(data);
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: true,
            history_learning: false,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("食事");
        assert_eq!(
            engine.contextual_history_surfaces_for_reading("はし", None),
            ["箸"]
        );
        type_text(&mut engine, "hashi");
        assert_eq!(
            engine.contextual_history_surfaces_for_reading("はし", None),
            ["箸"]
        );
        assert_eq!(engine.snapshot().preedit, "箸");
        assert!(engine.live_candidate_ranking_snapshot().is_none());
        type_text(&mut engine, "go");
        assert_eq!(engine.snapshot().preedit, "はしご");
        engine.handle(InputEvent::Escape);
        engine.reset_context();
        type_text(&mut engine, "hashi");
        assert_ne!(engine.snapshot().preedit, "箸");
        engine.handle(InputEvent::Escape);
        for _ in 0..2 {
            engine
                .user_data
                .record_context("どうろ", "道路", "はし", "架空の候補");
        }
        engine.set_external_left_context("道路");
        type_text(&mut engine, "hashi");
        assert_ne!(engine.snapshot().preedit, "架空の候補");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_particle_extension_uses_only_existing_contextual_candidates() {
        let directory = test_directory("context-particle-extension");
        let mut data = UserData::load(&directory);
        for _ in 0..2 {
            data.record_context("しょくじ", "食事", "はし", "箸");
        }
        let mut engine = SlimeEngine::bundled_with_user_data(data);
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            ..EnginePreferences::default()
        });
        engine.set_external_left_context("食事");
        assert!(
            engine
                .contextual_particle_history_surfaces("はしを", None, &["橋を".to_owned()])
                .is_empty()
        );
        type_text(&mut engine, "hashiwo");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "箸を");
        assert_eq!(
            engine.candidate_details()[0].annotation,
            super::CandidateAnnotation::History
        );
        if let Some(request) = engine.candidate_ranking_request() {
            assert!(
                request
                    .candidates
                    .iter()
                    .all(|candidate| candidate.surface != "箸を")
            );
        }
        assert!(
            engine
                .contextual_particle_history_surfaces("はしご", None, &["箸ご".to_owned()])
                .is_empty()
        );
        engine.handle(InputEvent::Escape);
        engine.handle(InputEvent::Escape);
        engine.reset_context();
        assert!(
            engine
                .contextual_particle_history_surfaces("はしを", None, &["箸を".to_owned()])
                .is_empty()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn learned_live_particle_extensions_remain_reopenable() {
        let directory = test_directory("live-particle-extension");
        let mut data = UserData::load(&directory);
        for _ in 0..2 {
            data.record_context("しょくじ", "食事", "はし", "箸");
        }
        for (suffix, expected) in [
            ("ha", "箸は"),
            ("ga", "箸が"),
            ("wo", "箸を"),
            ("ni", "箸に"),
            ("de", "箸で"),
            ("to", "箸と"),
            ("mo", "箸も"),
            ("he", "箸へ"),
            ("no", "箸の"),
            ("moto", "橋本"),
        ] {
            let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
            engine.set_preferences(EnginePreferences {
                live_conversion: true,
                history_completion: true,
                ..EnginePreferences::default()
            });
            engine.set_external_left_context("食事");
            type_text(&mut engine, "hashi");
            type_text(&mut engine, suffix);
            assert_eq!(engine.snapshot().preedit, expected, "suffix: {suffix}");
            if suffix != "moto" {
                assert!(engine.live_candidate_ranking_snapshot().is_none());
                assert!(!engine.live_preview.as_ref().unwrap().sealable_bunsetsu);
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn private_mode_neither_reads_nor_writes_history() {
        let directory = test_directory("private-mode");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nにほんご	日本語履歴	10	10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
            private_mode: true,
            date_format_mask: ALL_DATE_FORMATS,
        });

        type_text(&mut engine, "nih");
        assert!(
            !engine
                .snapshot()
                .candidates
                .contains(&"日本語履歴".to_owned())
        );
        type_text(&mut engine, "on");
        engine.handle(InputEvent::Space);
        engine.handle(InputEvent::Enter);
        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert_eq!(history.lines().count(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn empty_control_keys_are_forwarded() {
        let mut engine = SlimeEngine::bundled();

        assert_eq!(
            engine.handle(InputEvent::Enter),
            vec![SlimeAction::ForwardKey]
        );
        assert_eq!(
            engine.handle(InputEvent::Space),
            vec![SlimeAction::ForwardKey]
        );
    }
}
