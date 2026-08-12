//! Platform-independent IME state machine.

use slime_converter::{Candidate, Conversion, Dictionary, Segment};
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
    DictionaryPackCandidateMode, DictionaryPackInfo, DictionaryPackLoadError, DictionaryPackTrust,
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
const LONG_DEEPENED_N_BEST: usize = 32;
const MAX_EXPANDED_READING_CHARACTERS: usize = 8;
const MAX_COMPOUND_READING_CHARACTERS: usize = 16;
const COMPOUND_ENTRIES_PER_SEGMENT: usize = 8;
const COMPOUND_CANDIDATE_LIMIT: usize = 32;
const PERSONAL_NAME_ENTRIES_PER_PART: usize = 64;
const PERSONAL_NAME_CANDIDATE_LIMIT: usize = 64;
const EXPLICIT_PACK_CANDIDATE_LIMIT: usize = 64;
const FIXED_SEGMENT_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_CANDIDATE_LIMIT: usize = 22;
const CONTEXT_RULE_PROMOTION_LIMIT: usize = 8;
const SHORT_RESCORE_CANDIDATE_LIMIT: usize = 5;
const LONG_RESCORE_CANDIDATE_LIMIT: usize = 8;
const LONG_RESCORE_READING_CHARACTERS: usize = MAX_EXPANDED_READING_CHARACTERS + 1;
const DEFAULT_EXTENDED_LONG_RESCORE_CANDIDATES: usize = 16;
const MAX_EXTENDED_LONG_RESCORE_CANDIDATES: usize = 32;
const RESCORE_MAX_BASE_COST_GAP: i32 = 1_000;
const RESCORE_MAX_CANDIDATE_COST_GAP: i32 = 1_500;
const SHORT_CONFIRMED_CONTEXT_RESCORE_MAX_READING_CHARACTERS: usize = 6;
const SHORT_CONFIRMED_CONTEXT_RESCORE_COST_GAP: i32 = 2_000;
const LONG_RESCORE_MAX_CANDIDATE_COST_GAP: i32 = 2_500;
const MODEL_KATAKANA_RECALL_ADDITIONAL_CANDIDATES: usize = 3;
const MODEL_KATAKANA_RECALL_MIN_RUN_CHARACTERS: usize = 5;
const SHORT_KATAKANA_RECALL_SEARCH_LIMIT: usize = 32;
const SHORT_KATAKANA_RECALL_MIN_BASE_COST: i32 = 20_000;
const RESCORE_COST_LOG_SCALE: f64 = 500.0;
const CONTEXT_ABLATED_EXACT_FRAGMENT_MIN_MODEL_MARGIN: f64 = 0.75;
const MODEL_SUPPLEMENTAL_ADDITIONAL_MARGIN: f64 = 1.5;
const PREFIX_CONSTRAINED_INITIAL_CANDIDATE_LIMIT: usize = 8;
const PREFIX_CONSTRAINED_MAX_CANDIDATE_LIMIT: usize = 32;
const PREFIX_CORRECTION_MAX_CHANGED_CHARACTERS: usize = 2;
const GENERATIVE_MIN_READING_CHARACTERS: usize = 6;
const GENERATIVE_MAX_READING_CHARACTERS: usize = 32;
const WHOLE_RESULT_MAX_READING_CHARACTERS: usize = 40;
const LONG_WHOLE_RESULT_MIN_COST_GAP: i32 = 500;
const GENERATIVE_CONSTRAINED_CANDIDATE_LIMIT: usize = 8;
const GENERATIVE_MIN_CHANGED_REGIONS: usize = 2;
const GENERATIVE_MAX_CHANGED_REGIONS: usize = 4;
const GENERATIVE_MAX_CHANGED_CHARACTERS_PER_REGION: usize = 2;
const GENERATIVE_MAX_COMPRESSION_CHARACTERS_PER_REGION: usize = 4;
const GENERATIVE_MAX_SURFACE_COMPRESSION_CHARACTERS: usize = 2;
const GENERATIVE_CONSENSUS_MIN_MODEL_ADVANTAGE: f64 = 0.1;
const GENERATIVE_LOCAL_CONSENSUS_MAX_MODEL_ADVANTAGE: f64 = 0.2;
const GENERATIVE_MULTI_REGION_CONSENSUS_MAX_MODEL_ADVANTAGE: f64 = 0.25;
const GENERATIVE_EXTENDED_MULTI_REGION_COST_GAP: i32 = 3_100;
const GENERATIVE_MODEL_VERIFIED_WHOLE_COST_GAP: i32 = 3_100;
const GENERATIVE_MODEL_VERIFIED_WHOLE_MARGIN: f64 = 1.8;
const GENERATIVE_FOREIGN_PREFIX_MIN_CHARACTERS: usize = 4;
const GENERATIVE_FOREIGN_PREFIX_MAX_CHARACTERS: usize = 12;
const GENERATIVE_FOREIGN_PREFIX_MAX_BASE_KATAKANA: usize = 3;

fn accepts_whole_result_cost(reading_characters: usize, cost_gap: i32) -> bool {
    cost_gap <= RESCORE_MAX_BASE_COST_GAP
        && (reading_characters <= GENERATIVE_MAX_READING_CHARACTERS
            || (reading_characters <= WHOLE_RESULT_MAX_READING_CHARACTERS
                && cost_gap >= LONG_WHOLE_RESULT_MIN_COST_GAP))
}

fn is_quoted_span(left_context: &str, right_context: &str) -> bool {
    const QUOTE_PAIRS: [(char, char); 6] = [
        ('「', '」'),
        ('『', '』'),
        ('“', '”'),
        ('‘', '’'),
        ('《', '》'),
        ('〈', '〉'),
    ];
    let quote_characters = |character: &char| {
        QUOTE_PAIRS
            .iter()
            .any(|&(open, close)| *character == open || *character == close)
    };
    let Some(left_boundary) = left_context.chars().rev().find(quote_characters) else {
        return false;
    };
    let Some(right_boundary) = right_context.chars().find(quote_characters) else {
        return false;
    };
    QUOTE_PAIRS.contains(&(left_boundary, right_boundary))
}

/// A complete generated lattice path awaiting the stricter model-only gates.
/// Ordinary dictionary candidates never pass through this verifier.
struct ModelVerifiedCandidate<'a> {
    dictionary: &'a Dictionary,
    reading: &'a str,
    base_surface: &'a str,
    generated_surface: &'a str,
    conversion: &'a Conversion,
    cost_gap: i32,
    structurally_bounded: bool,
    quoted_span: bool,
}

/// One model-proposed foreign-looking prefix followed by a Japanese suffix.
struct ForeignPrefix {
    characters: usize,
    reading_bytes: usize,
    suffix: String,
    suffix_reading: String,
}

fn foreign_prefix(conversion: &Conversion) -> Option<ForeignPrefix> {
    let mut prefix_characters = 0_usize;
    let mut prefix_segments = 0_usize;
    let mut prefix_reading_bytes = 0_usize;
    for segment in &conversion.segments {
        if segment.surface.is_empty()
            || segment.surface != text_transform::full_katakana(&segment.reading)
            || !segment.surface.chars().all(is_full_katakana_or_mark)
        {
            break;
        }
        prefix_characters += segment.surface.chars().count();
        prefix_segments += 1;
        prefix_reading_bytes += segment.reading.len();
    }
    if prefix_segments < 2
        || !(GENERATIVE_FOREIGN_PREFIX_MIN_CHARACTERS..=GENERATIVE_FOREIGN_PREFIX_MAX_CHARACTERS)
            .contains(&prefix_characters)
    {
        return None;
    }
    let suffix = conversion.segments[prefix_segments..]
        .iter()
        .map(|segment| segment.surface.as_str())
        .collect::<String>();
    let suffix_reading = conversion.segments[prefix_segments..]
        .iter()
        .map(|segment| segment.reading.as_str())
        .collect::<String>();
    suffix
        .chars()
        .next()
        .is_some_and(|character| is_hiragana(character) || is_kanji(character))
        .then_some(ForeignPrefix {
            characters: prefix_characters,
            reading_bytes: prefix_reading_bytes,
            suffix,
            suffix_reading,
        })
}

fn conversion_surface_split(
    conversion: &Conversion,
    prefix_reading_bytes: usize,
) -> Option<(String, String)> {
    let mut consumed_reading_bytes = 0_usize;
    let suffix_start = conversion
        .segments
        .iter()
        .enumerate()
        .find_map(|(index, segment)| {
            consumed_reading_bytes += segment.reading.len();
            (consumed_reading_bytes == prefix_reading_bytes).then_some(index + 1)
        })?;
    (consumed_reading_bytes == prefix_reading_bytes).then(|| {
        let join_surface = |segments: &[Segment]| {
            segments
                .iter()
                .map(|segment| segment.surface.as_str())
                .collect::<String>()
        };
        (
            join_surface(&conversion.segments[..suffix_start]),
            join_surface(&conversion.segments[suffix_start..]),
        )
    })
}

impl ModelVerifiedCandidate<'_> {
    fn accepts(self) -> bool {
        self.accepts_whole_surface() || self.accepts_foreign_prefix()
    }

    fn accepts_whole_surface(&self) -> bool {
        !self.structurally_bounded
            && self.base_surface.chars().count() == self.generated_surface.chars().count()
            && self.cost_gap > RESCORE_MAX_BASE_COST_GAP
            && self.cost_gap <= GENERATIVE_MODEL_VERIFIED_WHOLE_COST_GAP
            && !self.quoted_span
            && preserves_ascii_alphanumerics(self.base_surface, self.generated_surface)
            && preserves_kanji_from_hiragana_deconversion(self.base_surface, self.generated_surface)
            && !self
                .dictionary
                .changes_exact_personal_name_or_region_segment(
                    self.reading,
                    self.base_surface,
                    self.generated_surface,
                )
    }

    fn accepts_foreign_prefix(&self) -> bool {
        if self.structurally_bounded
            || self.quoted_span
            || self.cost_gap <= RESCORE_MAX_BASE_COST_GAP
            || self.cost_gap > GENERATIVE_MODEL_VERIFIED_WHOLE_COST_GAP
            || !preserves_ascii_alphanumerics(self.base_surface, self.generated_surface)
        {
            return false;
        }
        let Some(prefix) = foreign_prefix(self.conversion) else {
            return false;
        };
        let base_prefix_characters = self
            .base_surface
            .chars()
            .take_while(|character| is_full_katakana_or_mark(*character))
            .count();
        if base_prefix_characters >= prefix.characters
            || base_prefix_characters > GENERATIVE_FOREIGN_PREFIX_MAX_BASE_KATAKANA
        {
            return false;
        }
        let Some(base_conversion) = self
            .dictionary
            .convert_n_best_with_surface_prefix(
                self.reading,
                self.base_surface,
                GENERATIVE_CONSTRAINED_CANDIDATE_LIMIT,
            )
            .into_iter()
            .find(|candidate| candidate.surface == self.base_surface)
        else {
            return false;
        };
        let Some((base_prefix, base_suffix)) =
            conversion_surface_split(&base_conversion, prefix.reading_bytes)
        else {
            return false;
        };
        if base_prefix.chars().count() >= 2 && base_prefix.chars().all(is_kanji) {
            return false;
        }
        if prefix.suffix == base_suffix {
            return true;
        }
        bounded_local_substitution(
            &base_suffix,
            &prefix.suffix,
            PREFIX_CORRECTION_MAX_CHANGED_CHARACTERS,
        ) && preserves_kanji_from_hiragana_deconversion(&base_suffix, &prefix.suffix)
            && (!self
                .dictionary
                .changes_exact_personal_name_or_region_segment(
                    &prefix.suffix_reading,
                    &base_suffix,
                    &prefix.suffix,
                )
                || self
                    .dictionary
                    .has_exact_region_surface(&prefix.suffix_reading, &prefix.suffix))
    }
}

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
/// Base dictionary candidates and model-rescore-only supplemental entries can
/// be exposed here. Candidates promoted by the user dictionary, history, an
/// installed context rule, or typo correction remain outside this request and
/// cannot be displaced by an external model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRescoreRequest {
    pub context: String,
    pub right_context: String,
    pub reading: String,
    pub candidates: Vec<String>,
}

impl CandidateRescoreRequest {
    /// Whether this request uses the engine's measured long-input rescore path.
    #[must_use]
    pub fn is_long_input(&self) -> bool {
        self.reading.chars().count() >= LONG_RESCORE_READING_CHARACTERS
    }
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
    Deepened,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerativeConsensusKind {
    Local,
    MultiRegion,
    ExtendedMultiRegion,
    ModelVerifiedWhole,
    Whole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GenerativeConsensus {
    candidate: usize,
    kind: GenerativeConsensusKind,
    accepts_whole_result: bool,
}

#[derive(Clone, Debug)]
struct CandidateRescoreState {
    request: CandidateRescoreRequest,
    candidates: Vec<Candidate>,
    model_supplemental: Vec<bool>,
    generative_consensus: Option<GenerativeConsensus>,
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
    model_rescore_dictionary: Option<Dictionary>,
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
            model_rescore_dictionary: None,
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
        let (dictionary, model_rescore_dictionary) =
            bundled_dictionaries_with_packs(0, &user_data, &installed_packs);
        let mut engine = Self::with_user_data(dictionary, user_data);
        engine.model_rescore_dictionary = model_rescore_dictionary;
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
            (self.dictionary, self.model_rescore_dictionary) = bundled_dictionaries_with_packs(
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
            (self.dictionary, self.model_rescore_dictionary) = bundled_dictionaries_with_packs(
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

    /// Prepares the pending pool after an optional scorer becomes ready.
    ///
    /// Long readings receive a wider bounded search. Short readings in the
    /// generative-recall window are revisited for unknown katakana runs only
    /// when the base result has low lattice confidence; shorter readings are
    /// revisited only when a model-rescore-only dictionary pack is installed.
    /// The visible result remains untouched unless scoring succeeds. A missing
    /// or not-yet-ready optional model cannot add latency, and a scoring
    /// failure cannot partially publish the prepared result.
    pub fn prepare_extended_candidate_rescore(&mut self) {
        self.prepare_extended_candidate_rescore_with_limit(
            DEFAULT_EXTENDED_LONG_RESCORE_CANDIDATES,
        );
    }

    /// Prepares a profile-selected long-reading pool for a ready local scorer.
    ///
    /// The requested size is bounded so callers cannot expand the search or
    /// neural runtime beyond the product's measured 32-candidate ceiling.
    pub fn prepare_extended_candidate_rescore_with_limit(&mut self, requested_candidates: usize) {
        self.prepare_extended_candidate_rescore_with_limit_and_confidence(
            requested_candidates,
            requested_candidates,
            false,
        );
    }

    /// Prepares a profile-selected pool with an optional high-accuracy
    /// confidence override and model-only supplemental vocabulary.
    ///
    /// The override applies only to long readings without right context. It
    /// never bypasses personalized, rule-based, typo-correction, or
    /// non-conversion candidates. Its separate candidate bound avoids paying
    /// the full ambiguous-reading pool cost for an otherwise decisive input.
    pub fn prepare_extended_candidate_rescore_with_limit_and_confidence(
        &mut self,
        requested_candidates: usize,
        confidence_bypass_candidates: usize,
        bypass_long_input_confidence: bool,
    ) {
        let reading_characters = self.reading.chars().count();
        let is_long_input = reading_characters >= LONG_RESCORE_READING_CHARACTERS;
        let supports_katakana_model_recall =
            reading_characters >= GENERATIVE_MIN_READING_CHARACTERS;
        if self.candidate_kind != Some(CandidateKind::Conversion)
            || self.selected != 0
            || (!is_long_input
                && !supports_katakana_model_recall
                && self.model_rescore_dictionary.is_none())
            || !self.candidate_corrections.is_empty()
        {
            return;
        }
        let reading = self.reading.clone();
        let candidate_limit = if is_long_input {
            requested_candidates.clamp(
                LONG_RESCORE_CANDIDATE_LIMIT,
                MAX_EXTENDED_LONG_RESCORE_CANDIDATES,
            )
        } else {
            SHORT_RESCORE_CANDIDATE_LIMIT
        };
        if self.candidate_rescore.is_some() {
            self.candidate_rescore = self.prepared_rescore_from_current(
                &reading,
                candidate_limit,
                bypass_long_input_confidence,
            );
            return;
        }
        if self.model_rescore_dictionary.is_some() {
            self.candidate_rescore = self.prepared_rescore_without_current(
                &reading,
                candidate_limit,
                bypass_long_input_confidence,
            );
            if self.candidate_rescore.is_some() || !is_long_input {
                return;
            }
        }
        if !bypass_long_input_confidence
            || self
                .session_history
                .right_surface()
                .is_some_and(|right| !right.is_empty())
        {
            return;
        }
        let confidence_bypass_limit = confidence_bypass_candidates.clamp(
            LONG_RESCORE_CANDIDATE_LIMIT,
            MAX_EXTENDED_LONG_RESCORE_CANDIDATES,
        );
        self.candidate_rescore = self.prepared_rescore_without_current(
            &reading,
            confidence_bypass_limit,
            bypass_long_input_confidence,
        );
    }

    fn prepared_rescore_from_current(
        &self,
        reading: &str,
        candidate_limit: usize,
        bypass_long_input_confidence: bool,
    ) -> Option<CandidateRescoreState> {
        let current = self.candidate_rescore.as_ref()?;
        let base_winner = current.candidates.first()?.clone();
        if self.model_rescore_dictionary.is_none()
            && reading.chars().count() < LONG_RESCORE_READING_CHARACTERS
            && base_winner.cost < SHORT_KATAKANA_RECALL_MIN_BASE_COST
            && !has_short_initial_katakana_run(&base_winner.surface)
        {
            return Some(current.clone());
        }
        let context = &current.request.context;
        let right_context = &current.request.right_context;
        let previous_surface = (!context.is_empty()).then_some(context.as_str());
        let base_candidates = Self::dictionary_candidates_for_context_from(
            &self.dictionary,
            reading,
            Some(candidate_limit),
            previous_surface,
            right_context,
        );
        let model_dictionary = self
            .model_rescore_dictionary
            .as_ref()
            .unwrap_or(&self.dictionary);
        let model_recall_dictionary = model_dictionary.with_model_recall_katakana_cost();
        let recall_candidate_limit =
            model_katakana_recall_search_limit(candidate_limit, &base_winner.surface);
        let model_candidates = Self::dictionary_candidates_for_context_from(
            model_dictionary,
            reading,
            Some(candidate_limit),
            previous_surface,
            right_context,
        );
        let recall_candidates = Self::dictionary_candidates_for_context_from(
            &model_recall_dictionary,
            reading,
            Some(recall_candidate_limit),
            previous_surface,
            right_context,
        );
        if self.model_rescore_dictionary.is_none()
            && reading.chars().count() < LONG_RESCORE_READING_CHARACTERS
            && !recall_candidates.iter().any(|candidate| {
                is_model_katakana_recall_surface(&candidate.surface, &base_winner.surface)
                    && !base_candidates
                        .iter()
                        .any(|base| base.surface == candidate.surface)
            })
        {
            return Some(current.clone());
        }
        let state = candidate_rescore_state_with_limit(
            reading,
            context,
            right_context,
            false,
            &model_candidates,
            candidate_limit,
            bypass_long_input_confidence,
        )?;
        let mut state =
            anchor_model_rescore_state(state, base_winner, &base_candidates, candidate_limit)?;
        append_model_katakana_recall_candidates(
            &mut state,
            &recall_candidates,
            &base_candidates,
            candidate_limit,
        );
        Some(state)
    }

    fn prepared_rescore_without_current(
        &self,
        reading: &str,
        candidate_limit: usize,
        bypass_long_input_confidence: bool,
    ) -> Option<CandidateRescoreState> {
        let (previous_surface, right_context) = self.conversion_contexts(None);
        let base_candidates = Self::dictionary_candidates_for_context_from(
            &self.dictionary,
            reading,
            Some(candidate_limit),
            previous_surface,
            right_context,
        );
        let base_winner = base_candidates.first()?.clone();
        let model_dictionary = self
            .model_rescore_dictionary
            .as_ref()
            .unwrap_or(&self.dictionary);
        let model_recall_dictionary = model_dictionary.with_model_recall_katakana_cost();
        let recall_candidate_limit =
            model_katakana_recall_search_limit(candidate_limit, &base_winner.surface);
        let state = self
            .conversion_candidate_set_for_reading_with_limit_and_context_policy_from(
                model_dictionary,
                reading,
                Some(candidate_limit),
                None,
                bypass_long_input_confidence,
                Some(candidate_limit),
            )
            .rescore?;
        let mut state =
            anchor_model_rescore_state(state, base_winner, &base_candidates, candidate_limit)?;
        let recall_candidates = Self::dictionary_candidates_for_context_from(
            &model_recall_dictionary,
            reading,
            Some(recall_candidate_limit),
            previous_surface,
            right_context,
        );
        append_model_katakana_recall_candidates(
            &mut state,
            &recall_candidates,
            &base_candidates,
            candidate_limit,
        );
        Some(state)
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
        self.apply_candidate_rescore_internal(log_likelihoods, None, None, lambda, minimum_margin)
    }

    /// Whether the pending request is inside the bounded generative-recall
    /// reading window. Callers can avoid model generation outside this gate;
    /// [`Self::prepare_generative_rescore_candidate`] repeats the validation.
    #[must_use]
    pub fn candidate_rescore_supports_generative_recall(&self) -> bool {
        self.candidate_kind == Some(CandidateKind::Conversion)
            && self.selected == 0
            && self.candidate_rescore.as_ref().is_some_and(|state| {
                !requires_dictionary_only_context_ranking(state)
                    && (GENERATIVE_MIN_READING_CHARACTERS..=GENERATIVE_MAX_READING_CHARACTERS)
                        .contains(&state.request.reading.chars().count())
            })
    }

    /// Whether this request only exists because confirmed left context widened
    /// the ordinary confidence gate. Such requests may reorder the bounded
    /// dictionary pool, but must not introduce generated or prefix-followup
    /// surfaces.
    #[must_use]
    pub fn candidate_rescore_requires_dictionary_only_ranking(&self) -> bool {
        self.candidate_rescore
            .as_ref()
            .is_some_and(requires_dictionary_only_context_ranking)
    }

    /// Whether contextual scoring would break exact dictionary structure that
    /// the context-ablated model and base ranker both preserve. This covers a
    /// fragmented ideographic segment and an exact phrase spanning the caret.
    /// The caller can retain the ablated scores without a third model pass when
    /// either conservative boundary is crossed.
    #[must_use]
    pub fn candidate_rescore_should_use_context_ablated_scores(
        &self,
        contextual_log_likelihoods: &[f64],
        context_ablated_log_likelihoods: &[f64],
        lambda: f64,
        minimum_margin: f64,
    ) -> bool {
        let Some(state) = self.candidate_rescore.as_ref() else {
            return false;
        };
        if state.request.context.is_empty() && state.request.right_context.is_empty() {
            return false;
        }
        let Some((_, _, contextual)) = candidate_rescore_order_for_state(
            state,
            contextual_log_likelihoods,
            lambda,
            minimum_margin,
        ) else {
            return false;
        };
        if self
            .safe_whole_result_candidate(state, &state.candidates[contextual].surface)
            .is_some_and(|whole| {
                state.candidates[whole].surface != state.candidates[contextual].surface
            })
        {
            return false;
        }
        if state.candidates.len() != context_ablated_log_likelihoods.len()
            || context_ablated_log_likelihoods
                .iter()
                .any(|score| !score.is_finite())
        {
            return false;
        }
        let mut ablated_order = (0..state.candidates.len()).collect::<Vec<_>>();
        ablated_order.sort_by(|&left, &right| {
            context_ablated_log_likelihoods[right].total_cmp(&context_ablated_log_likelihoods[left])
        });
        let Some((&ablated, runner_up)) = ablated_order
            .split_first()
            .and_then(|(top, rest)| rest.first().map(|runner_up| (top, *runner_up)))
        else {
            return false;
        };
        let Some((_, _, ablated_selected)) =
            candidate_rescore_order_for_state(state, context_ablated_log_likelihoods, lambda, 0.0)
        else {
            return false;
        };
        if ablated_selected != ablated
            || state.candidates[ablated].surface == state.candidates[contextual].surface
        {
            return false;
        }
        let preserves_fragmented_exact_segment = context_ablated_log_likelihoods[ablated]
            - context_ablated_log_likelihoods[runner_up]
            >= CONTEXT_ABLATED_EXACT_FRAGMENT_MIN_MODEL_MARGIN
            && bounded_local_substitution(
                &state.candidates[ablated].surface,
                &state.candidates[contextual].surface,
                PREFIX_CORRECTION_MAX_CHANGED_CHARACTERS,
            )
            && self
                .dictionary
                .fragments_exact_ideographic_segment_into_hiragana(
                    &state.request.reading,
                    &state.candidates[ablated].surface,
                    &state.candidates[contextual].surface,
                );
        let preserves_exact_right_phrase = ablated == 0
            && self.dictionary.has_exact_right_phrase_continuation(
                &state.request.reading,
                &state.candidates[ablated].surface,
                &state.request.right_context,
            )
            && !self.dictionary.has_exact_right_phrase_continuation(
                &state.request.reading,
                &state.candidates[contextual].surface,
                &state.request.right_context,
            );
        preserves_fragmented_exact_segment || preserves_exact_right_phrase
    }

    /// Whether an already-scored long N-best winner justifies one delayed
    /// greedy verification. This is only a generation pre-gate; the generated
    /// surface must still pass [`Self::prepare_generative_rescore_candidate`].
    #[must_use]
    pub fn candidate_rescore_supports_delayed_long_generation(
        &self,
        log_likelihoods: &[f64],
    ) -> bool {
        if self.candidate_kind != Some(CandidateKind::Conversion) || self.selected != 0 {
            return false;
        }
        let Some(state) = self.candidate_rescore.as_ref() else {
            return false;
        };
        if requires_dictionary_only_context_ranking(state) {
            return false;
        }
        if state.candidates.len() != log_likelihoods.len()
            || log_likelihoods.iter().any(|score| !score.is_finite())
        {
            return false;
        }
        let reading_characters = state.request.reading.chars().count();
        if !((GENERATIVE_MAX_READING_CHARACTERS + 1)..=WHOLE_RESULT_MAX_READING_CHARACTERS)
            .contains(&reading_characters)
        {
            return false;
        }
        let Some((winner, _)) = log_likelihoods
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
        else {
            return false;
        };
        if winner == 0 || state.model_supplemental[winner] {
            return false;
        }
        let Some(base) = state.candidates.first() else {
            return false;
        };
        (LONG_WHOLE_RESULT_MIN_COST_GAP..=RESCORE_MAX_BASE_COST_GAP).contains(
            &state.candidates[winner]
                .cost
                .saturating_sub(base.cost)
                .max(0),
        )
    }

    /// Records agreement with an existing candidate, or adds one generated
    /// surface after proving it is a bounded path through the base lattice.
    ///
    /// The generated text is never accepted directly: its complete surface
    /// must be a path through the base lattice. An otherwise unrestricted path
    /// must stay inside the strict base-confidence cost gap. Structurally
    /// bounded paths may use the ordinary model-candidate window: they preserve
    /// ASCII alphanumerics and change two to four regions, with at most two
    /// characters per equal-length region. A surface compression may remove at
    /// most two characters overall and align at most four characters per side
    /// in each region. A bounded equal-length multi-region path may use a
    /// separately evaluated cost window and records direct generation
    /// consensus. A same-length path just outside the strict base window may
    /// join as model-verified whole-result evidence only after preserving
    /// ASCII alphanumerics, existing kanji, and dictionary-confirmed personal
    /// names. It must later beat every scored candidate by a separately
    /// evaluated raw-model margin. Other new candidates are marked as model
    /// supplemental, so the usual additional score margin still applies. A
    /// four- to twelve-character foreign-looking prefix may also join when
    /// it consists of multiple katakana lattice segments followed by an
    /// unchanged or tightly bounded Japanese suffix. Existing all-kanji words
    /// and established long katakana prefixes remain protected. An existing
    /// candidate is recorded as local or multi-region generation
    /// consensus; it may override the ordinary winner later when their model
    /// scores are a narrow near-tie. Independently, a complete lattice path
    /// inside the strict base cost window records whole-result agreement. It
    /// may replace the final local-correction result only after preserving the
    /// same surface invariants.
    pub fn prepare_generative_rescore_candidate(
        &mut self,
        generated_surface: &str,
    ) -> Option<CandidateRescoreRequest> {
        if self.candidate_kind != Some(CandidateKind::Conversion) || self.selected != 0 {
            return None;
        }
        let state = self.candidate_rescore.as_ref()?;
        let reading = &state.request.reading;
        let reading_characters = reading.chars().count();
        let whole_result_only = reading_characters > GENERATIVE_MAX_READING_CHARACTERS;
        if !(GENERATIVE_MIN_READING_CHARACTERS..=WHOLE_RESULT_MAX_READING_CHARACTERS)
            .contains(&reading_characters)
        {
            return None;
        }
        let base = state.candidates.first()?;
        if let Some(index) = state
            .candidates
            .iter()
            .position(|candidate| candidate.surface == generated_surface)
        {
            let consensus = self.existing_generative_consensus(state, index, generated_surface)?;
            let state = self.candidate_rescore.as_mut()?;
            state.generative_consensus = Some(consensus);
            return Some(state.request.clone());
        }
        if whole_result_only {
            return None;
        }
        let conversion = self
            .dictionary
            .convert_n_best_with_surface_prefix(
                reading,
                generated_surface,
                GENERATIVE_CONSTRAINED_CANDIDATE_LIMIT,
            )
            .into_iter()
            .find(|conversion| conversion.surface == generated_surface)?;
        let cost_gap = conversion.cost.saturating_sub(base.cost).max(0);
        let accepts_whole_result = accepts_whole_result_cost(reading_characters, cost_gap);
        let base_surface = &base.surface;
        let is_multi_region = bounded_multi_region_substitution(base_surface, generated_surface);
        let is_surface_compression =
            bounded_multi_region_surface_compression(base_surface, generated_surface);
        let structurally_bounded = is_multi_region || is_surface_compression;
        let quoted_span = is_quoted_span(&state.request.context, &state.request.right_context);
        let is_model_verified_whole = ModelVerifiedCandidate {
            dictionary: &self.dictionary,
            reading,
            base_surface,
            generated_surface,
            conversion: &conversion,
            cost_gap,
            structurally_bounded,
            quoted_span,
        }
        .accepts();
        let maximum_cost_gap = if !structurally_bounded {
            RESCORE_MAX_BASE_COST_GAP
        } else if reading_characters >= LONG_RESCORE_READING_CHARACTERS {
            LONG_RESCORE_MAX_CANDIDATE_COST_GAP
        } else {
            RESCORE_MAX_CANDIDATE_COST_GAP
        };
        let uses_extended_multi_region_consensus = reading_characters
            >= LONG_RESCORE_READING_CHARACTERS
            && is_multi_region
            && cost_gap > maximum_cost_gap
            && cost_gap <= GENERATIVE_EXTENDED_MULTI_REGION_COST_GAP;
        if cost_gap > maximum_cost_gap
            && !uses_extended_multi_region_consensus
            && !is_model_verified_whole
        {
            return None;
        }

        let state = self.candidate_rescore.as_mut()?;
        if state.candidates.len() >= MAX_EXTENDED_LONG_RESCORE_CANDIDATES {
            state.candidates.pop();
            state.model_supplemental.pop();
            state.request.candidates.pop();
        }
        state.candidates.push(Candidate {
            surface: conversion.surface.clone(),
            cost: conversion.cost,
        });
        state.model_supplemental.push(true);
        state.request.candidates.push(conversion.surface);
        if uses_extended_multi_region_consensus || is_model_verified_whole || accepts_whole_result {
            state.generative_consensus = Some(GenerativeConsensus {
                candidate: state.candidates.len() - 1,
                kind: if uses_extended_multi_region_consensus {
                    GenerativeConsensusKind::ExtendedMultiRegion
                } else if is_model_verified_whole {
                    GenerativeConsensusKind::ModelVerifiedWhole
                } else {
                    GenerativeConsensusKind::Whole
                },
                accepts_whole_result,
            });
        }
        Some(state.request.clone())
    }

    fn existing_generative_consensus(
        &self,
        state: &CandidateRescoreState,
        candidate: usize,
        generated_surface: &str,
    ) -> Option<GenerativeConsensus> {
        let base = state.candidates.first()?;
        let existing = state.candidates.get(candidate)?;
        let existing_cost = if state.model_supplemental.get(candidate).copied()? {
            self.dictionary
                .convert_n_best_with_surface_prefix(
                    &state.request.reading,
                    generated_surface,
                    GENERATIVE_CONSTRAINED_CANDIDATE_LIMIT,
                )
                .into_iter()
                .find(|conversion| conversion.surface == generated_surface)?
                .cost
        } else {
            existing.cost
        };
        let reading_characters = state.request.reading.chars().count();
        let accepts_whole_result = accepts_whole_result_cost(
            reading_characters,
            existing_cost.saturating_sub(base.cost).max(0),
        );
        let kind = if reading_characters > GENERATIVE_MAX_READING_CHARACTERS {
            if !accepts_whole_result {
                return None;
            }
            GenerativeConsensusKind::Whole
        } else if bounded_local_substitution(
            &base.surface,
            generated_surface,
            PREFIX_CORRECTION_MAX_CHANGED_CHARACTERS,
        ) {
            GenerativeConsensusKind::Local
        } else if bounded_multi_region_substitution(&base.surface, generated_surface) {
            GenerativeConsensusKind::MultiRegion
        } else if accepts_whole_result {
            GenerativeConsensusKind::Whole
        } else {
            return None;
        };
        Some(GenerativeConsensus {
            candidate,
            kind,
            accepts_whole_result,
        })
    }

    /// Applies model scores and optional model-directed surface prefixes.
    ///
    /// Prefixes are aligned with the pending request. A prefix can replace the
    /// rescored winner only when constrained lattice search changes at most
    /// two adjacent characters and leaves the rest of the candidate intact.
    /// This keeps token-level model disagreement from rewriting unrelated
    /// parts of a long sentence.
    pub fn apply_candidate_rescore_with_prefix_constraints(
        &mut self,
        log_likelihoods: &[f64],
        prefix_constraints: &[Option<String>],
        lambda: f64,
        minimum_margin: f64,
    ) -> Option<Vec<SlimeAction>> {
        self.apply_candidate_rescore_internal(
            log_likelihoods,
            Some(prefix_constraints),
            None,
            lambda,
            minimum_margin,
        )
    }

    /// Previews the first safe prefix correction as a one-candidate request.
    ///
    /// This does not consume or mutate the pending rescore state. A caller can
    /// ask the same model to diagnose the corrected surface once more, then
    /// pass that follow-up prefix to
    /// [`Self::apply_candidate_rescore_with_prefix_constraints_and_followup`].
    #[must_use]
    pub fn candidate_rescore_prefix_followup_request(
        &self,
        log_likelihoods: &[f64],
        prefix_constraints: &[Option<String>],
        lambda: f64,
        minimum_margin: f64,
    ) -> Option<CandidateRescoreRequest> {
        let state = self.candidate_rescore.as_ref()?;
        if self.candidate_kind != Some(CandidateKind::Conversion)
            || self.selected != 0
            || state.candidates.len() != prefix_constraints.len()
        {
            return None;
        }
        let (_, _, selected) =
            candidate_rescore_order_for_state(state, log_likelihoods, lambda, minimum_margin)?;
        let prefix = prefix_constraints[selected].as_deref()?;
        let current = &state.candidates[selected].surface;
        let correction = self.constrained_local_correction(
            &state.request.reading,
            current,
            prefix,
            &self.candidates,
        )?;
        Some(CandidateRescoreRequest {
            context: state.request.context.clone(),
            right_context: state.request.right_context.clone(),
            reading: state.request.reading.clone(),
            candidates: vec![correction],
        })
    }

    /// Applies initial scores plus one optional follow-up prefix correction.
    ///
    /// Each correction is independently limited to the same adjacent
    /// two-character substitution. If the follow-up is invalid, the safe
    /// first correction is still applied.
    pub fn apply_candidate_rescore_with_prefix_constraints_and_followup(
        &mut self,
        log_likelihoods: &[f64],
        prefix_constraints: &[Option<String>],
        followup_prefix_constraint: Option<&str>,
        lambda: f64,
        minimum_margin: f64,
    ) -> Option<Vec<SlimeAction>> {
        self.apply_candidate_rescore_internal(
            log_likelihoods,
            Some(prefix_constraints),
            followup_prefix_constraint,
            lambda,
            minimum_margin,
        )
    }

    fn constrained_local_correction(
        &self,
        reading: &str,
        current: &str,
        prefix: &str,
        existing_candidates: &[String],
    ) -> Option<String> {
        let is_safe = |correction: &String| {
            bounded_local_substitution(
                current,
                correction,
                PREFIX_CORRECTION_MAX_CHANGED_CHARACTERS,
            ) && preserves_kanji_from_hiragana_deconversion(current, correction)
                && !self
                    .dictionary
                    .changes_exact_personal_name_or_region_segment(reading, current, correction)
                && !existing_candidates.contains(correction)
        };
        let initial = self.dictionary.convert_n_best_with_surface_prefix(
            reading,
            prefix,
            PREFIX_CONSTRAINED_INITIAL_CANDIDATE_LIMIT,
        );
        if let Some(correction) = initial
            .into_iter()
            .map(|conversion| conversion.surface)
            .find(is_safe)
        {
            return Some(correction);
        }
        self.dictionary
            .convert_n_best_with_surface_prefix(
                reading,
                prefix,
                PREFIX_CONSTRAINED_MAX_CANDIDATE_LIMIT,
            )
            .into_iter()
            .map(|conversion| conversion.surface)
            .find(is_safe)
    }

    fn apply_candidate_rescore_internal(
        &mut self,
        log_likelihoods: &[f64],
        prefix_constraints: Option<&[Option<String>]>,
        followup_prefix_constraint: Option<&str>,
        lambda: f64,
        minimum_margin: f64,
    ) -> Option<Vec<SlimeAction>> {
        let state = self.candidate_rescore.take()?;
        if self.candidate_kind != Some(CandidateKind::Conversion)
            || self.selected != 0
            || state.candidates.len() != log_likelihoods.len()
            || prefix_constraints
                .is_some_and(|constraints| constraints.len() != state.candidates.len())
        {
            return None;
        }

        let (mut order, mut margin_protects_base, selected) =
            candidate_rescore_order_for_state(&state, log_likelihoods, lambda, minimum_margin)?;
        if self.rescore_requires_base(&state, selected) {
            return Some(self.candidate_actions());
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
        positions.sort_unstable();

        let mut prefix_correction = prefix_constraints
            .and_then(|constraints| constraints[selected].as_deref())
            .and_then(|prefix| {
                self.constrained_local_correction(
                    &state.request.reading,
                    &state.candidates[selected].surface,
                    prefix,
                    &pending_candidates,
                )
            });
        let mut followup_correction = prefix_correction.as_deref().and_then(|correction| {
            followup_prefix_constraint.and_then(|prefix| {
                self.constrained_local_correction(
                    &state.request.reading,
                    correction,
                    prefix,
                    &pending_candidates,
                )
            })
        });
        let current = followup_correction
            .as_deref()
            .or(prefix_correction.as_deref())
            .unwrap_or(&state.candidates[selected].surface);
        if let Some(whole_result) = self.safe_whole_result_candidate(&state, current) {
            order.retain(|&index| index != whole_result);
            order.insert(0, whole_result);
            margin_protects_base = false;
            prefix_correction = None;
            followup_correction = None;
        }
        if margin_protects_base && prefix_correction.is_none() {
            return Some(self.candidate_actions());
        }
        let correction_position = *positions.first()?;
        for (position, candidate_index) in positions.into_iter().zip(order) {
            if !margin_protects_base {
                pending_candidates[position].clone_from(&state.candidates[candidate_index].surface);
            }
        }
        if let Some(correction) = prefix_correction {
            pending_candidates.insert(correction_position, correction);
            if let Some(followup) = followup_correction {
                pending_candidates.insert(correction_position, followup);
            }
        }
        self.candidates = pending_candidates;
        Some(self.candidate_actions())
    }

    fn rescore_requires_base(&self, state: &CandidateRescoreState, selected: usize) -> bool {
        self.rescore_changes_exact_region_segment(state, selected)
            || self.rescore_changes_uncontextualized_personal_name(state, selected)
            || self.rescore_fragments_exact_katakana_segment(state, selected)
            || rescore_only_expands_ascii_digit_width(
                &state.candidates[0].surface,
                &state.candidates[selected].surface,
            )
            || rescore_changes_calendar_or_clock_ascii_digits(
                &state.candidates[0].surface,
                &state.candidates[selected].surface,
            )
            || rescore_removes_alphanumeric_compound_number(state, selected)
    }

    fn rescore_changes_uncontextualized_personal_name(
        &self,
        state: &CandidateRescoreState,
        selected: usize,
    ) -> bool {
        selected != 0
            && state.request.context.is_empty()
            && self.dictionary.is_exact_full_personal_name_surface(
                &state.request.reading,
                &state.candidates[0].surface,
            )
            && state.candidates[0].surface != state.candidates[selected].surface
    }

    fn rescore_changes_exact_region_segment(
        &self,
        state: &CandidateRescoreState,
        selected: usize,
    ) -> bool {
        let base = &state.candidates[0].surface;
        selected != 0
            && !preserves_kanji_from_hiragana_deconversion(
                base,
                &state.candidates[selected].surface,
            )
            && self.dictionary.changes_exact_region_segment(
                &state.request.reading,
                base,
                &state.candidates[selected].surface,
            )
    }

    fn rescore_fragments_exact_katakana_segment(
        &self,
        state: &CandidateRescoreState,
        selected: usize,
    ) -> bool {
        selected != 0
            && self.dictionary.fragments_exact_katakana_segment(
                &state.request.reading,
                &state.candidates[0].surface,
                &state.candidates[selected].surface,
            )
    }

    fn safe_whole_result_candidate(
        &self,
        state: &CandidateRescoreState,
        current: &str,
    ) -> Option<usize> {
        let consensus = state
            .generative_consensus
            .filter(|consensus| consensus.accepts_whole_result)?;
        let generated = &state.candidates.get(consensus.candidate)?.surface;
        (preserves_ascii_alphanumerics(current, generated)
            && preserves_kanji_from_hiragana_deconversion(current, generated)
            && !self
                .dictionary
                .changes_exact_personal_name_or_region_segment(
                    &state.request.reading,
                    current,
                    generated,
                ))
        .then_some(consensus.candidate)
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
        let installed_words = self
            .installed_packs
            .standard_words()
            .map(|(_, surface)| surface);
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

    fn dictionary_candidates_for_context_from(
        dictionary: &Dictionary,
        reading: &str,
        dictionary_limit: Option<usize>,
        previous_surface: Option<&str>,
        right_context: &str,
    ) -> Vec<Candidate> {
        if previous_surface.is_some() || !right_context.is_empty() {
            return match dictionary_limit {
                Some(limit) => dictionary.candidates_with_surrounding_context_limit(
                    reading,
                    previous_surface.unwrap_or_default(),
                    right_context,
                    limit,
                ),
                None => dictionary.candidates_with_surrounding_context(
                    reading,
                    previous_surface.unwrap_or_default(),
                    right_context,
                ),
            };
        }
        match dictionary_limit {
            Some(limit) => dictionary.candidates_with_limit(reading, limit),
            None => dictionary.candidates(reading),
        }
    }

    fn contextual_dictionary_winner<'a>(
        &self,
        reading: &str,
        has_document_context: bool,
        has_transient_history: bool,
        dictionary_candidates: &'a [Candidate],
    ) -> Option<&'a str> {
        if !has_document_context || !has_transient_history {
            return None;
        }
        dictionary_candidates.first().and_then(|contextual_winner| {
            let ordinary_winner = self.dictionary.candidates_with_limit(reading, 1);
            ordinary_winner
                .first()
                .filter(|ordinary| ordinary.surface != contextual_winner.surface)
                .map(|_| contextual_winner.surface.as_str())
        })
    }

    fn conversion_candidate_set_for_reading_with_limit_and_context(
        &self,
        reading: &str,
        dictionary_limit: Option<usize>,
        explicit_previous_surface: Option<&str>,
    ) -> ConversionCandidateSet {
        self.conversion_candidate_set_for_reading_with_limit_and_context_policy(
            reading,
            dictionary_limit,
            explicit_previous_surface,
            false,
            None,
        )
    }

    fn conversion_candidate_set_for_reading_with_limit_and_context_policy(
        &self,
        reading: &str,
        dictionary_limit: Option<usize>,
        explicit_previous_surface: Option<&str>,
        bypass_long_input_confidence: bool,
        rescore_candidate_limit: Option<usize>,
    ) -> ConversionCandidateSet {
        self.conversion_candidate_set_for_reading_with_limit_and_context_policy_from(
            &self.dictionary,
            reading,
            dictionary_limit,
            explicit_previous_surface,
            bypass_long_input_confidence,
            rescore_candidate_limit,
        )
    }

    fn conversion_candidate_set_for_reading_with_limit_and_context_policy_from(
        &self,
        dictionary: &Dictionary,
        reading: &str,
        dictionary_limit: Option<usize>,
        explicit_previous_surface: Option<&str>,
        bypass_long_input_confidence: bool,
        rescore_candidate_limit: Option<usize>,
    ) -> ConversionCandidateSet {
        let mut candidates = Vec::new();
        let (previous_surface, right_context) = self.conversion_contexts(explicit_previous_surface);
        let (contextual_history, established_history, transient_history) =
            if self.history_is_available() {
                let contextual = self
                    .contextual_history_surfaces_for_reading(reading, explicit_previous_surface);
                let (established, transient) =
                    self.user_data.exact_history_surfaces_by_strength(reading);
                (contextual, established, transient)
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };
        extend_unique(
            &mut candidates,
            self.user_data.exact_dictionary_surfaces(reading),
        );
        extend_unique(&mut candidates, contextual_history);
        extend_unique(&mut candidates, established_history);
        // Explicit dictionary entries and repeated learning are durable user
        // authority. A one-off history surface stays selectable, but when it
        // is also a normal dictionary candidate the context model may still
        // prefer a more natural conversion.
        let mut has_protected_candidates = !candidates.is_empty();
        let dictionary_candidates = Self::dictionary_candidates_for_context_from(
            dictionary,
            reading,
            dictionary_limit,
            previous_surface,
            right_context,
        );
        let contextual_dictionary_winner = self.contextual_dictionary_winner(
            reading,
            previous_surface.is_some() || !right_context.is_empty(),
            !transient_history.is_empty(),
            &dictionary_candidates,
        );
        let should_defer_transient_history = |surface: &str| {
            contextual_dictionary_winner.is_some()
                && dictionary_candidates
                    .iter()
                    .any(|candidate| candidate.surface == surface)
        };
        for surface in &transient_history {
            if !should_defer_transient_history(surface) {
                push_unique(&mut candidates, (*surface).to_owned());
            }
        }
        for (key, surface) in &self.ascii_surfaces {
            if english_reverse::reverse_match(reading, key) == Some(ReverseMatch::Exact) {
                push_unique(&mut candidates, surface.clone());
                has_protected_candidates = true;
            }
        }
        if let Some(surface) = contextual_dictionary_winner {
            push_unique(&mut candidates, surface.to_owned());
        }
        for surface in transient_history {
            if should_defer_transient_history(surface) {
                push_unique(&mut candidates, surface.to_owned());
            }
        }
        // The literal hiragana reading stays selectable; hiding it made
        // single-kana words like み unreachable through the candidate window.
        let dictionary_surfaces: Vec<_> = dictionary_candidates
            .iter()
            .map(|candidate| candidate.surface.as_str())
            .collect();
        if let Some(previous_surface) = previous_surface {
            let mut promoted = 0;
            self.installed_packs
                .visit_contextual_surfaces(previous_surface, reading, |surface| {
                    if dictionary_surfaces.contains(&surface) {
                        has_protected_candidates = true;
                        if !candidates.iter().any(|candidate| candidate == surface) {
                            candidates.push(surface.to_owned());
                            promoted += 1;
                        }
                    }
                    promoted < CONTEXT_RULE_PROMOTION_LIMIT
                });
        }
        let rescore = candidate_rescore_state_with_optional_limit(
            reading,
            previous_surface.unwrap_or_default(),
            right_context,
            has_protected_candidates,
            &dictionary_candidates,
            rescore_candidate_limit,
            bypass_long_input_confidence,
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

    fn conversion_contexts<'a>(
        &'a self,
        explicit_previous_surface: Option<&'a str>,
    ) -> (Option<&'a str>, &'a str) {
        if self.preferences.private_mode {
            return (None, "");
        }
        let previous_surface =
            explicit_previous_surface.or_else(|| self.session_history.previous_surface());
        let right_context = explicit_previous_surface
            .is_none()
            .then(|| self.session_history.right_surface())
            .flatten()
            .unwrap_or_default();
        (previous_surface, right_context)
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
        if self.candidate_kind != Some(CandidateKind::Conversion) {
            return;
        }

        let mut merged = self.candidates.clone();
        let reading_length = self.reading.chars().count();
        match self.conversion_search {
            ConversionSearch::Initial => {
                self.conversion_search = ConversionSearch::Expanded;
                // Keep long-input expansion bounded: this runs only after the
                // user reaches the end of the initial candidate list, never
                // on first show.
                let expanded_n_best = if reading_length <= MAX_EXPANDED_READING_CHARACTERS {
                    SHORT_EXPANDED_N_BEST
                } else {
                    LONG_EXPANDED_N_BEST
                };
                for candidate in self.conversion_candidates_for_reading_with_limit(
                    &self.reading,
                    Some(expanded_n_best),
                ) {
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
                for surface in self
                    .installed_packs
                    .explicit_search_surfaces(&self.reading, EXPLICIT_PACK_CANDIDATE_LIMIT)
                {
                    push_unique(&mut merged, surface);
                }
            }
            ConversionSearch::Expanded if reading_length > MAX_EXPANDED_READING_CHARACTERS => {
                self.conversion_search = ConversionSearch::Deepened;
                for candidate in self.conversion_candidates_for_reading_with_limit(
                    &self.reading,
                    Some(LONG_DEEPENED_N_BEST),
                ) {
                    push_unique(&mut merged, candidate);
                }
            }
            ConversionSearch::Expanded | ConversionSearch::Deepened => return,
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
            // mistake immediately override the confidence gate next time. It
            // is still confirmed document text and therefore useful to the
            // transient prediction context.
            self.session_history.record_transient_surface(&committed);
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
            } else if let Some(previous_surface) = self.session_history.previous_surface() {
                for surface in self
                    .user_data
                    .contextual_completion_surfaces_for_external_surface(
                        previous_surface,
                        &self.reading,
                        9,
                    )
                {
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
            self.session_history.record_commit(reading, surface);
            return;
        }

        let previous = self
            .session_history
            .previous_commit()
            .map(|(reading, surface)| (reading.to_owned(), surface.to_owned()));
        if self.preferences.history_learning {
            if let Some((previous_reading, previous_surface)) = previous.as_ref() {
                self.user_data
                    .record_context(previous_reading, previous_surface, reading, surface);
            }
            if should_record_history(reading, surface) {
                if let Some((previous_reading, previous_surface)) = previous.as_ref() {
                    self.user_data.record_with_preference_context(
                        reading,
                        surface,
                        Some((previous_reading, previous_surface)),
                    );
                } else {
                    self.user_data.record(reading, surface);
                }
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
                if let Some((previous_reading, previous_surface)) = context.as_ref()
                    && !recorded_contexts.iter().any(
                        |(
                            recorded_previous_reading,
                            recorded_previous_surface,
                            recorded_reading,
                            recorded_surface,
                        )| {
                            recorded_previous_reading == previous_reading
                                && recorded_previous_surface == previous_surface
                                && recorded_reading == &segment_reading
                                && recorded_surface == &segment_surface
                        },
                    )
                {
                    self.user_data.record_context(
                        previous_reading,
                        previous_surface,
                        &segment_reading,
                        &segment_surface,
                    );
                    recorded_contexts.push((
                        previous_reading.clone(),
                        previous_surface.clone(),
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
                if let Some((previous_reading, previous_surface)) = context.as_ref() {
                    self.user_data.record_with_preference_context(
                        &segment_reading,
                        &segment_surface,
                        Some((previous_reading, previous_surface)),
                    );
                } else {
                    self.user_data.record(&segment_reading, &segment_surface);
                }
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
        if self.preferences.private_mode {
            self.session_history.reset_context();
            return;
        }
        if !self.preferences.history_learning {
            self.session_history.record_commit(prefix, surface);
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
                self.session_history.record_commit(prefix, surface);
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
    bundled_dictionaries_with_packs(dictionary_packs, user_data, &DictionaryPackStore::default()).0
}

fn bundled_dictionaries_with_packs(
    dictionary_packs: u32,
    user_data: &UserData,
    installed_packs: &DictionaryPackStore,
) -> (Dictionary, Option<Dictionary>) {
    let mut layers = domain_dictionaries::layers(dictionary_packs);
    layers.extend(installed_packs.layers());
    if let Some(user_layer) = domain_dictionaries::user_layer(user_data.dictionary_entries()) {
        layers.push(user_layer);
    }
    let standard = Dictionary::bundled_with_layers(layers.clone());
    let supplemental_layers = installed_packs.model_rescore_layers(&standard);
    if supplemental_layers.is_empty() {
        return (standard, None);
    }
    let mut model_layers = layers;
    model_layers.extend(supplemental_layers);
    let model_rescore = Dictionary::bundled_with_layers(model_layers);
    (standard, Some(model_rescore))
}

fn candidate_rescore_state(
    reading: &str,
    context: &str,
    right_context: &str,
    has_protected_candidates: bool,
    dictionary_candidates: &[Candidate],
    bypass_long_input_confidence: bool,
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
        has_protected_candidates,
        dictionary_candidates,
        candidate_limit,
        bypass_long_input_confidence,
    )
}

fn candidate_rescore_state_with_optional_limit(
    reading: &str,
    context: &str,
    right_context: &str,
    has_protected_candidates: bool,
    dictionary_candidates: &[Candidate],
    candidate_limit: Option<usize>,
    bypass_long_input_confidence: bool,
) -> Option<CandidateRescoreState> {
    candidate_limit.map_or_else(
        || {
            candidate_rescore_state(
                reading,
                context,
                right_context,
                has_protected_candidates,
                dictionary_candidates,
                bypass_long_input_confidence,
            )
        },
        |candidate_limit| {
            candidate_rescore_state_with_limit(
                reading,
                context,
                right_context,
                has_protected_candidates,
                dictionary_candidates,
                candidate_limit,
                bypass_long_input_confidence,
            )
        },
    )
}

fn candidate_rescore_state_with_limit(
    reading: &str,
    context: &str,
    right_context: &str,
    has_protected_candidates: bool,
    dictionary_candidates: &[Candidate],
    candidate_limit: usize,
    bypass_long_input_confidence: bool,
) -> Option<CandidateRescoreState> {
    if has_protected_candidates {
        return None;
    }
    if dictionary_candidates.first().is_some_and(|candidate| {
        confirmed_parallel_percentage(context, right_context, &candidate.surface)
    }) {
        return None;
    }
    let base_cost = dictionary_candidates.first()?.cost;
    let uses_short_confirmed_context = !context.is_empty()
        && reading.chars().count() <= SHORT_CONFIRMED_CONTEXT_RESCORE_MAX_READING_CHARACTERS;
    // Multi-segment paths accumulate a wider base-cost spread than short
    // homophones. Let the ready local model inspect that bounded tail without
    // weakening the conservative window used by short readings.
    let max_candidate_cost_gap = if reading.chars().count() >= LONG_RESCORE_READING_CHARACTERS {
        LONG_RESCORE_MAX_CANDIDATE_COST_GAP
    } else if uses_short_confirmed_context {
        SHORT_CONFIRMED_CONTEXT_RESCORE_COST_GAP
    } else {
        RESCORE_MAX_CANDIDATE_COST_GAP
    };
    let candidates: Vec<_> = dictionary_candidates
        .iter()
        .take(candidate_limit)
        .take_while(|candidate| {
            candidate.cost.saturating_sub(base_cost).max(0) <= max_candidate_cost_gap
        })
        .cloned()
        .collect();
    let first = candidates.first()?.cost;
    let alternative = candidates
        .iter()
        .skip(1)
        .map(|candidate| candidate.cost)
        .min()?;
    let bypasses_base_confidence = bypass_long_input_confidence
        && right_context.is_empty()
        && reading.chars().count() >= LONG_RESCORE_READING_CHARACTERS;
    let max_base_confidence_gap = if uses_short_confirmed_context {
        SHORT_CONFIRMED_CONTEXT_RESCORE_COST_GAP
    } else {
        RESCORE_MAX_BASE_COST_GAP
    };
    if alternative.saturating_sub(first).max(0) > max_base_confidence_gap
        && !bypasses_base_confidence
    {
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
        model_supplemental: vec![false; candidates.len()],
        generative_consensus: None,
        candidates,
    })
}

fn requires_dictionary_only_context_ranking(state: &CandidateRescoreState) -> bool {
    !state.request.context.is_empty()
        && state.request.reading.chars().count()
            <= SHORT_CONFIRMED_CONTEXT_RESCORE_MAX_READING_CHARACTERS
        && state.candidates.first().is_some_and(|base| {
            state
                .candidates
                .iter()
                .skip(1)
                .map(|candidate| candidate.cost)
                .min()
                .is_some_and(|alternative| {
                    alternative.saturating_sub(base.cost).max(0) > RESCORE_MAX_BASE_COST_GAP
                })
        })
}

fn confirmed_parallel_percentage(left_context: &str, right_context: &str, surface: &str) -> bool {
    fn is_decimal_digit(character: char) -> bool {
        matches!(character, '0'..='9' | '０'..='９')
    }

    fn has_trailing_percentage(text: &str) -> bool {
        let text = text.trim_end();
        let Some(before_separator) = text
            .strip_suffix('、')
            .or_else(|| text.strip_suffix('，'))
            .or_else(|| text.strip_suffix(','))
        else {
            return false;
        };
        let text = before_separator.trim_end();
        let Some(before_percent) = text.strip_suffix('%').or_else(|| text.strip_suffix('％'))
        else {
            return false;
        };
        let mut characters = before_percent.chars().rev().peekable();
        let mut fractional_digits = 0;
        while characters
            .peek()
            .is_some_and(|character| is_decimal_digit(*character))
        {
            characters.next();
            fractional_digits += 1;
        }
        if fractional_digits == 0 {
            return false;
        }
        if characters
            .peek()
            .is_some_and(|character| matches!(character, '.' | '．' | '・'))
        {
            characters.next();
            return characters.next().is_some_and(is_decimal_digit);
        }
        true
    }

    fn starts_with_fractional_percentage(text: &str) -> bool {
        let mut characters = text.trim_start().chars();
        if !characters
            .next()
            .is_some_and(|character| matches!(character, '.' | '．' | '・'))
        {
            return false;
        }
        let mut digits = 0;
        for character in characters.by_ref() {
            if is_decimal_digit(character) {
                digits += 1;
            } else {
                return digits > 0 && matches!(character, '%' | '％');
            }
        }
        false
    }

    has_trailing_percentage(left_context)
        && starts_with_fractional_percentage(right_context)
        && surface.chars().next_back().is_some_and(is_decimal_digit)
}

/// Full-width digits remain an explicit transform/candidate choice. Neural
/// rescoring may resolve the surrounding words, but it must not replace the
/// converter's default ASCII number with an otherwise identical full-width
/// spelling. Such a rewrite has no semantic evidence and makes codes and years
/// inconsistent with the deterministic candidate order.
fn rescore_only_expands_ascii_digit_width(base: &str, selected: &str) -> bool {
    if base.chars().count() != selected.chars().count() {
        return false;
    }
    let mut expanded = false;
    for (base, selected) in base.chars().zip(selected.chars()) {
        if base == selected {
            continue;
        }
        if base.is_ascii_digit()
            && char::from_u32(u32::from(base) - u32::from('0') + u32::from('０')) == Some(selected)
        {
            expanded = true;
            continue;
        }
        return false;
    }
    expanded
}

/// Preserve the converter's explicit ASCII value inside an unambiguous
/// multi-part calendar or clock expression. A local model may improve the
/// words around it, but it must not turn `6月10日` back into `6月トーカ`.
/// Requiring two components deliberately excludes isolated counters such as
/// `1007位`, whose numeric parse may itself be the dictionary error.
fn rescore_changes_calendar_or_clock_ascii_digits(base: &str, selected: &str) -> bool {
    let base_characters = base.chars().collect::<Vec<_>>();
    let structured_components = base_characters
        .iter()
        .enumerate()
        .filter(|&(index, character)| {
            character.is_ascii_digit()
                && (index == 0 || !base_characters[index - 1].is_ascii_digit())
                && base_characters[index..]
                    .iter()
                    .position(|character| !character.is_ascii_digit())
                    .and_then(|offset| base_characters.get(index + offset))
                    .is_some_and(|character| is_calendar_or_clock_unit(*character))
        })
        .count();
    structured_components >= 2
        && !base
            .chars()
            .filter(char::is_ascii_digit)
            .eq(selected.chars().filter(char::is_ascii_digit))
}

/// Preserve a dictionary-selected number when it completes a compact
/// alphanumeric designation across the caret (for example, a letter followed
/// by a spoken digit and an ideographic suffix). In this structure the
/// confirmed letter and following noun are stronger evidence than a language
/// model's preference for spelling the spoken digit as katakana.
fn rescore_removes_alphanumeric_compound_number(
    state: &CandidateRescoreState,
    selected: usize,
) -> bool {
    if selected == 0
        || state.request.reading.chars().count() > 8
        || !state.request.context.chars().next_back().is_some_and(
            |character| matches!(character, 'A'..='Z' | 'a'..='z' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ'),
        )
        || !state
            .request
            .right_context
            .chars()
            .next()
            .is_some_and(is_ideographic_or_digit)
    {
        return false;
    }
    let base_digits = state.candidates[0]
        .surface
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    !base_digits.is_empty() && !state.candidates[selected].surface.starts_with(&base_digits)
}

fn is_ideographic_or_digit(character: char) -> bool {
    matches!(
        character,
        '0'..='9'
            | '\u{ff10}'..='\u{ff19}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
    )
}

fn is_calendar_or_clock_unit(character: char) -> bool {
    matches!(character, '年' | '月' | '日' | '時' | '分' | '秒')
}

fn anchor_model_rescore_state(
    mut state: CandidateRescoreState,
    base_winner: Candidate,
    base_candidates: &[Candidate],
    candidate_limit: usize,
) -> Option<CandidateRescoreState> {
    state
        .candidates
        .retain(|candidate| candidate.surface != base_winner.surface);
    state.candidates.insert(0, base_winner);
    state.candidates.truncate(candidate_limit);
    if state.candidates.len() < 2 {
        return None;
    }
    state.model_supplemental = state
        .candidates
        .iter()
        .map(|candidate| {
            !base_candidates
                .iter()
                .any(|base| base.surface == candidate.surface)
        })
        .collect();
    state.generative_consensus = None;
    state.request.candidates = state
        .candidates
        .iter()
        .map(|candidate| candidate.surface.clone())
        .collect();
    Some(state)
}

fn append_model_katakana_recall_candidates(
    state: &mut CandidateRescoreState,
    recall_candidates: &[Candidate],
    base_candidates: &[Candidate],
    base_limit: usize,
) {
    let Some(base_surface) = state
        .candidates
        .first()
        .map(|candidate| candidate.surface.clone())
    else {
        return;
    };
    let maximum = base_limit
        .saturating_add(MODEL_KATAKANA_RECALL_ADDITIONAL_CANDIDATES)
        .min(MAX_EXTENDED_LONG_RESCORE_CANDIDATES);
    for candidate in recall_candidates {
        if state.candidates.len() >= maximum {
            break;
        }
        if !is_model_katakana_recall_surface(&candidate.surface, &base_surface)
            || base_candidates
                .iter()
                .any(|base| base.surface == candidate.surface)
            || state
                .candidates
                .iter()
                .any(|existing| existing.surface == candidate.surface)
        {
            continue;
        }
        state.candidates.push(candidate.clone());
        state.model_supplemental.push(true);
        state.request.candidates.push(candidate.surface.clone());
    }
}

fn is_model_katakana_recall_surface(surface: &str, base_surface: &str) -> bool {
    is_mixed_katakana_recall_surface(surface)
        || extends_short_initial_katakana_run(surface, base_surface)
}

fn model_katakana_recall_search_limit(candidate_limit: usize, base_surface: &str) -> usize {
    if has_short_initial_katakana_run(base_surface) {
        candidate_limit.max(SHORT_KATAKANA_RECALL_SEARCH_LIMIT)
    } else {
        candidate_limit
    }
}

fn is_katakana_character(character: char) -> bool {
    matches!(character, '\u{30A1}'..='\u{30FA}' | '\u{30FD}'..='\u{30FF}' | 'ー')
}

fn initial_katakana_run_characters(surface: &str) -> usize {
    surface
        .chars()
        .take_while(|&character| is_katakana_character(character))
        .count()
}

fn has_short_initial_katakana_run(surface: &str) -> bool {
    (2..MODEL_KATAKANA_RECALL_MIN_RUN_CHARACTERS.saturating_sub(1))
        .contains(&initial_katakana_run_characters(surface))
}

fn extends_short_initial_katakana_run(surface: &str, base_surface: &str) -> bool {
    let surface_run = initial_katakana_run_characters(surface);
    let base_run = initial_katakana_run_characters(base_surface);
    surface_run + 1 == MODEL_KATAKANA_RECALL_MIN_RUN_CHARACTERS
        && (2..surface_run).contains(&base_run)
        && surface
            .chars()
            .zip(base_surface.chars())
            .take(base_run)
            .all(|(surface, base)| surface == base)
        && surface
            .chars()
            .skip(surface_run)
            .any(|character| is_hiragana(character) || is_kanji(character))
}

fn is_mixed_katakana_recall_surface(surface: &str) -> bool {
    let mut maximum_katakana_run = 0_usize;
    let mut current_katakana_run = 0_usize;
    let mut has_japanese_non_katakana = false;
    for character in surface.chars() {
        if is_katakana_character(character) {
            current_katakana_run += 1;
            maximum_katakana_run = maximum_katakana_run.max(current_katakana_run);
        } else {
            current_katakana_run = 0;
            has_japanese_non_katakana |= is_hiragana(character) || is_kanji(character);
        }
    }
    maximum_katakana_run >= MODEL_KATAKANA_RECALL_MIN_RUN_CHARACTERS && has_japanese_non_katakana
}

fn candidate_rescore_order(
    candidates: &[Candidate],
    model_supplemental: &[bool],
    log_likelihoods: &[f64],
    lambda: f64,
    minimum_margin: f64,
) -> Option<(Vec<usize>, bool, usize)> {
    if candidates.is_empty()
        || candidates.len() != log_likelihoods.len()
        || candidates.len() != model_supplemental.len()
        || !(0.0..=1.0).contains(&lambda)
        || !lambda.is_finite()
        || minimum_margin < 0.0
        || !minimum_margin.is_finite()
        || log_likelihoods.iter().any(|score| !score.is_finite())
    {
        return None;
    }
    let combined = candidates
        .iter()
        .zip(log_likelihoods)
        .map(|(candidate, log_likelihood)| {
            (1.0 - lambda) * (-f64::from(candidate.cost) / RESCORE_COST_LOG_SCALE)
                + lambda * log_likelihood
        })
        .collect::<Vec<_>>();
    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by(|&left, &right| combined[right].total_cmp(&combined[left]));
    let top = *order.first()?;
    let required_margin = minimum_margin
        + if model_supplemental[top] {
            MODEL_SUPPLEMENTAL_ADDITIONAL_MARGIN
        } else {
            0.0
        };
    let margin_protects_base = top != 0 && combined[top] - combined[0] < required_margin;
    let selected = if margin_protects_base { 0 } else { top };
    Some((order, margin_protects_base, selected))
}

fn candidate_rescore_order_for_state(
    state: &CandidateRescoreState,
    log_likelihoods: &[f64],
    lambda: f64,
    minimum_margin: f64,
) -> Option<(Vec<usize>, bool, usize)> {
    let (mut order, mut margin_protects_base, mut selected) = candidate_rescore_order(
        &state.candidates,
        &state.model_supplemental,
        log_likelihoods,
        lambda,
        minimum_margin,
    )?;
    let Some(consensus) = state.generative_consensus else {
        return Some((order, margin_protects_base, selected));
    };
    if consensus.candidate >= log_likelihoods.len() {
        return None;
    }
    if consensus.kind == GenerativeConsensusKind::ExtendedMultiRegion {
        if !state.model_supplemental[consensus.candidate] {
            return None;
        }
        order.retain(|&index| index != consensus.candidate);
        order.insert(0, consensus.candidate);
        return Some((order, false, consensus.candidate));
    }
    if consensus.kind == GenerativeConsensusKind::ModelVerifiedWhole {
        if !state.model_supplemental[consensus.candidate] {
            return None;
        }
        let candidate_score = log_likelihoods[consensus.candidate];
        let runner_up = log_likelihoods
            .iter()
            .enumerate()
            .filter(|&(index, _)| index != consensus.candidate)
            .map(|(_, score)| *score)
            .max_by(f64::total_cmp)?;
        if candidate_score - runner_up >= GENERATIVE_MODEL_VERIFIED_WHOLE_MARGIN {
            order.retain(|&index| index != consensus.candidate);
            order.insert(0, consensus.candidate);
            return Some((order, false, consensus.candidate));
        }
        // This candidate was admitted beyond the ordinary lattice-confidence
        // window specifically on the promise of dominant raw-model evidence.
        // If that evidence is absent, do not let interpolation or prefix
        // diagnostics select the same supplemental path through a weaker
        // route. Recompute the ordinary order without it and leave the
        // supplemental surface at the end as a selectable alternative.
        let retained = (0..state.candidates.len())
            .filter(|&index| index != consensus.candidate)
            .collect::<Vec<_>>();
        let filtered_candidates = retained
            .iter()
            .map(|&index| state.candidates[index].clone())
            .collect::<Vec<_>>();
        let filtered_supplemental = retained
            .iter()
            .map(|&index| state.model_supplemental[index])
            .collect::<Vec<_>>();
        let filtered_scores = retained
            .iter()
            .map(|&index| log_likelihoods[index])
            .collect::<Vec<_>>();
        let (filtered_order, filtered_margin, filtered_selected) = candidate_rescore_order(
            &filtered_candidates,
            &filtered_supplemental,
            &filtered_scores,
            lambda,
            minimum_margin,
        )?;
        order = filtered_order
            .into_iter()
            .map(|index| retained[index])
            .collect();
        order.push(consensus.candidate);
        return Some((order, filtered_margin, retained[filtered_selected]));
    }
    if consensus.kind == GenerativeConsensusKind::Whole {
        return consensus
            .accepts_whole_result
            .then_some((order, margin_protects_base, selected));
    }
    if state.model_supplemental[consensus.candidate] {
        return consensus
            .accepts_whole_result
            .then_some((order, margin_protects_base, selected));
    }
    let maximum_model_advantage = match consensus.kind {
        GenerativeConsensusKind::Local => GENERATIVE_LOCAL_CONSENSUS_MAX_MODEL_ADVANTAGE,
        GenerativeConsensusKind::MultiRegion => {
            GENERATIVE_MULTI_REGION_CONSENSUS_MAX_MODEL_ADVANTAGE
        }
        GenerativeConsensusKind::ExtendedMultiRegion
        | GenerativeConsensusKind::ModelVerifiedWhole
        | GenerativeConsensusKind::Whole => {
            unreachable!()
        }
    };
    let model_advantage = log_likelihoods[consensus.candidate] - log_likelihoods[selected];
    if consensus.candidate != selected
        && (GENERATIVE_CONSENSUS_MIN_MODEL_ADVANTAGE..=maximum_model_advantage)
            .contains(&model_advantage)
    {
        order.retain(|&index| index != consensus.candidate);
        order.insert(0, consensus.candidate);
        selected = consensus.candidate;
        margin_protects_base = false;
    }
    Some((order, margin_protects_base, selected))
}

fn bounded_local_substitution(current: &str, alternative: &str, maximum_changes: usize) -> bool {
    let current = current.chars().collect::<Vec<_>>();
    let alternative = alternative.chars().collect::<Vec<_>>();
    if current.len() != alternative.len() {
        return false;
    }
    let changed = current
        .iter()
        .zip(&alternative)
        .enumerate()
        .filter_map(|(index, (current, alternative))| {
            (current != alternative).then_some((index, *current, *alternative))
        })
        .collect::<Vec<_>>();
    let Some((&(first, _, _), &(last, _, _))) = changed.first().zip(changed.last()) else {
        return false;
    };
    changed.len() <= maximum_changes
        && last - first + 1 == changed.len()
        && changed.iter().all(|&(_, current, alternative)| {
            !current.is_ascii_alphanumeric() && !alternative.is_ascii_alphanumeric()
        })
}

fn preserves_kanji_from_hiragana_deconversion(current: &str, alternative: &str) -> bool {
    current
        .chars()
        .zip(alternative.chars())
        .all(|(current, alternative)| {
            current == alternative || !is_kanji(current) || !is_hiragana(alternative)
        })
}

fn preserves_ascii_alphanumerics(current: &str, alternative: &str) -> bool {
    current
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .eq(alternative.chars().filter(char::is_ascii_alphanumeric))
}

fn is_kanji(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_hiragana(character: char) -> bool {
    matches!(character, '\u{3041}'..='\u{3096}' | '\u{309D}'..='\u{309F}')
}

fn is_full_katakana_or_mark(character: char) -> bool {
    matches!(
        character,
        '\u{30A1}'..='\u{30FA}' | '\u{30FD}'..='\u{30FF}' | 'ー' | '・'
    )
}

fn bounded_multi_region_substitution(current: &str, alternative: &str) -> bool {
    let current = current.chars().collect::<Vec<_>>();
    let alternative = alternative.chars().collect::<Vec<_>>();
    if current.len() != alternative.len() {
        return false;
    }
    let mut regions = 0usize;
    let mut region_characters = 0usize;
    for (&current, &alternative) in current.iter().zip(&alternative) {
        if current == alternative {
            region_characters = 0;
            continue;
        }
        if current.is_ascii_alphanumeric() || alternative.is_ascii_alphanumeric() {
            return false;
        }
        if region_characters == 0 {
            regions += 1;
            if regions > GENERATIVE_MAX_CHANGED_REGIONS {
                return false;
            }
        }
        region_characters += 1;
        if region_characters > GENERATIVE_MAX_CHANGED_CHARACTERS_PER_REGION {
            return false;
        }
    }
    regions >= GENERATIVE_MIN_CHANGED_REGIONS
}

fn bounded_multi_region_surface_compression(current: &str, alternative: &str) -> bool {
    let current = current.chars().collect::<Vec<_>>();
    let alternative = alternative.chars().collect::<Vec<_>>();
    if current.len() <= alternative.len()
        || current.len() - alternative.len() > GENERATIVE_MAX_SURFACE_COMPRESSION_CHARACTERS
    {
        return false;
    }

    let width = alternative.len() + 1;
    let mut costs = vec![0usize; (current.len() + 1) * width];
    for (row, costs) in costs.chunks_mut(width).enumerate() {
        costs[0] = row;
    }
    for (column, cost) in costs.iter_mut().take(width).enumerate() {
        *cost = column;
    }
    for row in 1..=current.len() {
        for column in 1..=alternative.len() {
            let substitution = costs[(row - 1) * width + column - 1]
                + usize::from(current[row - 1] != alternative[column - 1]);
            let deletion = costs[(row - 1) * width + column] + 1;
            let insertion = costs[row * width + column - 1] + 1;
            costs[row * width + column] = substitution.min(deletion).min(insertion);
        }
    }

    let (mut row, mut column) = (current.len(), alternative.len());
    let mut regions = 0usize;
    let mut inside_region = false;
    let mut current_region_characters = 0usize;
    let mut alternative_region_characters = 0usize;
    while row > 0 || column > 0 {
        let cost = costs[row * width + column];
        if row > 0
            && column > 0
            && current[row - 1] == alternative[column - 1]
            && cost == costs[(row - 1) * width + column - 1]
        {
            inside_region = false;
            current_region_characters = 0;
            alternative_region_characters = 0;
            row -= 1;
            column -= 1;
            continue;
        }
        let (current_character, alternative_character) =
            if row > 0 && column > 0 && cost == costs[(row - 1) * width + column - 1] + 1 {
                row -= 1;
                column -= 1;
                (Some(current[row]), Some(alternative[column]))
            } else if row > 0 && cost == costs[(row - 1) * width + column] + 1 {
                row -= 1;
                (Some(current[row]), None)
            } else if column > 0 && cost == costs[row * width + column - 1] + 1 {
                column -= 1;
                (None, Some(alternative[column]))
            } else {
                return false;
            };
        if current_character.is_some_and(|character| character.is_ascii_alphanumeric())
            || alternative_character.is_some_and(|character| character.is_ascii_alphanumeric())
        {
            return false;
        }
        if !inside_region {
            regions += 1;
            if regions > GENERATIVE_MAX_CHANGED_REGIONS {
                return false;
            }
            inside_region = true;
        }
        current_region_characters += usize::from(current_character.is_some());
        alternative_region_characters += usize::from(alternative_character.is_some());
        if current_region_characters > GENERATIVE_MAX_COMPRESSION_CHARACTERS_PER_REGION
            || alternative_region_characters > GENERATIVE_MAX_COMPRESSION_CHARACTERS_PER_REGION
        {
            return false;
        }
    }
    regions >= GENERATIVE_MIN_CHANGED_REGIONS
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

fn extend_unique<'a>(values: &mut Vec<String>, surfaces: impl IntoIterator<Item = &'a str>) {
    for surface in surfaces {
        push_unique(values, surface.to_owned());
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
        DictionaryPackWord, EnginePreferences, GenerativeConsensus, GenerativeConsensusKind,
        InputEvent, LiveConversionDecision, MAX_EXPANDED_READING_CHARACTERS, Phase, SlimeAction,
        SlimeEngine, TECHNOLOGY_DICTIONARY, UserData, bounded_local_substitution,
        bundled_dictionary, candidate_rescore_order, candidate_rescore_order_for_state,
        date_time_candidates, katakana_candidate, preserves_kanji_from_hiragana_deconversion,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use slime_converter::{Candidate, Dictionary, DictionaryEntry, DictionaryLayer};
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

    fn exact_candidate(dictionary: &Dictionary, reading: &str, surface: &str) -> Candidate {
        dictionary
            .convert_n_best_with_surface_prefix(reading, surface, 1)
            .into_iter()
            .find(|candidate| candidate.surface == surface)
            .map(|candidate| Candidate {
                surface: candidate.surface,
                cost: candidate.cost,
            })
            .expect("exact lattice candidate")
    }

    fn engine_with_rescore_candidates(
        dictionary: Dictionary,
        reading: &str,
        candidates: Vec<Candidate>,
    ) -> SlimeEngine {
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = reading.to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: reading.to_owned(),
                candidates: engine.candidates.clone(),
            },
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
            candidates,
        });
        engine
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

    fn write_model_rescore_pack(directory: &std::path::Path) {
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let payload = "てすとようご\t補助試験語甲\t500\n\
てすとようご\t補助試験語乙\t550\n";
        let digest = lower_hex(&Sha256::digest(payload.as_bytes()));
        fs::write(
            pack_directory.join("sample-model-rescore.slime-dict"),
            format!(
                "# slime-dictionary-pack-v4\n\
                 # id: sample-model-rescore\n\
                 # name: 補助語彙サンプル\n\
                 # version: 2026.08.1\n\
                 # license: Example-Test-Only\n\
                 # minimum-slime-version: 0.1.0\n\
                 # published-at: 2026-08-11\n\
                 # provenance: fixture/generated/sample-model-rescore\n\
                 # candidate-mode: model-rescore-only\n\
                 # payload-sha256: {digest}\n\
                 # entries\n\
                 {payload}"
            ),
        )
        .unwrap();
    }

    fn write_explicit_search_pack(directory: &std::path::Path) {
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        let payload = "てすとようご\t明示試験語甲\t500\n\
てすとようご\t明示試験語乙\t550\n\
ぎっとはぶ\tGitHub\t500\n";
        let digest = lower_hex(&Sha256::digest(payload.as_bytes()));
        fs::write(
            pack_directory.join("sample-explicit-search.slime-dict"),
            format!(
                "# slime-dictionary-pack-v5\n\
                 # id: sample-explicit-search\n\
                 # name: 明示探索語彙サンプル\n\
                 # version: 2026.08.1\n\
                 # license: Example-Test-Only\n\
                 # minimum-slime-version: 0.1.0\n\
                 # published-at: 2026-08-11\n\
                 # provenance: fixture/generated/sample-explicit-search\n\
                 # candidate-mode: explicit-search-only\n\
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
    fn external_right_context_ranks_a_measured_reach_range() {
        let mut engine = SlimeEngine::bundled();

        engine.set_external_context("大阪駅から徒歩10分", "内のホテル");
        type_text(&mut engine, "kenn");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "圏");
    }

    #[test]
    fn contextual_dictionary_winners_outrank_transient_plain_history() {
        let cases = [
            (
                "derivational-suffix",
                "さいき",
                "再起",
                "以上の操作を",
                "的に繰り返す",
                "saiki",
                "再帰",
            ),
            (
                "right-phrase",
                "しんか",
                "進化",
                "事が明らかになった後の対応で",
                "が問われる",
                "shinka",
                "真価",
            ),
            (
                "right-compound",
                "かたく",
                "固く",
                "先日の",
                "捜索が行われた",
                "kataku",
                "家宅",
            ),
            (
                "left-compound",
                "かがく",
                "科学",
                "北部は早くから製鉄・石油",
                "・火力発電が発達した",
                "kagaku",
                "化学",
            ),
            (
                "grammar",
                "わたし",
                "ワタシ",
                "彼らは更に自らの救命胴衣を他の兵士に",
                "た。",
                "watashi",
                "渡し",
            ),
        ];

        for (id, reading, history, left, right, raw_input, expected) in cases {
            let directory = test_directory(id);
            fs::write(
                directory.join("history.tsv"),
                format!("# slime-history-v1\n{reading}\t{history}\t1\t10\n"),
            )
            .unwrap();
            let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
            engine.set_preferences(EnginePreferences {
                history_completion: true,
                ..EnginePreferences::default()
            });
            assert_eq!(engine.conversion_candidates(reading)[0], history, "{id}");

            engine.set_external_context(left, right);
            type_text(&mut engine, raw_input);
            engine.handle(InputEvent::Space);

            let snapshot = engine.snapshot();
            assert_eq!(snapshot.preedit, expected, "{id}: {snapshot:?}");
            assert!(
                snapshot
                    .candidates
                    .iter()
                    .skip(1)
                    .any(|candidate| candidate == history),
                "{id} must keep transient history selectable: {:?}",
                snapshot.candidates
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn established_plain_history_retains_priority_over_document_context() {
        let directory = test_directory("established-history-before-context");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nわたし\tワタシ\t5\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            history_completion: true,
            ..EnginePreferences::default()
        });

        engine.set_external_context("彼らは更に自らの救命胴衣を他の兵士に", "た。");
        type_text(&mut engine, "watashi");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "ワタシ");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn transient_history_outside_the_dictionary_pool_retains_priority() {
        let directory = test_directory("custom-history-before-context");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nわたし\t私達\t1\t10\n",
        )
        .unwrap();
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            history_completion: true,
            ..EnginePreferences::default()
        });

        engine.set_external_context("彼らは更に自らの救命胴衣を他の兵士に", "た。");
        type_text(&mut engine, "watashi");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "私達");
        fs::remove_dir_all(directory).unwrap();
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
    fn external_document_context_reuses_repeated_local_completion_history() {
        let directory = test_directory("external-adaptive-completion");
        fs::write(
            directory.join("context_history.tsv"),
            "# slime-context-history-v1\n\
             ぶんしょう\t文章\tしょうめいけいかく\t証明計画\t10\t10\n\
             へや\t部屋\tしょうめいけいかく\t照明計画\t2\t20\n",
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
        engine.set_external_left_context("既存文書の部屋");

        type_text(&mut engine, "shoumei");

        assert_eq!(engine.snapshot().candidates.first().unwrap(), "照明計画");
        assert_eq!(
            fs::read_to_string(directory.join("context_history.tsv")).unwrap(),
            "# slime-context-history-v1\n\
             ぶんしょう\t文章\tしょうめいけいかく\t証明計画\t10\t10\n\
             へや\t部屋\tしょうめいけいかく\t照明計画\t2\t20\n",
            "reading external context must not persist a learned edge"
        );

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
    fn repeated_exact_selection_changes_the_durable_preference() {
        let directory = test_directory("confirmed-exact-history-preference");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t漢字\t100\t20\n",
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

            if repetition == 0 {
                let mut one_off = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
                one_off.set_preferences(preferences);
                type_text(&mut one_off, "kanji");
                one_off.handle(InputEvent::Space);
                assert_eq!(one_off.snapshot().preedit, "漢字");
            }
        }

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        type_text(&mut reloaded, "kanji");
        reloaded.handle(InputEvent::Space);
        assert_eq!(reloaded.snapshot().preedit, "感じ");
        assert!(
            fs::read_to_string(directory.join("history_preferences.tsv"))
                .unwrap()
                .contains("かんじ\t感じ\t")
        );

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
        assert!(!request.is_long_input());
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
    fn external_scores_receive_accumulated_confirmed_text() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.set_external_context("文書冒頭。", "直後");
        engine.record_history("きょうは", "今日は");
        engine.record_history("はれ", "晴れ");
        engine.record_history("。", "。");
        engine.record_history("つぎ", "次は");

        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("ambiguous dictionary candidates should include context");
        assert_eq!(request.context, "文書冒頭。今日は晴れ。次は");
        assert_eq!(request.right_context, "直後");
        assert_eq!(
            engine.session_history.previous_commit(),
            Some(("つぎ", "次は")),
            "punctuation stays in model context without becoming a learning edge",
        );
    }

    #[test]
    fn unconverted_commit_remains_in_prediction_context_without_learning() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("にほん", "日本", 1_000),
            DictionaryEntry::new("にほん", "二本", 1_100),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.set_external_context("文書冒頭。", "直後");

        type_text(&mut engine, "kakuteizumi");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&SlimeAction::Commit("かくていずみ".to_owned())));

        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("raw confirmed text should remain available to prediction");
        assert_eq!(request.context, "文書冒頭。かくていずみ");
        assert_eq!(request.right_context, "直後");
        assert!(engine.session_history.previous_commit().is_none());
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
    fn supplemental_model_candidate_requires_an_additional_margin() {
        let candidates = [
            Candidate {
                surface: "基本".to_owned(),
                cost: 1_000,
            },
            Candidate {
                surface: "補助".to_owned(),
                cost: 1_000,
            },
        ];
        let (_, ordinary_protected, ordinary_selected) =
            candidate_rescore_order(&candidates, &[false, false], &[0.0, 0.4], 0.8, 0.0).unwrap();
        assert!(!ordinary_protected);
        assert_eq!(ordinary_selected, 1);

        let (_, supplemental_protected, supplemental_selected) =
            candidate_rescore_order(&candidates, &[false, true], &[0.0, 0.4], 0.8, 0.0).unwrap();
        assert!(supplemental_protected);
        assert_eq!(supplemental_selected, 0);

        let (_, confident_protected, confident_selected) =
            candidate_rescore_order(&candidates, &[false, true], &[0.0, 2.0], 0.8, 0.0).unwrap();
        assert!(!confident_protected);
        assert_eq!(confident_selected, 1);
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
        assert!(!request.is_long_input());
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
    fn surrounding_context_exposes_a_bounded_short_semantic_alternative() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context(
            "しかし、スキャンでは",
            "の右肺の腫瘍が成長していることがわかり、裁判をやめた。",
        );
        for character in "ぴゅーじょし".chars() {
            engine.handle(InputEvent::Character(character));
        }
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("surrounding context should expose the bounded ambiguity");
        assert_eq!(request.candidates[0], "ピュー女子");
        assert!(request.candidates.contains(&"ピュー女史".to_owned()));
        assert!(engine.candidate_rescore_requires_dictionary_only_ranking());
        assert!(!engine.candidate_rescore_supports_generative_recall());
    }

    #[test]
    fn confirmed_left_context_exposes_a_bounded_short_semantic_alternative() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context("キリスト教世界でも、たとえばアメリカでは大統領は", "");
        for character in "せんせいしき".chars() {
            engine.handle(InputEvent::Character(character));
        }
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("confirmed left context should expose the bounded ambiguity");
        assert_eq!(request.candidates[0], "先生式");
        assert!(request.candidates.contains(&"宣誓式".to_owned()));
        assert!(engine.candidate_rescore_requires_dictionary_only_ranking());
    }

    #[test]
    fn surrounding_context_does_not_widen_seven_character_confidence() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context("電車に戻ると、", "");
        for character in "なんぽーへたび".chars() {
            engine.handle(InputEvent::Character(character));
        }
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "南方へ旅");
        assert!(engine.candidate_rescore_request().is_none());
    }

    #[test]
    fn surrounding_percentage_series_protects_its_numeric_integer() {
        assert!(super::confirmed_parallel_percentage(
            "田老66・0%、",
            "・6%と地域差が大きかった。",
            "仙台39",
        ));
        assert!(!super::confirmed_parallel_percentage(
            "感謝の言葉を述べ、",
            "と答えた。",
            "仙台39",
        ));
        assert!(!super::confirmed_parallel_percentage(
            "田老66・0%",
            "・6%と地域差が大きかった。",
            "仙台39",
        ));

        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("せんだいさんきゅう", "仙台39", 1_000),
            DictionaryEntry::new("せんだいさんきゅう", "仙台サンキュー", 1_100),
        ]);
        let mut ordinary = SlimeEngine::new(dictionary.clone());
        type_text(&mut ordinary, "sendaisankyuu");
        ordinary.handle(InputEvent::Space);
        assert!(ordinary.candidate_rescore_request().is_some());

        let mut contextual = SlimeEngine::new(dictionary);
        contextual.set_external_context("田老66・0%、", "・6%と地域差が大きかった。");
        type_text(&mut contextual, "sendaisankyuu");
        contextual.handle(InputEvent::Space);
        assert_eq!(contextual.snapshot().preedit, "仙台39");
        assert!(contextual.candidate_rescore_request().is_none());
    }

    #[test]
    fn neural_rescoring_does_not_only_expand_ascii_digit_width() {
        assert!(super::rescore_only_expands_ascii_digit_width(
            "2014年",
            "２０１４年"
        ));
        assert!(super::rescore_only_expands_ascii_digit_width(
            "第1期",
            "第１期"
        ));
        assert!(!super::rescore_only_expands_ascii_digit_width(
            "2014年",
            "二〇一四年"
        ));
        assert!(!super::rescore_only_expands_ascii_digit_width(
            "2014年",
            "２０１５年"
        ));
        assert!(!super::rescore_only_expands_ascii_digit_width(
            "２０１４年",
            "2014年"
        ));

        let candidates = vec![
            Candidate {
                surface: "2014年".to_owned(),
                cost: 100,
            },
            Candidate {
                surface: "２０１４年".to_owned(),
                cost: 150,
            },
        ];
        let mut engine = SlimeEngine::new(Dictionary::new(Vec::new()));
        engine.reading = "にぜろいちよねん".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: engine.candidates.clone(),
            },
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
            candidates,
        });
        engine
            .apply_candidate_rescore(&[0.0, 10.0], 0.8, 0.0)
            .expect("digit-width-only rescore should preserve the base candidate");
        assert_eq!(engine.candidates[0], "2014年");
    }

    #[test]
    fn neural_rescoring_preserves_structured_ascii_numbers() {
        assert!(super::rescore_changes_calendar_or_clock_ascii_digits(
            "6月10日",
            "6月トーカ"
        ));
        assert!(super::rescore_changes_calendar_or_clock_ascii_digits(
            "2026年8月12日",
            "2026年8月十二日"
        ));
        assert!(!super::rescore_changes_calendar_or_clock_ascii_digits(
            "1990年の家事",
            "1990年の火事"
        ));
        assert!(!super::rescore_changes_calendar_or_clock_ascii_digits(
            "39編",
            "サンキュー編"
        ));
        assert!(!super::rescore_changes_calendar_or_clock_ascii_digits(
            "夜1007位",
            "予選7位"
        ));
        assert!(!super::rescore_changes_calendar_or_clock_ascii_digits(
            "グレード421",
            "グレード4に位置"
        ));

        let candidates = vec![
            Candidate {
                surface: "6月10日".to_owned(),
                cost: 100,
            },
            Candidate {
                surface: "6月トーカ".to_owned(),
                cost: 150,
            },
        ];
        let mut engine = SlimeEngine::new(Dictionary::new(Vec::new()));
        engine.reading = "ろくがつとおか".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: "の土曜日".to_owned(),
                reading: engine.reading.clone(),
                candidates: engine.candidates.clone(),
            },
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
            candidates,
        });
        engine
            .apply_candidate_rescore(&[0.0, 10.0], 0.8, 0.0)
            .expect("structured-number rescore should preserve the base candidate");
        assert_eq!(engine.candidates[0], "6月10日");
    }

    #[test]
    fn neural_rescoring_preserves_alphanumeric_compound_numbers() {
        let candidates = vec![
            Candidate {
                surface: "9幹線".to_owned(),
                cost: 100,
            },
            Candidate {
                surface: "キュー幹線".to_owned(),
                cost: 150,
            },
            Candidate {
                surface: "9感染".to_owned(),
                cost: 200,
            },
        ];
        let mut engine = SlimeEngine::new(Dictionary::new(Vec::new()));
        engine.reading = "きゅーかんせん".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        let state = CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: "デドフスクにはM".to_owned(),
                right_context: "道路が通る".to_owned(),
                reading: engine.reading.clone(),
                candidates: engine.candidates.clone(),
            },
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
            candidates,
        };
        assert!(super::rescore_removes_alphanumeric_compound_number(
            &state, 1
        ));
        assert!(!super::rescore_removes_alphanumeric_compound_number(
            &state, 2
        ));
        engine.candidate_rescore = Some(state);
        engine
            .apply_candidate_rescore(&[0.0, 10.0, -10.0], 0.8, 0.0)
            .expect("structured alphanumeric rescore should preserve the number");
        assert_eq!(engine.candidates[0], "9幹線");
    }

    #[test]
    fn model_rescore_dictionary_is_invisible_until_ready_and_can_supply_short_candidate() {
        let standard_entries = vec![
            DictionaryEntry::new("しんたく", "信託", 1_000),
            DictionaryEntry::new("しんたく", "新宅", 1_100),
        ];
        let mut model_entries = standard_entries.clone();
        model_entries.push(DictionaryEntry::new("しんたく", "神託", 1_050));
        let mut engine = SlimeEngine::new(Dictionary::new(standard_entries));
        engine.model_rescore_dictionary = Some(Dictionary::new(model_entries));

        type_text(&mut engine, "shintaku");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().candidates[0], "信託");
        assert!(!engine.snapshot().candidates.contains(&"神託".to_owned()));

        engine.prepare_extended_candidate_rescore_with_limit_and_confidence(32, 8, true);
        let request = engine
            .candidate_rescore_request()
            .expect("ready scorer should receive supplemental short vocabulary");
        assert!(request.candidates.contains(&"神託".to_owned()));
        let scores: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| if candidate == "神託" { 0.0 } else { -10.0 })
            .collect();
        engine
            .apply_candidate_rescore(&scores, 0.7, 0.1)
            .expect("aligned model scores should publish supplemental candidate");
        assert_eq!(engine.snapshot().candidates[0], "神託");
    }

    #[test]
    fn installed_model_rescore_pack_never_changes_unscored_candidates() {
        let directory = test_directory("model-rescore-pack");
        write_model_rescore_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        type_text(&mut engine, "tesutoyougo");
        engine.handle(InputEvent::Space);
        let unscored = engine.snapshot().candidates;
        assert!(!unscored.contains(&"補助試験語甲".to_owned()));
        assert!(!unscored.contains(&"補助試験語乙".to_owned()));

        engine.prepare_extended_candidate_rescore_with_limit_and_confidence(32, 8, true);
        let request = engine
            .candidate_rescore_request()
            .expect("ready scorer should activate installed supplemental pack");
        assert!(request.candidates.contains(&"補助試験語甲".to_owned()));
        assert!(request.candidates.contains(&"補助試験語乙".to_owned()));
        assert_eq!(engine.snapshot().candidates, unscored);
        assert_eq!(request.candidates[0], unscored[0]);
        let scores: Vec<_> = request
            .candidates
            .iter()
            .map(|candidate| {
                if candidate == "補助試験語乙" {
                    0.0
                } else {
                    -10.0
                }
            })
            .collect();
        engine
            .apply_candidate_rescore(&scores, 0.7, 0.1)
            .expect("successful scoring should publish supplemental pack candidate");
        assert_eq!(engine.snapshot().candidates[0], "補助試験語乙");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installed_explicit_search_pack_joins_only_after_candidate_tail() {
        let directory = test_directory("explicit-search-pack");
        write_explicit_search_pack(&directory);
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));

        assert!(
            !engine
                .ascii_surfaces
                .iter()
                .any(|(_, surface)| surface == "GitHub")
        );
        assert!(
            !engine
                .conversion_candidates("てすとようご")
                .contains(&"明示試験語甲".to_owned())
        );
        type_text(&mut engine, "tesutoyougo");
        engine.handle(InputEvent::Space);
        let initial = engine.snapshot();
        assert!(!initial.candidates.contains(&"明示試験語甲".to_owned()));
        assert!(!initial.candidates.contains(&"明示試験語乙".to_owned()));

        for _ in 0..initial.candidates.len() {
            engine.handle(InputEvent::NextCandidate);
        }
        let expanded = engine.snapshot();
        assert!(expanded.candidates.contains(&"明示試験語甲".to_owned()));
        assert!(expanded.candidates.contains(&"明示試験語乙".to_owned()));
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);

        fs::remove_dir_all(directory).unwrap();
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
        assert!(request.is_long_input());
        assert_eq!(
            request.candidates,
            (0..super::LONG_RESCORE_CANDIDATE_LIMIT)
                .map(|index| format!("長文候補{index}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn external_scoring_widens_the_cost_window_only_for_long_readings() {
        let entries_for = |reading: &str| {
            [
                DictionaryEntry::new(reading, "第一候補", 1_000),
                DictionaryEntry::new(reading, "第二候補", 1_100),
                DictionaryEntry::new(reading, "深い候補", 3_000),
            ]
        };

        let mut short = SlimeEngine::new(Dictionary::new(Vec::from(entries_for("しょうぶん"))));
        type_text(&mut short, "shoubun");
        short.handle(InputEvent::Space);
        assert_eq!(
            short
                .candidate_rescore_request()
                .expect("short ambiguous reading should be scoreable")
                .candidates,
            ["第一候補", "第二候補"]
        );

        let long_reading = "ちょうぶんしょうに";
        let mut long = SlimeEngine::new(Dictionary::new(Vec::from(entries_for(long_reading))));
        type_text(&mut long, "choubunshouni");
        long.handle(InputEvent::Space);
        assert_eq!(
            long.candidate_rescore_request()
                .expect("long ambiguous reading should be scoreable")
                .candidates,
            ["第一候補", "第二候補", "深い候補"]
        );
    }

    #[test]
    fn ready_external_scorer_can_prepare_a_deeper_long_reading_pool() {
        let reading = "ちょうぶんしょうに";
        let entries = (0_i32..40)
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
            16
        );
        engine.prepare_extended_candidate_rescore_with_limit(usize::MAX);
        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("ready scorer should receive the bounded maximum request")
                .candidates
                .len(),
            super::MAX_EXTENDED_LONG_RESCORE_CANDIDATES
        );
    }

    #[test]
    fn high_accuracy_scorer_bypasses_confidence_only_for_left_context_long_input() {
        let reading = "ちょうぶんしょうに";
        let dictionary = || {
            Dictionary::new(
                (0_i32..20)
                    .map(|index| {
                        DictionaryEntry::new(
                            reading,
                            format!("長文候補{index}"),
                            if index == 0 {
                                1_000
                            } else {
                                2_190 + index * 10
                            },
                        )
                    })
                    .collect(),
            )
        };
        let mut engine = SlimeEngine::new(dictionary());
        type_text(&mut engine, "choubunshouni");
        engine.handle(InputEvent::Space);

        assert!(engine.candidate_rescore_request().is_none());
        engine.prepare_extended_candidate_rescore_with_limit_and_confidence(32, 8, false);
        assert!(engine.candidate_rescore_request().is_none());
        engine.prepare_extended_candidate_rescore_with_limit_and_confidence(32, 8, true);
        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("high-accuracy long input should bypass base confidence")
                .candidates,
            (0..8)
                .map(|index| format!("長文候補{index}"))
                .collect::<Vec<_>>()
        );

        let mut with_right_context = SlimeEngine::new(dictionary());
        with_right_context.set_external_context("", "ました。");
        type_text(&mut with_right_context, "choubunshouni");
        with_right_context.handle(InputEvent::Space);
        with_right_context
            .prepare_extended_candidate_rescore_with_limit_and_confidence(32, 8, true);
        assert!(with_right_context.candidate_rescore_request().is_none());

        let directory = test_directory("long-rescore-protected-history");
        fs::write(
            directory.join("history.tsv"),
            format!("# slime-history-v1\n{reading}\t履歴候補\t5\t10\n"),
        )
        .unwrap();
        let mut with_history =
            SlimeEngine::with_user_data(dictionary(), UserData::load(&directory));
        with_history.set_preferences(EnginePreferences {
            history_completion: true,
            history_learning: true,
            ..EnginePreferences::default()
        });
        type_text(&mut with_history, "choubunshouni");
        with_history.handle(InputEvent::Space);
        assert_eq!(with_history.snapshot().candidates[0], "履歴候補");
        with_history.prepare_extended_candidate_rescore_with_limit_and_confidence(32, 8, true);
        assert!(with_history.candidate_rescore_request().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ready_external_scorer_keeps_right_context_in_the_deeper_pool() {
        let reading = "あしたゆっくりのめ";
        let dictionary = Dictionary::bundled_with_layers(vec![DictionaryLayer::new(
            "right-context-regression",
            "Right context regression",
            vec![
                DictionaryEntry::new(reading, "明日ゆっくりの目", -10_000),
                DictionaryEntry::new(reading, "明日ゆっくり飲め", -9_700),
            ],
        )]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.set_external_context("", "ました。");
        type_text(&mut engine, "ashitayukkurinome");
        engine.handle(InputEvent::Space);

        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("long ambiguous reading should be scoreable")
                .candidates[0],
            "明日ゆっくり飲め"
        );
        engine.prepare_extended_candidate_rescore();
        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("deeper request should retain right context")
                .candidates[0],
            "明日ゆっくり飲め"
        );
    }

    #[test]
    fn ready_external_scorer_adds_unknown_katakana_recall_without_moving_base() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "akemeneidogun");
        engine.handle(InputEvent::Space);
        let base = engine.snapshot().preedit;

        assert_ne!(base, "アケメネイド軍");
        engine.prepare_extended_candidate_rescore_with_limit(32);
        let request = engine
            .candidate_rescore_request()
            .expect("long reading should expose a model recall pool");
        assert_eq!(request.candidates[0], base);
        assert!(
            request.candidates.contains(&"アケメネイド軍".to_owned()),
            "model recall request: {request:?}"
        );
    }

    #[test]
    fn ready_external_scorer_extends_a_short_existing_katakana_prefix() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context(
            "ブリリアニアは、ノルウェー南部のクリスチャンサン付近から",
            "たエリアまで続く、岩礁で保護された水路です。",
        );
        type_text(&mut engine, "riresanwokoe");
        engine.handle(InputEvent::Space);
        let base = engine.snapshot().preedit;

        assert_eq!(base, "リレさんを超え");
        engine.prepare_extended_candidate_rescore_with_limit(32);
        let request = engine
            .candidate_rescore_request()
            .expect("existing katakana prefix should expose model recall");
        assert_eq!(request.candidates[0], base);
        assert!(
            request.candidates.contains(&"リレサンを越え".to_owned()),
            "model recall request: {request:?}"
        );
    }

    #[test]
    fn short_katakana_recall_does_not_deepen_an_all_kanji_base() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context("", "弟にあたる。");
        type_text(&mut engine, "りゅーゆーのいぼ");
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "劉裕の異母");
        engine.prepare_extended_candidate_rescore_with_limit(32);
        assert!(engine.candidate_rescore_request().is_none_or(|request| {
            !request.candidates.contains(&"リューユーの異母".to_owned())
        }));
    }

    #[test]
    fn short_japanese_phrase_does_not_rebuild_model_pool_for_katakana_recall() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context(
            "組織のパフォーマンスはどれだけ安全か、または規則に従うかという",
            "れることは滅多にない。",
        );
        type_text(&mut engine, "menkarahakara");
        engine.handle(InputEvent::Space);
        let before = engine
            .candidate_rescore_request()
            .expect("ambiguous phrase should expose the ordinary model pool");

        engine.prepare_extended_candidate_rescore_with_limit(32);
        assert_eq!(engine.candidate_rescore_request(), Some(before));
    }

    #[test]
    fn katakana_model_recall_requires_mixed_script_or_a_short_prefix_extension() {
        assert!(super::is_mixed_katakana_recall_surface("アケメネイド軍"));
        assert!(!super::is_mixed_katakana_recall_surface("メンカラ測ら"));
        assert!(!super::is_mixed_katakana_recall_surface("サンシャの過"));
        assert!(!super::is_mixed_katakana_recall_surface("アケメネイド"));
        assert!(super::is_model_katakana_recall_surface(
            "リレサンを越え",
            "リレさんを越え"
        ));
        assert!(!super::is_model_katakana_recall_surface(
            "メンカラ測ら",
            "面から測ら"
        ));
        assert!(!super::is_model_katakana_recall_surface(
            "サンシャの過",
            "三者の過"
        ));
        assert!(!super::has_short_initial_katakana_run("テストー語"));
    }

    fn accepts_foreign_prefix(
        dictionary: &Dictionary,
        reading: &str,
        base_surface: &str,
        generated_surface: &str,
        conversion: &slime_converter::Conversion,
        cost_gap: i32,
    ) -> bool {
        super::ModelVerifiedCandidate {
            dictionary,
            reading,
            base_surface,
            generated_surface,
            conversion,
            cost_gap,
            structurally_bounded: false,
            quoted_span: false,
        }
        .accepts_foreign_prefix()
    }

    #[test]
    fn generative_recall_accepts_only_bounded_foreign_prefix_paths() {
        let dictionary = Dictionary::bundled();
        let accepted = [
            ("みりかんがしん", "ミリ感が死ん", "ミリカンが死ん"),
            ("とぅるちゃけん", "トゥル茶権", "トゥルチャ県"),
            (
                "めるでぃんげんにうつり",
                "目ルディン源に移り",
                "メルディンゲンに移り",
            ),
        ];
        for (reading, base, generated) in accepted {
            let base = dictionary
                .convert_n_best_with_surface_prefix(reading, base, 32)
                .into_iter()
                .find(|conversion| conversion.surface == base)
                .expect("base surface must be a complete lattice path");
            let generated = dictionary
                .convert_n_best_with_surface_prefix(reading, generated, 32)
                .into_iter()
                .find(|conversion| conversion.surface == generated)
                .expect("generated surface must be a complete lattice path");
            let cost_gap = generated.cost.saturating_sub(base.cost).max(0);
            assert!(
                accepts_foreign_prefix(
                    &dictionary,
                    reading,
                    &base.surface,
                    &generated.surface,
                    &generated,
                    cost_gap,
                ),
                "generated={generated:?}, base={base:?}, gap={cost_gap}"
            );
        }

        let reading = "りれさんをこえ";
        let base = dictionary
            .convert_n_best_with_surface_prefix(reading, "リレさんを越え", 32)
            .into_iter()
            .find(|conversion| conversion.surface == "リレさんを越え")
            .unwrap();
        let short_prefix = dictionary
            .convert_n_best_with_surface_prefix(reading, "リレ山を越え", 32)
            .into_iter()
            .find(|conversion| conversion.surface == "リレ山を越え")
            .unwrap();
        assert!(!accepts_foreign_prefix(
            &dictionary,
            reading,
            &base.surface,
            &short_prefix.surface,
            &short_prefix,
            short_prefix.cost.saturating_sub(base.cost).max(0),
        ));

        for (reading, base_surface, generated_surface) in [
            (
                "ぷろてくとよーのみ",
                "プロテクト用のみ",
                "プロテクトヨーのみ",
            ),
            ("りゅーゆーのいぼ", "劉裕の異母", "リューユーの異母"),
        ] {
            let base = dictionary
                .convert_n_best_with_surface_prefix(reading, base_surface, 32)
                .into_iter()
                .find(|conversion| conversion.surface == base_surface)
                .unwrap();
            let generated = dictionary
                .convert_n_best_with_surface_prefix(reading, generated_surface, 32)
                .into_iter()
                .find(|conversion| conversion.surface == generated_surface)
                .unwrap();
            assert!(!accepts_foreign_prefix(
                &dictionary,
                reading,
                &base.surface,
                &generated.surface,
                &generated,
                generated.cost.saturating_sub(base.cost).max(0),
            ));
        }
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
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
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
    fn model_prefix_can_insert_a_bounded_local_correction() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "少額", 20),
            DictionaryEntry::new("もんだい", "問題", 10),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "しょうがくのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["奨学の問題".to_owned()];
        let candidate = Candidate {
            surface: "奨学の問題".to_owned(),
            cost: 100,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![candidate.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![candidate],
        });

        engine
            .apply_candidate_rescore_with_prefix_constraints(
                &[0.0],
                &[Some("少".to_owned())],
                0.8,
                0.0,
            )
            .expect("aligned prefix correction should apply");

        assert_eq!(engine.candidates[0], "少額の問題");
        assert!(engine.candidates.contains(&"奨学の問題".to_owned()));
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&SlimeAction::Commit("少額の問題".to_owned()))
        );
    }

    #[test]
    fn model_prefix_preserves_an_exact_personal_name_segment() {
        const GIVEN_NAME_POS_ID: u16 = 1922;
        const SURNAME_POS_ID: u16 = 1923;
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos(
                "かたせしま",
                "片瀬志麻",
                SURNAME_POS_ID,
                GIVEN_NAME_POS_ID,
                10,
            ),
            DictionaryEntry::with_pos("かたせ", "片瀬", SURNAME_POS_ID, SURNAME_POS_ID, 20),
            DictionaryEntry::with_pos("しま", "志摩", GIVEN_NAME_POS_ID, GIVEN_NAME_POS_ID, 20),
            DictionaryEntry::new("たち", "たち", 10),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "かたせしまたち".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["片瀬志麻たち".to_owned()];
        let candidate = Candidate {
            surface: "片瀬志麻たち".to_owned(),
            cost: 100,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![candidate.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![candidate],
        });

        engine
            .apply_candidate_rescore_with_prefix_constraints(
                &[0.0],
                &[Some("片瀬志摩".to_owned())],
                0.8,
                0.0,
            )
            .expect("valid scores should still apply");

        assert_eq!(engine.candidates[0], "片瀬志麻たち");
        assert!(!engine.candidates.contains(&"片瀬志摩たち".to_owned()));
    }

    #[test]
    fn model_rescore_preserves_an_uncontextualized_personal_name() {
        const GIVEN_NAME_POS_ID: u16 = 1922;
        const SURNAME_POS_ID: u16 = 1923;
        let reading = "かたせしま";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos(
                "かたせしま",
                "片瀬志麻",
                SURNAME_POS_ID,
                GIVEN_NAME_POS_ID,
                10,
            ),
            DictionaryEntry::with_pos("かたせ", "片瀬", SURNAME_POS_ID, SURNAME_POS_ID, 20),
            DictionaryEntry::with_pos("しま", "志摩", GIVEN_NAME_POS_ID, GIVEN_NAME_POS_ID, 20),
        ]);
        let candidates = vec![
            Candidate {
                surface: "片瀬志麻".to_owned(),
                cost: 100,
            },
            Candidate {
                surface: "片瀬志摩".to_owned(),
                cost: 200,
            },
        ];
        let make_engine = |context: &str| {
            let mut engine = SlimeEngine::new(dictionary.clone());
            engine.reading = reading.to_owned();
            engine.candidate_kind = Some(CandidateKind::Conversion);
            engine.candidates = candidates
                .iter()
                .map(|candidate| candidate.surface.clone())
                .collect();
            engine.candidate_rescore = Some(CandidateRescoreState {
                request: CandidateRescoreRequest {
                    context: context.to_owned(),
                    right_context: "を訪ねた".to_owned(),
                    reading: reading.to_owned(),
                    candidates: engine.candidates.clone(),
                },
                model_supplemental: vec![false; candidates.len()],
                generative_consensus: None,
                candidates: candidates.clone(),
            });
            engine
        };

        let mut without_left_context = make_engine("");
        without_left_context
            .apply_candidate_rescore(&[0.0, 10.0], 0.8, 0.0)
            .expect("aligned scores should preserve the uncontextualized name");
        assert_eq!(without_left_context.candidates[0], "片瀬志麻");

        let mut with_left_context = make_engine("同級生の");
        with_left_context
            .apply_candidate_rescore(&[0.0, 10.0], 0.8, 0.0)
            .expect("confirmed left context should allow contextual name ranking");
        assert_eq!(with_left_context.candidates[0], "片瀬志摩");
    }

    #[test]
    fn model_rescore_preserves_a_specific_exact_region_segment() {
        const REGION_POS_ID: u16 = 1924;
        let reading = "くるみだてちゅうざいしょ";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("くるみだて", "胡桃舘", REGION_POS_ID, REGION_POS_ID, 10),
            DictionaryEntry::new("ちゅうざいしょ", "駐在所", 10),
            DictionaryEntry::new(reading, "くるみだて駐在所", 100),
        ]);
        let candidates = vec![
            Candidate {
                surface: "胡桃舘駐在所".to_owned(),
                cost: 100,
            },
            Candidate {
                surface: "くるみだて駐在所".to_owned(),
                cost: 200,
            },
        ];
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = reading.to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: reading.to_owned(),
                candidates: engine.candidates.clone(),
            },
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
            candidates,
        });

        engine
            .apply_candidate_rescore(&[0.0, 10.0], 0.8, 0.0)
            .expect("aligned scores should preserve the exact region");

        assert_eq!(engine.candidates[0], "胡桃舘駐在所");
    }

    #[test]
    fn model_rescore_rejects_fragmented_exact_katakana() {
        let reading = "あるごるたいようけい";
        let candidates = vec![
            Candidate {
                surface: "アルゴル太陽系".to_owned(),
                cost: 100,
            },
            Candidate {
                surface: "あるゴル太陽系".to_owned(),
                cost: 200,
            },
        ];
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あるごる", "アルゴル", 10),
            DictionaryEntry::new("たいようけい", "太陽系", 10),
            DictionaryEntry::new(reading, "あるゴル太陽系", 100),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = reading.to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: reading.to_owned(),
                candidates: engine.candidates.clone(),
            },
            model_supplemental: vec![false; candidates.len()],
            generative_consensus: None,
            candidates,
        });

        engine
            .apply_candidate_rescore(&[0.0, 10.0], 0.8, 0.0)
            .expect("mixed-script fragment should not replace exact katakana");

        assert_eq!(engine.candidates[0], "アルゴル太陽系");
    }

    #[test]
    fn model_prefix_can_review_one_safe_correction_once() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "少額", 20),
            DictionaryEntry::new("もんだい", "問題", 10),
            DictionaryEntry::new("もんだい", "課題", 20),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "しょうがくのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["奨学の問題".to_owned()];
        let candidate = Candidate {
            surface: "奨学の問題".to_owned(),
            cost: 100,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: "前の文".to_owned(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![candidate.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![candidate],
        });

        let constraints = [Some("少".to_owned())];
        let followup = engine
            .candidate_rescore_prefix_followup_request(&[0.0], &constraints, 0.8, 0.0)
            .expect("the first safe correction should be reviewable");
        assert_eq!(followup.context, "前の文");
        assert_eq!(followup.candidates, ["少額の問題"]);
        assert_eq!(engine.candidates, ["奨学の問題"]);

        engine
            .apply_candidate_rescore_with_prefix_constraints_and_followup(
                &[0.0],
                &constraints,
                Some("少額の課"),
                0.8,
                0.0,
            )
            .expect("both independently bounded corrections should apply");

        assert_eq!(engine.candidates[0], "少額の課題");
        assert!(engine.candidates.contains(&"少額の問題".to_owned()));
        assert!(engine.candidates.contains(&"奨学の問題".to_owned()));
    }

    #[test]
    fn model_prefix_cannot_rewrite_a_second_distant_region() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "少額", 20),
            DictionaryEntry::new("もんだい", "課題", 10),
        ]);
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "しょうがくのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["奨学の問題".to_owned()];
        let candidate = Candidate {
            surface: "奨学の問題".to_owned(),
            cost: 100,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![candidate.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![candidate],
        });

        engine
            .apply_candidate_rescore_with_prefix_constraints(
                &[0.0],
                &[Some("少".to_owned())],
                0.8,
                0.0,
            )
            .expect("valid scores should still apply");

        assert_eq!(engine.candidates[0], "奨学の問題");
        assert!(!engine.candidates.contains(&"少額の課題".to_owned()));
    }

    #[test]
    fn model_prefix_skips_unsafe_paths_before_a_safe_local_correction() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "少額", 20),
            DictionaryEntry::new("もんだい", "課題", 1),
            DictionaryEntry::new("もんだい", "命題", 2),
            DictionaryEntry::new("もんだい", "設問", 3),
            DictionaryEntry::new("もんだい", "題目", 4),
            DictionaryEntry::new("もんだい", "疑問", 5),
            DictionaryEntry::new("もんだい", "論題", 6),
            DictionaryEntry::new("もんだい", "難題", 7),
            DictionaryEntry::new("もんだい", "問答", 8),
            DictionaryEntry::new("もんだい", "問題", 20),
        ]);
        let first_eight =
            dictionary.convert_n_best_with_surface_prefix("しょうがくのもんだい", "少", 8);
        assert_eq!(first_eight.len(), 8);
        assert!(
            first_eight
                .iter()
                .all(|conversion| conversion.surface != "少額の問題")
        );

        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "しょうがくのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["奨学の問題".to_owned()];
        let candidate = Candidate {
            surface: "奨学の問題".to_owned(),
            cost: 100,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![candidate.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![candidate],
        });

        engine
            .apply_candidate_rescore_with_prefix_constraints(
                &[0.0],
                &[Some("少".to_owned())],
                0.8,
                0.0,
            )
            .expect("a safe deeper prefix correction should apply");

        assert_eq!(engine.candidates[0], "少額の問題");
        assert!(engine.candidates.contains(&"奨学の問題".to_owned()));
        assert!(!engine.candidates.contains(&"少額の課題".to_owned()));
    }

    #[test]
    fn generated_surface_can_join_rescore_only_after_bounded_lattice_validation() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "奨学", 10),
            DictionaryEntry::new("しょうがく", "少額", 20),
            DictionaryEntry::new("もんだい", "問題", 10),
            DictionaryEntry::new("もんだい", "課題", 20),
        ]);
        let base_conversion = dictionary
            .convert_n_best_with_surface_prefix("しょうがくのもんだい", "奨学の問題", 1)
            .into_iter()
            .next()
            .expect("base lattice path");
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "しょうがくのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["奨学の問題".to_owned()];
        let base = Candidate {
            surface: "奨学の問題".to_owned(),
            cost: base_conversion.cost,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![base.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![base],
        });

        let request = engine
            .prepare_generative_rescore_candidate("少額の課題")
            .expect("two bounded regions backed by the lattice should join rescoring");
        assert_eq!(request.candidates, ["奨学の問題", "少額の課題"]);
        assert_eq!(
            engine
                .candidate_rescore
                .as_ref()
                .expect("pending rescore state")
                .model_supplemental,
            [false, true]
        );
        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("safe whole-result agreement should override ordinary scoring");
        assert_eq!(engine.candidates[0], "少額の課題");
    }

    #[test]
    fn generated_surface_compression_can_join_after_bounded_lattice_validation() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "あい", 10),
            DictionaryEntry::new("あい", "愛", 20),
            DictionaryEntry::new("もんだい", "問題", 10),
            DictionaryEntry::new("もんだい", "課題", 20),
        ]);
        let base_conversion = dictionary
            .convert_n_best_with_surface_prefix("あいのもんだい", "あいの問題", 1)
            .into_iter()
            .next()
            .expect("base lattice path");
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "あいのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["あいの問題".to_owned()];
        let base = Candidate {
            surface: "あいの問題".to_owned(),
            cost: base_conversion.cost,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: engine.reading.clone(),
                candidates: vec![base.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![base],
        });

        let request = engine
            .prepare_generative_rescore_candidate("愛の課題")
            .expect("a bounded dictionary-backed surface compression should join rescoring");
        assert_eq!(request.candidates, ["あいの問題", "愛の課題"]);
        let state = engine
            .candidate_rescore
            .as_ref()
            .expect("pending rescore state");
        assert_eq!(state.model_supplemental, [false, true]);
        assert_eq!(
            state.generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::Whole,
                accepts_whole_result: true,
            })
        );
    }

    #[test]
    fn generated_multi_region_surface_can_use_extended_cost_consensus() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "奨学", 10),
            DictionaryEntry::new("しょうがく", "少額", 1_560),
            DictionaryEntry::new("しょうがく", "商学", 1_561),
            DictionaryEntry::new("もんだい", "問題", 10),
            DictionaryEntry::new("もんだい", "課題", 1_560),
            DictionaryEntry::new("もんだい", "門題", 1_561),
        ]);
        let reading = "しょうがくのもんだい";
        let base = dictionary
            .convert_n_best_with_surface_prefix(reading, "奨学の問題", 1)
            .into_iter()
            .next()
            .expect("base lattice path");
        let generated = dictionary
            .convert_n_best_with_surface_prefix(reading, "少額の課題", 1)
            .into_iter()
            .next()
            .expect("generated lattice path");
        let beyond_limit = dictionary
            .convert_n_best_with_surface_prefix(reading, "商学の門題", 1)
            .into_iter()
            .next()
            .expect("beyond-limit lattice path");
        let cost_gap = generated.cost.saturating_sub(base.cost);
        assert!(cost_gap > super::LONG_RESCORE_MAX_CANDIDATE_COST_GAP);
        assert!(cost_gap <= super::GENERATIVE_EXTENDED_MULTI_REGION_COST_GAP);
        assert!(
            beyond_limit.cost.saturating_sub(base.cost)
                > super::GENERATIVE_EXTENDED_MULTI_REGION_COST_GAP
        );

        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = reading.to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec![base.surface.clone()];
        let base_candidate = Candidate {
            surface: base.surface.clone(),
            cost: base.cost,
        };
        engine.candidate_rescore = Some(CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: reading.to_owned(),
                candidates: vec![base.surface.clone()],
            },
            model_supplemental: vec![false],
            generative_consensus: None,
            candidates: vec![base_candidate],
        });

        assert!(
            engine
                .prepare_generative_rescore_candidate(&beyond_limit.surface)
                .is_none()
        );
        let request = engine
            .prepare_generative_rescore_candidate(&generated.surface)
            .expect("bounded multi-region generation should use the extended window");
        assert_eq!(request.candidates, ["奨学の問題", "少額の課題"]);
        let state = engine
            .candidate_rescore
            .as_ref()
            .expect("pending rescore state");
        assert_eq!(state.model_supplemental, [false, true]);
        assert_eq!(
            state.generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::ExtendedMultiRegion,
                accepts_whole_result: false,
            })
        );

        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("aligned scores should apply direct generation consensus");
        assert_eq!(engine.candidates[0], "少額の課題");
        assert!(engine.candidates.contains(&"奨学の問題".to_owned()));
    }

    #[test]
    fn existing_generated_surface_records_generation_consensus_without_duplication() {
        let reading = "しょうがくせい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "奨学生", 100),
            DictionaryEntry::new(reading, "小学生", 1_100),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "奨学生"),
            exact_candidate(&dictionary, reading, "小学生"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);

        let request = engine
            .prepare_generative_rescore_candidate("小学生")
            .expect("an existing generated surface should be recorded");
        assert_eq!(request.candidates, ["奨学生", "小学生"]);
        let state = engine
            .candidate_rescore
            .as_ref()
            .expect("pending rescore state");
        assert_eq!(
            state.generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::Local,
                accepts_whole_result: true,
            })
        );
        assert_eq!(state.model_supplemental, [false, false]);
    }

    #[test]
    fn whole_result_consensus_revalidates_an_existing_supplemental_path() {
        let reading = "しょうがくせい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "奨学生", 100),
            DictionaryEntry::new(reading, "小学生", 200),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "奨学生"),
            exact_candidate(&dictionary, reading, "小学生"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);
        engine
            .candidate_rescore
            .as_mut()
            .expect("pending rescore state")
            .model_supplemental[1] = true;

        engine
            .prepare_generative_rescore_candidate("小学生")
            .expect("a supplemental candidate must pass exact lattice validation");
        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("safe whole-result agreement should apply");
        assert_eq!(engine.candidates[0], "小学生");
    }

    #[test]
    fn existing_multi_region_surface_records_distinct_generation_consensus() {
        let reading = "しょうがくのもんだい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "奨学の問題", 100),
            DictionaryEntry::new(reading, "少額の課題", 1_100),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "奨学の問題"),
            exact_candidate(&dictionary, reading, "少額の課題"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);

        let request = engine
            .prepare_generative_rescore_candidate("少額の課題")
            .expect("an existing bounded multi-region surface should be recorded");
        assert_eq!(request.candidates, ["奨学の問題", "少額の課題"]);
        let state = engine
            .candidate_rescore
            .as_ref()
            .expect("pending rescore state");
        assert_eq!(
            state.generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::MultiRegion,
                accepts_whole_result: true,
            })
        );
        assert_eq!(state.model_supplemental, [false, false]);
    }

    #[test]
    fn whole_result_consensus_accepts_the_strict_base_cost_boundary() {
        let reading = "かんぜんなかいとう";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "第一候補", 100),
            DictionaryEntry::new(reading, "完全正解", 1_100),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "第一候補"),
            exact_candidate(&dictionary, reading, "完全正解"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);

        let request = engine
            .prepare_generative_rescore_candidate("完全正解")
            .expect("a complete path at the strict cost boundary should be recorded");
        assert_eq!(request.candidates, ["第一候補", "完全正解"]);
        assert_eq!(
            engine
                .candidate_rescore
                .as_ref()
                .expect("pending rescore state")
                .generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::Whole,
                accepts_whole_result: true,
            })
        );

        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("whole-result consensus should apply");
        assert_eq!(engine.candidates[0], "完全正解");
    }

    #[test]
    fn whole_result_consensus_accepts_long_reading_at_the_evidence_floor() {
        let reading = "あ".repeat(33);
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(&reading, "第一候補", 100),
            DictionaryEntry::new(&reading, "完全正解", 600),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, &reading, "第一候補"),
            exact_candidate(&dictionary, &reading, "完全正解"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, &reading, candidates);

        assert!(!engine.candidate_rescore_supports_generative_recall());
        assert!(!engine.candidate_rescore_supports_delayed_long_generation(&[1.0, 0.0]));
        assert!(engine.candidate_rescore_supports_delayed_long_generation(&[0.0, 1.0]));
        engine
            .prepare_generative_rescore_candidate("完全正解")
            .expect("a long complete path at the evidence floor should be recorded");
        assert_eq!(
            engine
                .candidate_rescore
                .as_ref()
                .expect("pending rescore state")
                .generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::Whole,
                accepts_whole_result: true,
            })
        );

        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("safe long whole-result consensus should apply");
        assert_eq!(engine.candidates[0], "完全正解");
    }

    #[test]
    fn whole_result_consensus_rejects_weak_or_overlong_long_readings() {
        for (length, alternative_cost) in [(33, 599), (33, 1_101), (41, 600)] {
            let reading = "あ".repeat(length);
            let dictionary = Dictionary::new(vec![
                DictionaryEntry::new(&reading, "第一候補", 100),
                DictionaryEntry::new(&reading, "完全正解", alternative_cost),
            ]);
            let candidates = vec![
                exact_candidate(&dictionary, &reading, "第一候補"),
                exact_candidate(&dictionary, &reading, "完全正解"),
            ];
            let mut engine = engine_with_rescore_candidates(dictionary, &reading, candidates);

            assert!(!engine.candidate_rescore_supports_generative_recall());
            assert!(!engine.candidate_rescore_supports_delayed_long_generation(&[0.0, 1.0]));
            assert_eq!(
                engine.prepare_generative_rescore_candidate("完全正解"),
                None
            );
        }
    }

    #[test]
    fn long_whole_result_pre_gate_requires_existing_cost_evidence() {
        let reading = "あ".repeat(33);
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(&reading, "第一候補", 100),
            DictionaryEntry::new(&reading, "完全正解", 600),
        ]);
        let candidates = vec![exact_candidate(&dictionary, &reading, "第一候補")];
        let mut engine = engine_with_rescore_candidates(dictionary, &reading, candidates);

        assert!(!engine.candidate_rescore_supports_generative_recall());
        assert!(!engine.candidate_rescore_supports_delayed_long_generation(&[1.0]));
        assert_eq!(
            engine.prepare_generative_rescore_candidate("完全正解"),
            None
        );
    }

    #[test]
    fn whole_result_consensus_rejects_costs_beyond_the_strict_boundary() {
        let reading = "かんぜんなかいとう";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "第一候補", 100),
            DictionaryEntry::new(reading, "完全正解", 1_101),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "第一候補"),
            exact_candidate(&dictionary, reading, "完全正解"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);

        assert_eq!(
            engine.prepare_generative_rescore_candidate("完全正解"),
            None
        );
    }

    #[test]
    fn whole_result_consensus_preserves_ascii_kanji_and_personal_names() {
        let cases = [
            (
                "きげんぜんごひゃくじゅういち",
                vec![
                    DictionaryEntry::new("きげんぜんごひゃくじゅういち", "紀元前511年", 10),
                    DictionaryEntry::new("きげんぜんごひゃくじゅういち", "紀元前後11年", 20),
                ],
                "紀元前511年",
                "紀元前後11年",
            ),
            (
                "ほうほうがとら",
                vec![
                    DictionaryEntry::new("ほうほうがとら", "方法が取ら", 10),
                    DictionaryEntry::new("ほうほうがとら", "方法がとら", 20),
                ],
                "方法が取ら",
                "方法がとら",
            ),
        ];
        for (reading, entries, current, generated) in cases {
            let dictionary = Dictionary::new(entries);
            let candidates = vec![
                exact_candidate(&dictionary, reading, current),
                exact_candidate(&dictionary, reading, generated),
            ];
            let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);
            engine
                .prepare_generative_rescore_candidate(generated)
                .expect("the complete path should reach final safety validation");
            engine
                .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
                .expect("aligned scores should preserve the current candidate");
            assert_eq!(engine.candidates[0], current);
        }

        let reading = "かたせしまかてい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("かたせしま", "片瀬志麻", 1_923, 1_922, 10),
            DictionaryEntry::with_pos("かたせ", "片瀬", 1_923, 1_923, 20),
            DictionaryEntry::with_pos("しま", "志摩", 1_922, 1_922, 20),
            DictionaryEntry::new("かてい", "課程", 10),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "片瀬志麻課程"),
            exact_candidate(&dictionary, reading, "片瀬志摩課程"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);
        engine
            .prepare_generative_rescore_candidate("片瀬志摩課程")
            .expect("the complete path should reach final name validation");
        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("aligned scores should preserve the exact personal name");
        assert_eq!(engine.candidates[0], "片瀬志麻課程");

        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("かたせしま", "片瀬志麻", 1_923, 1_922, 10),
            DictionaryEntry::new("かてい", "課程", 10),
            DictionaryEntry::new(reading, "片瀬志課程", 30),
        ]);
        let candidates = vec![
            exact_candidate(&dictionary, reading, "片瀬志麻課程"),
            exact_candidate(&dictionary, reading, "片瀬志課程"),
        ];
        let mut engine = engine_with_rescore_candidates(dictionary, reading, candidates);
        engine
            .prepare_generative_rescore_candidate("片瀬志課程")
            .expect("the complete path should reach unequal-length name validation");
        engine
            .apply_candidate_rescore(&[0.0, -100.0], 0.8, 0.0)
            .expect("aligned scores should preserve the complete personal name");
        assert_eq!(engine.candidates[0], "片瀬志麻課程");
    }

    #[test]
    fn generation_consensus_only_overrides_for_a_narrow_model_near_tie() {
        let state = CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: "しょうがくせい".to_owned(),
                candidates: vec!["奨学生".to_owned(), "小学生".to_owned()],
            },
            candidates: vec![
                Candidate {
                    surface: "奨学生".to_owned(),
                    cost: 100,
                },
                Candidate {
                    surface: "小学生".to_owned(),
                    cost: 1_100,
                },
            ],
            model_supplemental: vec![false, false],
            generative_consensus: Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::Local,
                accepts_whole_result: false,
            }),
        };

        for (advantage, expected) in [(0.09, 0), (0.1, 1), (0.2, 1), (0.21, 0)] {
            let (order, protected, selected) =
                candidate_rescore_order_for_state(&state, &[0.0, advantage], 0.8, 0.0)
                    .expect("aligned finite scores");
            assert_eq!(selected, expected, "advantage={advantage}");
            assert_eq!(order[0], expected, "advantage={advantage}");
            assert!(!protected, "combined cost already keeps the base winner");
        }
    }

    #[test]
    fn multi_region_generation_consensus_uses_its_evaluated_near_tie_window() {
        let state = CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: "しょうがくのもんだい".to_owned(),
                candidates: vec!["奨学の問題".to_owned(), "少額の課題".to_owned()],
            },
            candidates: vec![
                Candidate {
                    surface: "奨学の問題".to_owned(),
                    cost: 100,
                },
                Candidate {
                    surface: "少額の課題".to_owned(),
                    cost: 1_100,
                },
            ],
            model_supplemental: vec![false, false],
            generative_consensus: Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::MultiRegion,
                accepts_whole_result: false,
            }),
        };

        for (advantage, expected) in [(0.09, 0), (0.1, 1), (0.25, 1), (0.26, 0)] {
            let (order, protected, selected) =
                candidate_rescore_order_for_state(&state, &[0.0, advantage], 0.8, 0.0)
                    .expect("aligned finite scores");
            assert_eq!(selected, expected, "advantage={advantage}");
            assert_eq!(order[0], expected, "advantage={advantage}");
            assert!(!protected, "combined cost already keeps the base winner");
        }
    }

    #[test]
    fn generated_surface_requires_full_lattice_and_bounds_unstructured_cost() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しょうがく", "奨学", 10),
            DictionaryEntry::new("しょうがく", "少額", 20),
            DictionaryEntry::new("もんだい", "問題", 10),
            DictionaryEntry::new("もんだい", "課題", 20),
            DictionaryEntry::new("しょうがくのもんだい", "全く別解", 20_000),
            DictionaryEntry::new("abc", "abd", 20),
        ]);
        let base_conversion = dictionary
            .convert_n_best_with_surface_prefix("しょうがくのもんだい", "奨学の問題", 1)
            .into_iter()
            .next()
            .expect("base lattice path");
        let remote_conversion = dictionary
            .convert_n_best_with_surface_prefix("しょうがくのもんだい", "全く別解", 1)
            .into_iter()
            .next()
            .expect("remote lattice path");
        assert!(
            remote_conversion.cost.saturating_sub(base_conversion.cost)
                > super::RESCORE_MAX_BASE_COST_GAP
        );
        let state = || {
            let base = Candidate {
                surface: "奨学の問題".to_owned(),
                cost: base_conversion.cost,
            };
            CandidateRescoreState {
                request: CandidateRescoreRequest {
                    context: String::new(),
                    right_context: String::new(),
                    reading: "しょうがくのもんだい".to_owned(),
                    candidates: vec![base.surface.clone()],
                },
                model_supplemental: vec![false],
                generative_consensus: None,
                candidates: vec![base],
            }
        };
        let mut engine = SlimeEngine::new(dictionary);
        engine.reading = "しょうがくのもんだい".to_owned();
        engine.candidate_kind = Some(CandidateKind::Conversion);
        engine.candidates = vec!["奨学の問題".to_owned()];

        engine.candidate_rescore = Some(state());
        let request = engine
            .prepare_generative_rescore_candidate("少額の問題")
            .expect("a confident complete lattice path should join rescoring");
        assert_eq!(request.candidates, ["奨学の問題", "少額の問題"]);
        let accepted = engine
            .candidate_rescore
            .as_ref()
            .expect("pending rescore state");
        assert_eq!(accepted.model_supplemental, [false, true]);
        assert_eq!(
            accepted.generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::Whole,
                accepts_whole_result: true,
            })
        );

        for rejected in ["少額の架空", "全く別解"] {
            engine.candidate_rescore = Some(state());
            assert_eq!(
                engine.prepare_generative_rescore_candidate(rejected),
                None,
                "{rejected} must remain outside the rescore pool"
            );
        }
        assert!(!super::bounded_multi_region_substitution(
            "abcの問題",
            "abdの課題"
        ));
    }

    #[test]
    fn model_verified_whole_result_requires_a_dominant_supplemental_score() {
        let reading = "しょうがくせい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "第一候補", 100),
            DictionaryEntry::new(reading, "完全正解", 1_500),
        ]);
        let base = exact_candidate(&dictionary, reading, "第一候補");
        let mut engine = engine_with_rescore_candidates(dictionary, reading, vec![base]);

        let request = engine
            .prepare_generative_rescore_candidate("完全正解")
            .expect("same-length whole-result evidence should join the scored pool");
        assert_eq!(request.candidates, ["第一候補", "完全正解"]);
        let state = engine
            .candidate_rescore
            .as_ref()
            .expect("pending rescore state");
        assert_eq!(state.model_supplemental, [false, true]);
        assert_eq!(
            state.generative_consensus,
            Some(GenerativeConsensus {
                candidate: 1,
                kind: GenerativeConsensusKind::ModelVerifiedWhole,
                accepts_whole_result: false,
            })
        );

        for (advantage, expected, protected) in [(1.79, 0, false), (1.8, 1, false), (2.0, 1, false)]
        {
            let (order, was_protected, selected) =
                candidate_rescore_order_for_state(state, &[0.0, advantage], 0.8, 0.0)
                    .expect("aligned finite scores");
            assert_eq!(selected, expected, "advantage={advantage}");
            assert_eq!(was_protected, protected, "advantage={advantage}");
            if !protected {
                assert_eq!(order[0], expected, "advantage={advantage}");
            }
        }

        let competing_state = CandidateRescoreState {
            request: CandidateRescoreRequest {
                context: String::new(),
                right_context: String::new(),
                reading: reading.to_owned(),
                candidates: vec![
                    "第一候補".to_owned(),
                    "通常候補".to_owned(),
                    "完全正解".to_owned(),
                ],
            },
            candidates: vec![
                Candidate {
                    surface: "第一候補".to_owned(),
                    cost: 100,
                },
                Candidate {
                    surface: "通常候補".to_owned(),
                    cost: 5_000,
                },
                Candidate {
                    surface: "完全正解".to_owned(),
                    cost: 1_500,
                },
            ],
            model_supplemental: vec![false, false, true],
            generative_consensus: Some(GenerativeConsensus {
                candidate: 2,
                kind: GenerativeConsensusKind::ModelVerifiedWhole,
                accepts_whole_result: false,
            }),
        };
        let (order, _, selected) =
            candidate_rescore_order_for_state(&competing_state, &[0.0, 10.0, 11.79], 0.8, 0.0)
                .expect("aligned finite scores");
        assert_eq!(selected, 1);
        assert_eq!(order, [1, 0, 2]);
    }

    #[test]
    fn model_verified_whole_result_preserves_a_quoted_name() {
        let reading = "しょうがくせい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "第一候補", 100),
            DictionaryEntry::new(reading, "完全正解", 2_100),
        ]);
        let base = exact_candidate(&dictionary, reading, "第一候補");

        let mut ordinary =
            engine_with_rescore_candidates(dictionary.clone(), reading, vec![base.clone()]);
        assert!(
            ordinary
                .prepare_generative_rescore_candidate("完全正解")
                .is_some(),
            "the wider cost window should remain available outside quoted names"
        );

        let mut quoted = engine_with_rescore_candidates(dictionary, reading, vec![base]);
        let state = quoted
            .candidate_rescore
            .as_mut()
            .expect("pending rescore state");
        state.request.context = "いったい、「".to_owned();
        state.request.right_context = "研究所」とは何か".to_owned();
        assert_eq!(
            quoted.prepare_generative_rescore_candidate("完全正解"),
            None,
            "a model-only whole rewrite must not replace a decisive quoted name"
        );
    }

    #[test]
    fn quoted_span_detection_uses_the_nearest_paired_boundaries() {
        assert!(super::is_quoted_span(
            "『閉じた引用』の後に“",
            "研究所”とは何か",
        ));
        assert!(!super::is_quoted_span(
            "“閉じた引用”の後",
            "研究所”とは何か",
        ));
        assert!(!super::is_quoted_span("「入れ子の『", "語句」だけ",));
    }

    #[test]
    fn model_verified_whole_result_rejects_wide_cost_or_length_changes() {
        let reading = "しょうがくせい";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new(reading, "第一候補", 100),
            DictionaryEntry::new(reading, "完全正解", 3_201),
            DictionaryEntry::new(reading, "完全な正解", 3_200),
        ]);
        let base = exact_candidate(&dictionary, reading, "第一候補");

        for generated in ["完全正解", "完全な正解"] {
            let mut engine =
                engine_with_rescore_candidates(dictionary.clone(), reading, vec![base.clone()]);
            assert_eq!(
                engine.prepare_generative_rescore_candidate(generated),
                None,
                "{generated} must remain outside the scored pool"
            );
        }
    }

    #[test]
    fn model_verified_whole_result_preserves_ascii_kanji_and_personal_names() {
        let cases = [
            (
                "えーびーしーこうほ",
                vec![
                    DictionaryEntry::new("えーびーしーこうほ", "ABC候補", 100),
                    DictionaryEntry::new("えーびーしーこうほ", "ABD正解", 1_500),
                ],
                "ABC候補",
                "ABD正解",
            ),
            (
                "かんじこうほ",
                vec![
                    DictionaryEntry::new("かんじこうほ", "漢字候補", 100),
                    DictionaryEntry::new("かんじこうほ", "かな正解", 1_500),
                ],
                "漢字候補",
                "かな正解",
            ),
        ];
        for (reading, entries, current, generated) in cases {
            let dictionary = Dictionary::new(entries);
            let base = exact_candidate(&dictionary, reading, current);
            let mut engine = engine_with_rescore_candidates(dictionary, reading, vec![base]);
            assert_eq!(
                engine.prepare_generative_rescore_candidate(generated),
                None,
                "{generated} must not cross a surface-preservation boundary"
            );
        }

        let reading = "かたせしまかていでした";
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("かたせしま", "片瀬志麻", 1_923, 1_922, 10),
            DictionaryEntry::with_pos("かたせ", "片瀬", 1_923, 1_923, 20),
            DictionaryEntry::with_pos("しま", "志摩", 1_922, 1_922, 20),
            DictionaryEntry::new("かてい", "課程", 10),
            DictionaryEntry::new("でした", "でした", 10),
            DictionaryEntry::new(reading, "片瀬志摩過程デシタ", 32_420),
        ]);
        let base = exact_candidate(&dictionary, reading, "片瀬志麻課程でした");
        assert!(dictionary.changes_exact_personal_name_segment(
            reading,
            "片瀬志麻課程でした",
            "片瀬志摩過程デシタ",
        ));
        let mut engine = engine_with_rescore_candidates(dictionary, reading, vec![base]);
        assert_eq!(
            engine.prepare_generative_rescore_candidate("片瀬志摩過程デシタ"),
            None,
            "an exact dictionary-backed personal name must remain unchanged"
        );
    }

    #[test]
    fn bounded_local_correction_never_reinterprets_ascii_alphanumerics() {
        assert!(bounded_local_substitution("奨学の問題", "少額の問題", 2));
        assert!(!bounded_local_substitution("紀元前511", "紀元前後11", 2));
        assert!(!bounded_local_substitution("abc版", "abd版", 2));
    }

    #[test]
    fn multi_region_surface_compression_is_bounded_and_preserves_ascii() {
        assert!(super::bounded_multi_region_surface_compression(
            "あいの問題",
            "愛の課題"
        ));
        assert!(super::bounded_multi_region_surface_compression(
            "そしてエンジェル帯に復讐渡渉していろいろなちょっかい",
            "そしてエンジェル隊に復讐と称して色々なちょっかい"
        ));
        assert!(!super::bounded_multi_region_surface_compression(
            "浮きの先駆け",
            "雨季のさきがけ"
        ));
        assert!(!super::bounded_multi_region_surface_compression(
            "abcあいの問題",
            "abd愛の課題"
        ));
        assert!(!super::bounded_multi_region_surface_compression(
            "あいうえの問題",
            "愛の課題"
        ));
    }

    #[test]
    fn local_correction_never_deconverts_kanji_to_hiragana() {
        assert!(!preserves_kanji_from_hiragana_deconversion("不", "ふ"));
        assert!(preserves_kanji_from_hiragana_deconversion(
            "奨学の問題",
            "少額の問題"
        ));
        assert!(preserves_kanji_from_hiragana_deconversion(
            "セウ知る",
            "セウシル"
        ));
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
    fn established_history_and_typo_corrections_are_never_exposed_to_external_scoring() {
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
    fn external_scoring_can_override_one_off_dictionary_history() {
        let directory = test_directory("rescore-transient-history");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t感じ\t1\t10\n",
        )
        .unwrap();
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("かんじ", "漢字", 1_000),
            DictionaryEntry::new("かんじ", "感じ", 1_100),
        ]);
        let mut engine = SlimeEngine::with_user_data(dictionary, UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            history_completion: true,
            history_learning: true,
            ..EnginePreferences::default()
        });

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().candidates[0], "感じ");
        assert_eq!(
            engine
                .candidate_rescore_request()
                .expect("one-off dictionary history should remain scoreable")
                .candidates,
            ["漢字", "感じ"]
        );

        engine
            .apply_candidate_rescore(&[0.0, -10.0], 0.8, 0.5)
            .expect("aligned model scores should apply");
        assert_eq!(engine.snapshot().candidates[0], "漢字");
        assert!(engine.snapshot().candidates.contains(&"感じ".to_owned()));

        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t感じ\t5\t20\n",
        )
        .unwrap();
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("かんじ", "漢字", 1_000),
            DictionaryEntry::new("かんじ", "感じ", 1_100),
        ]);
        let mut established = SlimeEngine::with_user_data(dictionary, UserData::load(&directory));
        established.set_preferences(EnginePreferences {
            history_completion: true,
            history_learning: true,
            ..EnginePreferences::default()
        });
        type_text(&mut established, "kanji");
        established.handle(InputEvent::Space);
        assert_eq!(established.snapshot().candidates[0], "感じ");
        assert!(established.candidate_rescore_request().is_none());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn context_ablated_scores_preserve_an_exact_word_from_hiragana_fragments() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context("途中でジュールが", "エドモンが完成。");
        type_text(&mut engine, "なくなったためあに");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("close exact and fragmented candidates should be scoreable");
        let exact = request
            .candidates
            .iter()
            .position(|candidate| candidate == "亡くなったため兄")
            .expect("exact candidate");
        let fragmented = request
            .candidates
            .iter()
            .position(|candidate| candidate == "なくなったため兄")
            .expect("fragmented candidate");
        let mut contextual = vec![-10.0; request.candidates.len()];
        let mut ablated = vec![-10.0; request.candidates.len()];
        contextual[fragmented] = 10.0;
        ablated[exact] = 10.0;
        ablated[fragmented] = 9.26;

        assert!(!engine.candidate_rescore_should_use_context_ablated_scores(
            &contextual,
            &ablated,
            0.8,
            0.0,
        ));

        ablated[fragmented] = 9.25;

        assert!(engine.candidate_rescore_should_use_context_ablated_scores(
            &contextual,
            &ablated,
            0.8,
            0.0,
        ));

        let mut intentional = SlimeEngine::bundled();
        intentional.set_external_left_context("つまり");
        type_text(&mut intentional, "いういみあい");
        intentional.handle(InputEvent::Space);
        let request = intentional
            .candidate_rescore_request()
            .expect("orthographic alternatives should be scoreable");
        let kanji = request
            .candidates
            .iter()
            .position(|candidate| candidate == "言う意味合い")
            .expect("kanji candidate");
        let hiragana = request
            .candidates
            .iter()
            .position(|candidate| candidate == "いう意味合い")
            .expect("hiragana candidate");
        let mut contextual = vec![-10.0; request.candidates.len()];
        let mut ablated = vec![-10.0; request.candidates.len()];
        contextual[hiragana] = 10.0;
        ablated[kanji] = 10.0;
        assert!(
            !intentional.candidate_rescore_should_use_context_ablated_scores(
                &contextual,
                &ablated,
                0.8,
                0.0,
            )
        );
    }

    #[test]
    fn context_ablated_scores_preserve_an_exact_phrase_across_the_caret() {
        let mut engine = SlimeEngine::bundled();
        engine.set_external_context("横浜横須賀", "湘南バイパスは、終日5割引。 ");
        type_text(&mut engine, "どーろとしん");
        engine.handle(InputEvent::Space);

        let request = engine
            .candidate_rescore_request()
            .expect("right-phrase alternatives should be scoreable");
        let exact_phrase = request
            .candidates
            .iter()
            .position(|candidate| candidate == "道路と新")
            .expect("exact cross-caret phrase candidate");
        let contextual = request
            .candidates
            .iter()
            .position(|candidate| candidate == "道路都心")
            .expect("contextual alternative");
        assert_eq!(exact_phrase, 0, "dictionary evidence should rank first");
        let mut contextual_scores = vec![-10.0; request.candidates.len()];
        let mut ablated_scores = vec![-10.0; request.candidates.len()];
        contextual_scores[contextual] = 10.0;
        ablated_scores[exact_phrase] = 10.0;
        ablated_scores[contextual] = 9.68;

        assert!(engine.candidate_rescore_should_use_context_ablated_scores(
            &contextual_scores,
            &ablated_scores,
            0.8,
            0.0,
        ));
    }

    #[test]
    fn external_scoring_keeps_one_off_custom_history_ahead_of_dictionary_candidates() {
        let directory = test_directory("rescore-custom-transient-history");
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nかんじ\t私の表記\t1\t10\n",
        )
        .unwrap();
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("かんじ", "漢字", 1_000),
            DictionaryEntry::new("かんじ", "感じ", 1_100),
        ]);
        let mut engine = SlimeEngine::with_user_data(dictionary, UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            history_completion: true,
            history_learning: true,
            ..EnginePreferences::default()
        });

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().candidates[0], "私の表記");

        engine
            .apply_candidate_rescore(&[0.0, -10.0], 0.8, 0.5)
            .expect("dictionary candidates should remain scoreable");
        assert_eq!(engine.snapshot().candidates[0], "私の表記");
        assert!(engine.snapshot().candidates.contains(&"漢字".to_owned()));

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
    fn cycling_past_expanded_long_candidates_runs_second_bounded_search() {
        let reading = "わたしはにほんじん";
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "watashihanihonjin");
        engine.handle(InputEvent::Space);

        let initial_count = engine.snapshot().candidates.len();
        for _ in 0..initial_count {
            engine.handle(InputEvent::NextCandidate);
        }
        let expanded = engine.snapshot();
        let expanded_count = expanded.candidates.len();
        let target = engine
            .conversion_candidates_for_reading_with_limit(
                reading,
                Some(super::LONG_DEEPENED_N_BEST),
            )
            .into_iter()
            .find(|surface| !expanded.candidates.contains(surface))
            .expect("bounded N-best 32 should add a candidate beyond N-best 16");
        let current = expanded.selected.expect("conversion has a selection");
        assert_eq!(engine.conversion_search, ConversionSearch::Expanded);

        for _ in current..expanded_count {
            engine.handle(InputEvent::NextCandidate);
        }

        let deepened = engine.snapshot();
        assert!(deepened.candidates.contains(&target));
        assert_eq!(deepened.selected, Some(expanded_count));
        assert_eq!(engine.conversion_search, ConversionSearch::Deepened);
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
    fn bounded_compound_recall_uses_pronunciation_style_long_marks() {
        let mut engine = SlimeEngine::bundled();
        type_text(&mut engine, "こーてーけん");
        engine.handle(InputEvent::Space);

        let target = "皇帝兼".to_owned();
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
