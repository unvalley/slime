//! Platform-independent IME state machine.

use slime_converter::{Candidate, Dictionary, Segment};
use slime_romaji::RomajiComposer;

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
use english_reverse::ReverseMatch;
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

const SHORT_EXPANDED_N_BEST: usize = 32;
const LONG_EXPANDED_N_BEST: usize = 16;
const MAX_EXPANDED_READING_CHARACTERS: usize = 8;
const MAX_COMPOUND_READING_CHARACTERS: usize = 16;
const COMPOUND_ENTRIES_PER_SEGMENT: usize = 8;
const COMPOUND_CANDIDATE_LIMIT: usize = 32;
const PERSONAL_NAME_ENTRIES_PER_PART: usize = 64;
const PERSONAL_NAME_CANDIDATE_LIMIT: usize = 64;
const FIXED_SEGMENT_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_CANDIDATE_LIMIT: usize = 22;
const CONTEXT_RULE_PROMOTION_LIMIT: usize = 8;
const SHORT_RESCORE_CANDIDATE_LIMIT: usize = 5;
const LONG_RESCORE_CANDIDATE_LIMIT: usize = 8;
const LONG_RESCORE_READING_CHARACTERS: usize = MAX_EXPANDED_READING_CHARACTERS + 1;
const EXTENDED_LONG_RESCORE_N_BEST: usize = 16;
const EXTENDED_LONG_RESCORE_CANDIDATE_LIMIT: usize = 16;
const RESCORE_MAX_BASE_COST_GAP: i32 = 1_000;
const RESCORE_MAX_CANDIDATE_COST_GAP: i32 = 1_500;
const RESCORE_COST_LOG_SCALE: f64 = 500.0;

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

/// Immutable input for an optional external candidate scorer.
///
/// Only ordinary dictionary candidates are exposed here. Candidates promoted
/// by the user dictionary, history, an installed context rule, or typo
/// correction remain outside this request and cannot be displaced by an
/// external model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRescoreRequest {
    pub context: String,
    pub right_context: String,
    pub reading: String,
    pub candidates: Vec<String>,
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

#[derive(Clone, Debug)]
struct LivePreview {
    /// Complete kana reading covered by `surface`.
    reading: String,
    surface: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CandidateCorrection {
    surface: String,
    reading: String,
}

#[derive(Clone, Debug)]
struct CandidateRescoreState {
    request: CandidateRescoreRequest,
    candidates: Vec<Candidate>,
}

#[derive(Debug)]
struct ConversionCandidateSet {
    surfaces: Vec<String>,
    rescore: Option<CandidateRescoreState>,
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
    dictionary: Dictionary,
    romaji: RomajiComposer,
    reading: String,
    raw_input: String,
    candidates: Vec<String>,
    candidate_corrections: Vec<CandidateCorrection>,
    candidate_rescore: Option<CandidateRescoreState>,
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
    user_data: UserData,
    installed_packs: DictionaryPackStore,
    dictionary_pack_trust: DictionaryPackTrust,
    session_history: SessionHistory,
    uses_bundled_dictionary: bool,
    /// `(lowercased key, surface)` pairs of ASCII words from the enabled
    /// domain dictionaries and the user dictionary, for reverse matching
    /// English words typed in kana mode.
    ascii_surfaces: Vec<(String, String)>,
}

impl SlimeEngine {
    #[must_use]
    pub fn new(dictionary: Dictionary) -> Self {
        Self {
            dictionary,
            romaji: RomajiComposer::new(),
            reading: String::new(),
            raw_input: String::new(),
            candidates: Vec::new(),
            candidate_corrections: Vec::new(),
            candidate_rescore: None,
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
            user_data: UserData::default(),
            installed_packs: DictionaryPackStore::default(),
            dictionary_pack_trust: DictionaryPackTrust::default(),
            session_history: SessionHistory::default(),
            uses_bundled_dictionary: false,
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
        if (!self.preferences.private_mode && preferences.private_mode)
            || (self.preferences.history_learning && !preferences.history_learning)
        {
            self.session_history.reset_context();
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
        self.rebuild_ascii_surfaces();
        self.live_preview_suppressed = false;
        self.refresh_live_preview();
        self.refresh_completion_actions(true)
    }

    pub fn reload_user_data(&mut self) -> Vec<SlimeAction> {
        // A reload can remove the commit retained by the current session.
        // Keeping it would recreate a deleted context edge on the next commit.
        self.session_history.reset_context();
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
        self.rebuild_ascii_surfaces();
        self.refresh_live_preview();
        self.refresh_completion_actions(true)
    }

    /// Breaks the transient left-context chain after an external caret,
    /// document, or input-client boundary without deleting persisted history.
    pub fn reset_context(&mut self) {
        self.session_history.reset_context();
    }

    /// Replaces the transient context with committed text owned by the input
    /// client. This surface is bounded in memory, is never persisted, and is
    /// not used to learn a contextual history edge because its reading is
    /// unknown.
    pub fn set_external_left_context(&mut self, surface: &str) {
        self.set_external_context(surface, "");
    }

    /// Replaces transient document context on both sides of the platform
    /// caret. Only the left side participates in lattice ranking; an optional
    /// external model may use both sides. Neither side is persisted.
    pub fn set_external_context(&mut self, left_surface: &str, right_surface: &str) {
        if self.preferences.private_mode {
            self.session_history.reset_context();
        } else {
            self.session_history
                .set_external_contexts(left_surface, right_surface);
        }
    }

    /// Returns conversion candidates without changing the active composition.
    /// Platform search integrations use this path so querying alternatives
    /// cannot move the user's selection or commit text as a side effect.
    #[must_use]
    pub fn conversion_candidates(&self, reading: &str) -> Vec<String> {
        self.conversion_candidates_for_reading(reading)
    }

    /// Returns the current explicit-conversion candidates eligible for an
    /// optional external scorer. The request is absent when a personalized or
    /// rule-based candidate is already promoted, the base winner is decisive,
    /// or the active candidates came from completion, reconversion, segmented
    /// conversion, or typo correction.
    #[must_use]
    pub fn candidate_rescore_request(&self) -> Option<CandidateRescoreRequest> {
        self.candidate_rescore
            .as_ref()
            .filter(|_| {
                self.candidate_kind == Some(CandidateKind::Conversion) && self.selected == 0
            })
            .map(|state| state.request.clone())
    }

    /// Widens the pending external-scoring pool for a long explicit reading.
    ///
    /// Interactive adapters call this only after their local scorer is ready.
    /// The visible ten-candidate result remains untouched unless scoring
    /// succeeds. A missing or not-yet-ready optional model cannot add latency,
    /// and a scoring failure cannot partially publish the deeper result.
    pub fn prepare_extended_candidate_rescore(&mut self) {
        if self.candidate_kind != Some(CandidateKind::Conversion)
            || self.selected != 0
            || self.reading.chars().count() < LONG_RESCORE_READING_CHARACTERS
        {
            return;
        }
        let Some(current) = self.candidate_rescore.as_ref() else {
            return;
        };
        let reading = current.request.reading.clone();
        let context = current.request.context.clone();
        let right_context = current.request.right_context.clone();
        let dictionary_candidates = if context.is_empty() {
            self.dictionary
                .candidates_with_limit(&reading, EXTENDED_LONG_RESCORE_N_BEST)
        } else {
            self.dictionary.candidates_with_context_limit(
                &reading,
                &context,
                EXTENDED_LONG_RESCORE_N_BEST,
            )
        };
        self.candidate_rescore = candidate_rescore_state_with_limit(
            &reading,
            &context,
            &right_context,
            &[],
            &dictionary_candidates,
            EXTENDED_LONG_RESCORE_CANDIDATE_LIMIT,
        );
    }

    /// Applies model log-likelihoods to the pending dictionary-only request.
    ///
    /// The pending request is consumed even when validation fails, so stale or
    /// malformed model output can never be applied to a later composition.
    /// A successful result contains replacement candidate/preedit actions;
    /// callers should publish these instead of the actions emitted before
    /// scoring.
    pub fn apply_candidate_rescore(
        &mut self,
        log_likelihoods: &[f64],
        lambda: f64,
        minimum_margin: f64,
    ) -> Option<Vec<SlimeAction>> {
        let state = self.candidate_rescore.take()?;
        if self.candidate_kind != Some(CandidateKind::Conversion)
            || self.selected != 0
            || state.candidates.len() != log_likelihoods.len()
            || !(0.0..=1.0).contains(&lambda)
            || !lambda.is_finite()
            || minimum_margin < 0.0
            || !minimum_margin.is_finite()
            || log_likelihoods.iter().any(|score| !score.is_finite())
        {
            return None;
        }

        let mut pending_candidates = self.candidates.clone();
        let existing_positions: Vec<_> = state
            .candidates
            .iter()
            .map(|candidate| {
                pending_candidates
                    .iter()
                    .position(|surface| surface == &candidate.surface)
            })
            .collect();
        let mut seen_positions = Vec::with_capacity(state.candidates.len());
        for position in existing_positions.iter().flatten().copied() {
            if seen_positions.contains(&position) {
                return None;
            }
            seen_positions.push(position);
        }
        let mut insertion_position = existing_positions
            .iter()
            .flatten()
            .copied()
            .max()?
            .saturating_add(1);
        let mut positions = Vec::with_capacity(state.candidates.len());
        for (candidate, existing_position) in state.candidates.iter().zip(existing_positions) {
            if let Some(position) = existing_position {
                positions.push(position);
            } else {
                pending_candidates.insert(insertion_position, candidate.surface.clone());
                positions.push(insertion_position);
                insertion_position += 1;
            }
        }

        let mut order: Vec<usize> = (0..state.candidates.len()).collect();
        let combined: Vec<f64> = state
            .candidates
            .iter()
            .zip(log_likelihoods)
            .map(|(candidate, log_likelihood)| {
                (1.0 - lambda) * (-f64::from(candidate.cost) / RESCORE_COST_LOG_SCALE)
                    + lambda * log_likelihood
            })
            .collect();
        order.sort_by(|&left, &right| combined[right].total_cmp(&combined[left]));
        if order
            .first()
            .is_some_and(|&top| top != 0 && combined[top] - combined[0] < minimum_margin)
        {
            return Some(self.candidate_actions());
        }
        for (position, candidate_index) in positions.into_iter().zip(order) {
            pending_candidates[position].clone_from(&state.candidates[candidate_index].surface);
        }
        self.candidates = pending_candidates;
        Some(self.candidate_actions())
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
        if !self
            .conversion_candidates_for_reading(reading)
            .iter()
            .any(|candidate| candidate == surface)
        {
            return false;
        }
        self.record_history(reading, surface);
        true
    }

    /// Starts explicit reconversion for a selected committed surface. An
    /// empty action list means the surface has no safe dictionary reading.
    pub fn begin_reconversion(&mut self, surface: &str) -> Vec<SlimeAction> {
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
    fn english_reverse_surfaces(&self, target: &str) -> Vec<String> {
        if target.is_empty() || self.ascii_surfaces.is_empty() {
            return Vec::new();
        }
        let mut exact = Vec::new();
        let mut prefix = Vec::new();
        for (key, surface) in &self.ascii_surfaces {
            match english_reverse::reverse_match(target, key) {
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

    pub fn handle(&mut self, event: InputEvent) -> Vec<SlimeAction> {
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
            let committed = self.committed_surface();
            let reading = if self.phase() == Phase::Converting {
                self.selected_learning_reading().to_owned()
            } else {
                self.reading.clone()
            };
            self.record_conversion_history(&reading, &committed);
            actions.push(SlimeAction::Commit(committed));
            self.clear_composition();
            actions.push(SlimeAction::HideCandidates);
        } else if had_completions {
            self.clear_candidates();
        }

        self.raw_input.push(character);

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
            self.candidate_rescore = None;
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

        let (candidates, corrections, rescore) =
            self.conversion_candidates_with_corrections(&self.reading, &self.raw_input);
        self.candidates = candidates;
        self.candidate_corrections = corrections;
        self.candidate_rescore = rescore;
        self.selected = 0;
        self.candidate_kind = Some(CandidateKind::Conversion);
        self.completion_selected = false;
        actions.extend(self.candidate_actions());
        actions
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
        self.conversion_candidate_set_for_reading_with_limit_and_context(
            reading,
            dictionary_limit,
            explicit_previous_surface,
        )
        .surfaces
    }

    fn conversion_candidate_set_for_reading_with_limit_and_context(
        &self,
        reading: &str,
        dictionary_limit: Option<usize>,
        explicit_previous_surface: Option<&str>,
    ) -> ConversionCandidateSet {
        let mut candidates = Vec::new();
        let previous_surface = if self.preferences.private_mode {
            None
        } else {
            explicit_previous_surface.or_else(|| self.session_history.previous_surface())
        };
        let right_context = if self.preferences.private_mode || explicit_previous_surface.is_some()
        {
            ""
        } else {
            self.session_history.right_surface().unwrap_or_default()
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
        for (key, surface) in &self.ascii_surfaces {
            if english_reverse::reverse_match(reading, key) == Some(ReverseMatch::Exact) {
                push_unique(&mut candidates, surface.clone());
            }
        }
        // The literal hiragana reading stays selectable; hiding it made
        // single-kana words like み unreachable through the candidate window.
        let dictionary_candidates = if previous_surface.is_some() || !right_context.is_empty() {
            match dictionary_limit {
                Some(limit) => self.dictionary.candidates_with_surrounding_context_limit(
                    reading,
                    previous_surface.unwrap_or_default(),
                    right_context,
                    limit,
                ),
                None => self.dictionary.candidates_with_surrounding_context(
                    reading,
                    previous_surface.unwrap_or_default(),
                    right_context,
                ),
            }
        } else {
            match dictionary_limit {
                Some(limit) => self.dictionary.candidates_with_limit(reading, limit),
                None => self.dictionary.candidates(reading),
            }
        };
        let dictionary_surfaces: Vec<_> = dictionary_candidates
            .iter()
            .map(|candidate| candidate.surface.as_str())
            .collect();
        if let Some(previous_surface) = previous_surface {
            let mut promoted = 0;
            self.installed_packs
                .visit_contextual_surfaces(previous_surface, reading, |surface| {
                    if dictionary_surfaces.contains(&surface)
                        && !candidates.iter().any(|candidate| candidate == surface)
                    {
                        candidates.push(surface.to_owned());
                        promoted += 1;
                    }
                    promoted < CONTEXT_RULE_PROMOTION_LIMIT
                });
        }
        let rescore = candidate_rescore_state(
            reading,
            previous_surface.unwrap_or_default(),
            right_context,
            &candidates,
            &dictionary_candidates,
        );
        for candidate in dictionary_candidates {
            push_unique(&mut candidates, candidate.surface);
        }
        insert_visible_katakana_candidate(&mut candidates, reading);
        insert_unique_candidates_after_first(
            &mut candidates,
            date_time_candidates::candidates(reading, self.preferences.date_format_mask),
        );
        ConversionCandidateSet {
            surfaces: candidates,
            rescore,
        }
    }

    fn conversion_candidates_with_corrections(
        &self,
        reading: &str,
        raw_input: &str,
    ) -> (
        Vec<String>,
        Vec<CandidateCorrection>,
        Option<CandidateRescoreState>,
    ) {
        let ordinary =
            self.conversion_candidate_set_for_reading_with_limit_and_context(reading, None, None);
        if self.dictionary.has_exact_reading(reading)
            || self
                .user_data
                .exact_dictionary_surfaces(reading)
                .next()
                .is_some()
            || (self.history_is_available()
                && !self.user_data.exact_history_surfaces(reading).is_empty())
            || self.ascii_surfaces.iter().any(|(key, _)| {
                english_reverse::reverse_match(reading, key) == Some(ReverseMatch::Exact)
            })
        {
            return (ordinary.surfaces, Vec::new(), ordinary.rescore);
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
            return (ordinary.surfaces, corrections, ordinary.rescore);
        }

        let mut candidates = Vec::with_capacity(ordinary.surfaces.len() + corrections.len() + 1);
        push_unique(&mut candidates, reading.to_owned());
        for correction in &corrections {
            push_unique(&mut candidates, correction.surface.clone());
        }
        for candidate in ordinary.surfaces {
            push_unique(&mut candidates, candidate);
        }
        (candidates, corrections, None)
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

    fn next_candidate(&mut self) -> Vec<SlimeAction> {
        if self.candidates.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        self.candidate_rescore = None;
        if self.selected + 1 == self.candidates.len() {
            self.expand_conversion_candidates_if_needed();
        }
        self.selected = (self.selected + 1) % self.candidates.len();
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

        self.candidate_rescore = None;
        if self.selected == 0 {
            self.expand_conversion_candidates_if_needed();
        }
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.candidates.len() - 1);
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
        // Keep long-input expansion bounded: this runs only after the user
        // reaches the end of the initial candidate list, never on first show.
        let expanded_n_best = if reading_length <= MAX_EXPANDED_READING_CHARACTERS {
            SHORT_EXPANDED_N_BEST
        } else {
            LONG_EXPANDED_N_BEST
        };
        for candidate in
            self.conversion_candidates_for_reading_with_limit(&self.reading, Some(expanded_n_best))
        {
            push_unique(&mut merged, candidate);
        }
        if reading_length <= MAX_COMPOUND_READING_CHARACTERS {
            for candidate in self.dictionary.compound_candidates(
                &self.reading,
                COMPOUND_ENTRIES_PER_SEGMENT,
                COMPOUND_CANDIDATE_LIMIT,
            ) {
                push_unique(&mut merged, candidate.surface);
            }
            for candidate in self.dictionary.personal_name_candidates(
                &self.reading,
                PERSONAL_NAME_ENTRIES_PER_PART,
                PERSONAL_NAME_CANDIDATE_LIMIT,
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
        }
        self.candidates = merged;
    }

    fn select_candidate(&mut self, index: u32) -> Vec<SlimeAction> {
        let index = index as usize;
        if index >= self.candidates.len() {
            return Vec::new();
        }

        self.selected = index;
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
        let raw = (!self.raw_input.is_empty()).then_some(self.raw_input.as_str());
        self.transformed_surface = Some(transform_text(style, &self.reading, raw));
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
        self.enter_segment_mode();
        if self.segments.is_empty() {
            return vec![SlimeAction::ForwardKey];
        }

        if expand {
            let next_index = self.active_segment + 1;
            if next_index >= self.segments.len() {
                return self.candidate_actions();
            }
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
            let Some(character) = self.segments[self.active_segment].reading.pop() else {
                return self.candidate_actions();
            };
            if self.segments[self.active_segment].reading.is_empty() {
                self.segments[self.active_segment].reading.push(character);
                return self.candidate_actions();
            }
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
        self.reset_segment_surface(self.active_segment);
        self.activate_segment_candidates();
        self.candidate_actions()
    }

    /// Returns true when this call changed whole-phrase conversion into
    /// segmented conversion.
    fn enter_segment_mode(&mut self) -> bool {
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            return false;
        }
        let selected_surface = self.selected_candidate().to_owned();
        let conversion = self
            .dictionary
            .convert_n_best(&self.reading, 32)
            .into_iter()
            .find(|conversion| conversion.surface == selected_surface)
            .or_else(|| self.dictionary.convert_best(&self.reading));
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
        self.activate_segment_candidates();
        true
    }

    fn activate_segment_candidates(&mut self) {
        let segment = &self.segments[self.active_segment];
        let reading = segment.reading.clone();
        let surface = segment.surface.clone();
        self.candidate_corrections.clear();
        self.candidates = self.conversion_candidates_for_reading(&reading);
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

    fn reset_segment_surface(&mut self, index: usize) {
        let reading = self.segments[index].reading.clone();
        self.segments[index].surface = self
            .conversion_candidates_for_reading(&reading)
            .into_iter()
            .next()
            .unwrap_or(reading);
        self.segments[index].explicitly_selected = false;
    }

    fn candidate_actions(&self) -> Vec<SlimeAction> {
        let mut actions = Vec::with_capacity(2);
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            actions.push(self.segmented_preedit_action());
        } else if self.candidate_kind == Some(CandidateKind::Conversion) || self.completion_selected
        {
            actions.push(SlimeAction::UpdatePreedit(
                self.selected_candidate().to_owned(),
            ));
        }
        actions.push(SlimeAction::ShowCandidates {
            candidates: self.displayed_candidates(),
            details: self.candidate_details(),
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
        let mut context = Vec::new();
        if !self.preferences.private_mode
            && let Some(previous_surface) = self.session_history.previous_surface()
        {
            self.installed_packs
                .visit_contextual_surfaces(previous_surface, reading, |surface| {
                    if self.candidates.iter().any(|candidate| candidate == surface)
                        && !context.iter().any(|candidate| candidate == surface)
                    {
                        context.push(surface.to_owned());
                    }
                    context.len() < CONTEXT_RULE_PROMOTION_LIMIT
                });
        }
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
                } else if history.contains(&candidate.as_str()) {
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
            self.raw_input.pop();
        } else {
            self.reading.pop();
            self.raw_input.clear();
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
        self.raw_input.clear();
        self.live_preview = None;
        self.live_preview_suppressed = false;
        self.segments.clear();
        self.active_segment = 0;
        self.transformed_surface = None;
        self.clear_candidates();
    }

    fn clear_candidates(&mut self) {
        self.candidates.clear();
        self.candidate_corrections.clear();
        self.candidate_rescore = None;
        self.selected = 0;
        self.candidate_kind = None;
        self.completion_selected = false;
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
        let can_evaluate = !resolved
            .chars()
            .any(|character| character.is_ascii_alphabetic())
            && resolved.chars().count() >= live_conversion::MINIMUM_READING_CHARACTERS;

        if can_evaluate {
            match self.live_conversion_decision(&resolved) {
                LiveConversionDecision::Confident(surface) => {
                    self.live_preview = Some(LivePreview {
                        reading: resolved,
                        surface,
                    });
                    return;
                }
                LiveConversionDecision::Ambiguous(surface)
                    if self.live_preview.as_ref().is_some_and(|preview| {
                        confirmed_literal_extension(preview, &resolved, &surface)
                    }) =>
                {
                    // The full lattice independently confirms exactly the
                    // surface already shown plus the newly resolved suffix.
                    // Keep that stable extension even when an alternative
                    // spelling is inside the global confidence margin.
                    self.live_preview = Some(LivePreview {
                        reading: resolved,
                        surface,
                    });
                    return;
                }
                LiveConversionDecision::Ambiguous(_) | LiveConversionDecision::Literal => {}
            }
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

        self.live_preview = None;
    }

    fn live_conversion_decision(&self, reading: &str) -> LiveConversionDecision {
        live_conversion::decide(&self.dictionary, &self.user_data, reading)
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

fn candidate_rescore_state(
    reading: &str,
    context: &str,
    right_context: &str,
    protected_candidates: &[String],
    dictionary_candidates: &[Candidate],
) -> Option<CandidateRescoreState> {
    let candidate_limit = if reading.chars().count() >= LONG_RESCORE_READING_CHARACTERS {
        LONG_RESCORE_CANDIDATE_LIMIT
    } else {
        SHORT_RESCORE_CANDIDATE_LIMIT
    };
    candidate_rescore_state_with_limit(
        reading,
        context,
        right_context,
        protected_candidates,
        dictionary_candidates,
        candidate_limit,
    )
}

fn candidate_rescore_state_with_limit(
    reading: &str,
    context: &str,
    right_context: &str,
    protected_candidates: &[String],
    dictionary_candidates: &[Candidate],
    candidate_limit: usize,
) -> Option<CandidateRescoreState> {
    if !protected_candidates.is_empty() {
        return None;
    }
    let base_cost = dictionary_candidates.first()?.cost;
    let candidates: Vec<_> = dictionary_candidates
        .iter()
        .take(candidate_limit)
        .take_while(|candidate| {
            candidate.cost.saturating_sub(base_cost).max(0) <= RESCORE_MAX_CANDIDATE_COST_GAP
        })
        .cloned()
        .collect();
    let first = candidates.first()?.cost;
    let alternative = candidates
        .iter()
        .skip(1)
        .map(|candidate| candidate.cost)
        .min()?;
    if alternative.saturating_sub(first).max(0) > RESCORE_MAX_BASE_COST_GAP {
        return None;
    }
    Some(CandidateRescoreState {
        request: CandidateRescoreRequest {
            context: context.to_owned(),
            right_context: right_context.to_owned(),
            reading: reading.to_owned(),
            candidates: candidates
                .iter()
                .map(|candidate| candidate.surface.clone())
                .collect(),
        },
        candidates,
    })
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

fn confirmed_literal_extension(preview: &LivePreview, reading: &str, surface: &str) -> bool {
    let Some(suffix) = reading.strip_prefix(&preview.reading) else {
        return false;
    };
    !suffix.is_empty() && surface.strip_prefix(&preview.surface) == Some(suffix)
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
        CandidateKind, CandidateRescoreRequest, CandidateRescoreState, ConversionSearch,
        DictionaryPackTrust, DictionaryPackVerificationKey, DictionaryPackVersionFloor,
        DictionaryPackWord, EnginePreferences, InputEvent, LiveConversionDecision,
        MAX_EXPANDED_READING_CHARACTERS, Phase, SlimeAction, SlimeEngine, TECHNOLOGY_DICTIONARY,
        UserData, bundled_dictionary, date_time_candidates, katakana_candidate,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use slime_converter::{Candidate, Dictionary, DictionaryEntry};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn type_text(engine: &mut SlimeEngine, input: &str) {
        for character in input.chars() {
            engine.handle(InputEvent::Character(character));
        }
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
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let payload = "てすとようご\t試験用語\n\
ながいぶんしょう\tこれは長い文章\n\
# context-rules\n\
文章\tかんじ\t漢字\t0\n\
文章\tかんじ\t架空候補\t1\n";
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

        assert_eq!(
            engine.live_conversion_decision("らいぶへんかん"),
            LiveConversionDecision::Confident("ライブ変換".to_owned())
        );
        assert_eq!(
            engine.live_conversion_decision("らいぶへんかんで"),
            LiveConversionDecision::Ambiguous("ライブ変換で".to_owned())
        );
        assert_eq!(
            engine.live_conversion_decision("らいぶへんかんの"),
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

        contextual.handle(InputEvent::Escape);
        contextual.reset_context();
        type_text(&mut contextual, "kanji");
        contextual.handle(InputEvent::Space);
        assert_ne!(contextual.snapshot().preedit, "漢字");

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
    fn external_right_context_ranks_an_inflected_word_before_a_polite_auxiliary() {
        let mut engine = SlimeEngine::bundled();

        engine.set_external_context("うまいコーヒーが", "ました。");
        type_text(&mut engine, "nome");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "飲め");
    }

    #[test]
    fn external_right_context_ranks_a_continuative_verb_before_desiderative_auxiliary() {
        let mut engine = SlimeEngine::bundled();

        engine.set_external_context("丁寧に案内してもらい、", "たい物が買えました。");
        type_text(&mut engine, "kai");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "買い");
    }

    #[test]
    fn external_right_context_ranks_a_unique_form_before_following_grammar() {
        let mut engine = SlimeEngine::bundled();

        engine.set_external_context("有名な先生方が講師として", "られています。");
        type_text(&mut engine, "ko");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "来");
    }

    #[test]
    fn external_right_context_ranks_a_dictionary_compound_prefix() {
        let mut engine = SlimeEngine::bundled();

        engine.set_external_context("患者と患者の", "時間は少ない");
        type_text(&mut engine, "machi");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "待ち");
    }

    #[test]
    fn external_right_context_ranks_an_inflectional_phrase_prefix() {
        let mut engine = SlimeEngine::bundled();

        engine.set_external_context("カラフルで色合いがいいデザインがあったので", "に入りました");
        type_text(&mut engine, "ki");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "気");
    }

    #[test]
    fn private_mode_ignores_external_right_context() {
        let mut engine = SlimeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            private_mode: true,
            ..EnginePreferences::default()
        });

        engine.set_external_context("うまいコーヒーが", "ました。");
        type_text(&mut engine, "nome");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "の目");
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
    fn external_scores_reorder_only_pending_dictionary_candidates() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
            DictionaryEntry::new("にほん", "仁本", 1_200),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.set_external_context("直前", "直後");
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("ambiguous dictionary candidates should be scoreable");
        assert_eq!(request.reading, "にほん");
        assert_eq!(request.context, "直前");
        assert_eq!(request.right_context, "直後");
        assert_eq!(request.candidates.len(), 3);
        let promoted = request.candidates[1].clone();
        let scores: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| if candidate == &promoted { 0.0 } else { -10.0 })
            .collect();
        let actions = engine
            .apply_candidate_rescore(&scores, 0.7, 0.1)
            .expect("aligned scores should apply");

        assert_eq!(engine.snapshot().candidates.first(), Some(&promoted));
        assert!(actions.contains(&SlimeAction::UpdatePreedit(promoted)));
        assert!(engine.candidate_rescore_request().is_none());
    }

    #[test]
    fn weak_external_score_change_keeps_the_base_winner() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let actions = engine
            .apply_candidate_rescore(&[-1.0, -0.9], 0.7, 0.1)
            .expect("aligned finite scores should be consumed");

        assert_eq!(
            engine.snapshot().candidates.first(),
            Some(&"日本".to_owned())
        );
        assert!(actions.contains(&SlimeAction::UpdatePreedit("日本".to_owned())));
    }

    #[test]
    fn external_scoring_exposes_the_full_short_candidate_pool() {
        let entries = (0_i32..12)
            .map(|index| {
                DictionaryEntry::new(
                    "こうほ",
                    format!("候補{index}"),
                    1_000 + index.saturating_mul(10),
                )
            })
            .collect();
        let mut engine = SlimeEngine::new(Dictionary::new(entries));
        type_text(&mut engine, "kouho");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("ambiguous dictionary candidates should be scoreable");
        assert_eq!(
            request.candidates.len(),
            super::SHORT_RESCORE_CANDIDATE_LIMIT
        );
        assert_eq!(
            request.candidates,
            (0..5)
                .map(|index| format!("候補{index}"))
                .collect::<Vec<_>>()
        );
        assert!(!request.candidates.contains(&"コウホ".to_owned()));
    }

    #[test]
    fn external_scoring_expands_the_candidate_pool_for_long_readings() {
        let reading = "ちょうぶんしょうに";
        assert_eq!(
            reading.chars().count(),
            super::LONG_RESCORE_READING_CHARACTERS
        );
        let entries = (0_i32..12)
            .map(|index| {
                DictionaryEntry::new(
                    reading,
                    format!("長文候補{index}"),
                    1_000 + index.saturating_mul(10),
                )
            })
            .collect();
        let mut engine = SlimeEngine::new(Dictionary::new(entries));
        type_text(&mut engine, "choubunshouni");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("long ambiguous readings should expose the expanded pool");
        assert_eq!(
            request.candidates,
            (0..super::LONG_RESCORE_CANDIDATE_LIMIT)
                .map(|index| format!("長文候補{index}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn ready_external_scorer_can_prepare_a_deeper_long_reading_pool() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "sekairekishitaikeiigirisushi");
        engine.handle(InputEvent::Space);

        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("long reading should have a standard request")
                .candidates
                .len(),
            super::LONG_RESCORE_CANDIDATE_LIMIT
        );
        engine.prepare_extended_candidate_rescore();
        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("ready scorer should receive the deeper request")
                .candidates
                .len(),
            super::EXTENDED_LONG_RESCORE_CANDIDATE_LIMIT
        );
    }

    #[test]
    fn external_scores_insert_a_deep_candidate_only_after_success() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        assert!(!engine.candidates.contains(&"深層".to_owned()));

        let candidates = vec![
            Candidate {
                surface: "日本".to_owned(),
                cost: 1_000,
            },
            Candidate {
                surface: "二本".to_owned(),
                cost: 1_100,
            },
            Candidate {
                surface: "深層".to_owned(),
                cost: 1_200,
            },
        ];
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: "にほん".to_owned(),
                candidates: candidates
                    .iter()
                    .map(|candidate| candidate.surface.clone())
                    .collect(),
            },
            candidates,
        });
        engine
            .apply_candidate_rescore(&[-10.0, -10.0, 0.0], 0.7, 0.1)
            .expect("aligned deep scores should apply");

        assert_eq!(engine.candidates[0], "深層");
        assert_eq!(engine.candidates[1], "ニホン");
        assert!(engine.candidates.contains(&"日本".to_owned()));
        assert!(engine.candidates.contains(&"二本".to_owned()));
    }

    #[test]
    fn external_scoring_omits_a_remote_candidate_tail() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("こうほ", "第一", 1_000),
            DictionaryEntry::new("こうほ", "第二", 1_100),
            DictionaryEntry::new("こうほ", "第三", 2_501),
            DictionaryEntry::new("こうほ", "第四", 2_600),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, "kouho");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("the close top two candidates should remain scoreable");
        assert_eq!(request.candidates, ["第一", "第二"]);
    }

    #[test]
    fn malformed_scores_leave_the_base_order_and_are_not_reused() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);
        let base = engine.snapshot().candidates;

        assert!(engine.apply_candidate_rescore(&[], 0.7, 0.1).is_none());
        assert_eq!(engine.snapshot().candidates, base);
        assert!(engine.candidate_rescore_request().is_none());
    }

    #[test]
    fn history_and_typo_corrections_are_never_exposed_to_external_scoring() {
        let directory = test_directory("rescore-protected");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nにほん\t履歴日本\t5\t10\n",
        )
        .unwrap();
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
        ]);
        let mut history = SlimeEngine::with_user_data(dictionary, UserData::load(&directory));
        history.set_preferences(EnginePreferences {
            history_completion: true,
            history_learning: true,
            ..EnginePreferences::default()
        });
        type_text(&mut history, "nihon");
        history.handle(InputEvent::Space);
        assert_eq!(
            history.snapshot().candidates.first().map(String::as_str),
            Some("履歴日本")
        );
        assert!(history.candidate_rescore_request().is_none());

        let mut typo = SlimeEngine::bundled();
        type_text(&mut typo, "nihpn");
        typo.handle(InputEvent::Space);
        assert!(
            typo.snapshot()
                .candidates
                .iter()
                .any(|candidate| candidate == "日本")
        );
        assert!(typo.candidate_rescore_request().is_none());
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
    fn cycling_long_reading_adds_bounded_n_best_candidates() {
        let dictionary = Dictionary::bundled();
        let reading = "わたしはにほんじん";
        let mut engine = SlimeEngine::new(dictionary);
        type_text(&mut engine, "watashihanihonjin");
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let target = engine
            .conversion_candidates_for_reading_with_limit(
                reading,
                Some(super::LONG_EXPANDED_N_BEST),
            )
            .into_iter()
            .find(|surface| !initial.contains(surface))
            .expect("bounded N-best 16 should add a candidate beyond the initial search");
        let initial_count = initial.len();
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }

        assert!(engine.snapshot().candidates.contains(&target));
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
    fn explicit_expansion_reaches_deep_personal_name_spellings() {
        const GIVEN_NAME_POS_ID: u16 = 1922;
        const SURNAME_POS_ID: u16 = 1923;

        let mut entries = vec![DictionaryEntry::with_pos(
            "やまだ",
            "山田",
            SURNAME_POS_ID,
            SURNAME_POS_ID,
            100,
        )];
        entries.extend((0_i32..48).map(|index| {
            DictionaryEntry::with_pos(
                "ふかな",
                format!("候補{index:02}"),
                GIVEN_NAME_POS_ID,
                GIVEN_NAME_POS_ID,
                index,
            )
        }));
        entries.push(DictionaryEntry::with_pos(
            "ふかな",
            "深名",
            GIVEN_NAME_POS_ID,
            GIVEN_NAME_POS_ID,
            5_000,
        ));
        let mut engine = SlimeEngine::new(Dictionary::new(entries));
        type_text(&mut engine, "yamadahukana");
        engine.handle(InputEvent::Space);

        let initial = engine.snapshot().candidates;
        let initial_top = initial[0].clone();
        assert!(!initial.contains(&"山田深名".to_owned()));
        for _ in 0..initial.len() {
            engine.handle(InputEvent::NextCandidate);
        }

        let expanded = engine.snapshot().candidates;
        assert!(expanded.contains(&"山田深名".to_owned()));
        assert_eq!(expanded[0], initial_top);
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
