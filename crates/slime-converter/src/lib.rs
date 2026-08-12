//! A small, deterministic kana-kanji conversion baseline backed by a compact
//! dictionary.

mod compact;
mod pronunciation;
mod ranking;
mod symbol_candidates;

pub use ranking::{CandidateRanker, CostOnlyRanker, DocumentContextRanker};

use bumpalo::{Bump, collections::String as BumpString};
use compact::CompactDictionary;
use compact_str::CompactString;
use pronunciation::{
    MAX_ADDED_CONVERSIONS as LONG_VOWEL_MAX_ADDED_CONVERSIONS,
    PATHS_PER_VARIANT as LONG_VOWEL_PATHS_PER_VARIANT, orthographic_long_vowel_variants,
    remap_conversion as remap_pronunciation_conversion,
    sort_and_deduplicate as sort_and_deduplicate_conversions,
    substitution_cost as long_vowel_substitution_cost,
};
use std::cell::RefCell;
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
    katakana_run_character_cost: i32,
}

/// Adds dictionary-backed phrase and word-boundary evidence to the narrower
/// repeated-surface signal. Every signal only reorders candidates already
/// produced by the lattice; none changes the dictionary or retains document
/// text.
struct DictionaryDocumentContextRanker<'a> {
    dictionary: &'a Dictionary,
    right_context: &'a str,
    boundary_promotions: Vec<DocumentBoundaryPromotion<'a>>,
    right_phrase_promotions: Vec<DocumentBoundaryPromotion<'a>>,
    strong_left_phrase_evidence: StrongLeftPhraseEvidence,
    right_function_word_costs: Vec<DocumentContextualCost<'a>>,
    right_particle_costs: Vec<DocumentContextualCost<'a>>,
    right_grammar_costs: Vec<DocumentContextualCost<'a>>,
    right_inflection_promotions: Vec<DocumentBoundaryPromotion<'a>>,
    right_auxiliary_costs: Vec<DocumentContextualCost<'a>>,
    unique_right_grammar_surface: Option<&'a str>,
    has_polite_right_context: bool,
    right_grammar_pos_id: Option<u16>,
    unique_right_suru_surface: Option<&'a str>,
    surrounding_notation: Option<(&'static str, i32)>,
    follows_region_name: bool,
    numeric_counter_promotions: Vec<(&'static str, i32)>,
    numeric_style: Option<DocumentNumericStyleEvidence>,
    measurement_abbreviation_style: Option<DocumentNumericStyle>,
    allows_single_character_phrase_prefix: bool,
    multi_segment_right_phrase_cache: RefCell<Vec<MultiSegmentRightPhraseCacheEntry<'a>>>,
    multi_segment_right_grammar_cache: RefCell<Vec<MultiSegmentRightGrammarCacheEntry<'a>>>,
}

#[derive(Clone, Copy, Debug)]
struct DocumentBoundaryPromotion<'a> {
    surface: &'a str,
    isolated_cost: i32,
    promotion: i32,
}

#[derive(Clone, Copy, Debug)]
struct DocumentContextualCost<'a> {
    surface: &'a str,
    relative_cost: i32,
}

#[derive(Debug)]
struct MultiSegmentRightPhraseCacheEntry<'a> {
    reading: CompactString,
    prefix_surface: CompactString,
    promotions: Vec<(&'a str, i32)>,
}

#[derive(Debug)]
struct MultiSegmentRightGrammarCacheEntry<'a> {
    reading: CompactString,
    costs: Vec<DocumentContextualCost<'a>>,
    unique_surface: Option<&'a str>,
    protects_strong_kana_surface: bool,
}

impl MultiSegmentRightGrammarCacheEntry<'_> {
    fn promotion_for(&self, surface: &str, has_polite_right_context: bool) -> i32 {
        let contextual = self
            .costs
            .iter()
            .filter(|contextual| contextual.surface == surface)
            .map(|contextual| {
                DOCUMENT_MULTI_SEGMENT_RIGHT_GRAMMAR_PROMOTION_CAP
                    .saturating_sub(contextual.relative_cost)
                    .clamp(0, DOCUMENT_MULTI_SEGMENT_RIGHT_GRAMMAR_PROMOTION_CAP)
            })
            .max()
            .unwrap_or(0);
        let unique = if self.unique_surface == Some(surface) {
            if has_polite_right_context {
                DOCUMENT_UNIQUE_RIGHT_POLITE_PROMOTION
            } else {
                DOCUMENT_UNIQUE_RIGHT_GRAMMAR_PROMOTION
            }
        } else {
            0
        };
        let promotion = contextual.max(unique);
        // A multi-character kana verb with a very low dictionary cost is an
        // established spelling, not merely an unconverted fallback. Grammar
        // can keep an ideographic alternative visible but must not erase that
        // lexical evidence on its own.
        if self.protects_strong_kana_surface {
            promotion.min(DOCUMENT_STRONG_KANA_GRAMMAR_PROMOTION_CAP)
        } else {
            promotion
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StrongLeftPhraseEvidence {
    Absent,
    Present,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DocumentNumericStyle {
    Ascii,
    Fullwidth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DocumentNumericStyleEvidence {
    style: DocumentNumericStyle,
    leading: Option<DocumentLeadingNumericStyle>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DocumentLeadingNumericStyle {
    reading_bytes: usize,
    kanji: char,
}

impl<'a> DictionaryDocumentContextRanker<'a> {
    fn new(dictionary: &'a Dictionary, reading: &str, left_context: &str) -> Self {
        Self::new_with_surrounding_context(dictionary, reading, left_context, "")
    }

    fn new_with_surrounding_context(
        dictionary: &'a Dictionary,
        reading: &str,
        left_context: &str,
        right_context: &'a str,
    ) -> Self {
        // Mozc gives 最 its own productive superlative-prefix POS, while full
        // dictionary entries capture honorific forms such as お菓子. Checking the
        // prefix surface keeps this bounded without a reverse scan on every
        // conversion. A following copula keeps ambiguous inflected forms out.
        let left_context_ends_with_honorific_prefix =
            document_context_ends_with_honorific_prefix(left_context);
        let allows_single_character_phrase_prefix = left_context.ends_with('最')
            || (left_context_ends_with_honorific_prefix && !starts_with_copula(right_context));
        let right_phrase_promotions = dictionary.document_right_phrase_promotions(
            reading,
            left_context,
            right_context,
            allows_single_character_phrase_prefix,
        );
        let strong_left_phrase_evidence = dictionary.document_strong_left_phrase_evidence(
            reading,
            left_context,
            allows_single_character_phrase_prefix,
        );
        Self {
            dictionary,
            right_context,
            boundary_promotions: dictionary.document_boundary_promotions(reading, left_context),
            right_phrase_promotions,
            strong_left_phrase_evidence,
            right_function_word_costs: dictionary
                .document_right_function_word_costs(reading, right_context),
            right_particle_costs: dictionary.document_right_particle_costs(reading, right_context),
            right_grammar_costs: dictionary.document_right_grammar_costs(reading, right_context),
            right_inflection_promotions: dictionary
                .document_right_inflection_promotions(reading, right_context),
            right_auxiliary_costs: dictionary
                .document_right_auxiliary_costs(reading, right_context),
            unique_right_grammar_surface: dictionary
                .document_unique_right_grammar_surface(reading, right_context),
            has_polite_right_context: starts_with_polite_auxiliary(right_context),
            right_grammar_pos_id: document_right_grammar_pos_id(right_context),
            unique_right_suru_surface: dictionary
                .document_unique_right_suru_surface(reading, right_context),
            surrounding_notation: surrounding_structured_notation(
                left_context,
                reading,
                right_context,
            ),
            follows_region_name: is_document_region_suffix_reading(reading)
                && dictionary.document_context_has_pos_suffix(
                    left_context,
                    &MOZC_REGION_POS_IDS,
                    2,
                    DOCUMENT_REGION_MAX_CONTEXT_CHARACTERS,
                ),
            numeric_counter_promotions: dictionary
                .document_numeric_counter_promotions(reading, left_context),
            numeric_style: document_numeric_style_evidence(left_context, reading, right_context),
            measurement_abbreviation_style: document_measurement_abbreviation_style(
                left_context,
                reading,
            ),
            allows_single_character_phrase_prefix,
            multi_segment_right_phrase_cache: RefCell::new(Vec::new()),
            multi_segment_right_grammar_cache: RefCell::new(Vec::new()),
        }
    }

    fn phrase_promotion(&self, reading: &str, left_context: &str, surface: &str) -> i32 {
        let Some(word_cost) = self.dictionary.document_phrase_word_cost(
            left_context,
            reading,
            surface,
            self.allows_single_character_phrase_prefix,
        ) else {
            return 0;
        };
        let regular = DOCUMENT_PHRASE_COST_CEILING
            .saturating_sub(word_cost)
            .min(DOCUMENT_PHRASE_PROMOTION);
        if regular <= 0 || !self.right_phrase_promotions.is_empty() {
            return regular;
        }
        if self.dictionary.document_left_phrase_is_unique(
            reading,
            left_context,
            surface,
            self.allows_single_character_phrase_prefix,
        ) {
            DOCUMENT_UNIQUE_PHRASE_COST_CEILING
                .saturating_sub(word_cost)
                .min(DOCUMENT_UNIQUE_PHRASE_PROMOTION)
        } else {
            regular
        }
    }

    fn right_function_word_promotion(&self, conversion: &Conversion) -> i32 {
        self.right_function_word_costs
            .iter()
            .filter(|contextual| {
                conversion.segments.len() == 1 && contextual.surface == conversion.surface
            })
            .map(|contextual| {
                DOCUMENT_RIGHT_FUNCTION_WORD_PROMOTION_CAP
                    .saturating_sub(contextual.relative_cost)
                    .clamp(0, DOCUMENT_RIGHT_FUNCTION_WORD_PROMOTION_CAP)
            })
            .max()
            .unwrap_or(0)
    }

    fn right_particle_promotion(&self, conversion: &Conversion) -> i32 {
        if !self.right_phrase_promotions.is_empty()
            || self.strong_left_phrase_evidence == StrongLeftPhraseEvidence::Present
        {
            return 0;
        }
        self.right_particle_costs
            .iter()
            .filter(|contextual| {
                conversion.segments.len() == 1 && contextual.surface == conversion.surface
            })
            .map(|contextual| {
                DOCUMENT_RIGHT_PARTICLE_PROMOTION_CAP
                    .saturating_sub(contextual.relative_cost)
                    .clamp(0, DOCUMENT_RIGHT_PARTICLE_PROMOTION_CAP)
            })
            .max()
            .unwrap_or(0)
    }

    fn right_grammar_promotion(&self, conversion: &Conversion) -> i32 {
        self.right_grammar_costs
            .iter()
            .filter(|contextual| {
                conversion.segments.len() == 1 && contextual.surface == conversion.surface
            })
            .map(|contextual| {
                DOCUMENT_RIGHT_GRAMMAR_PROMOTION_CAP
                    .saturating_sub(contextual.relative_cost)
                    .clamp(0, DOCUMENT_RIGHT_GRAMMAR_PROMOTION_CAP)
            })
            .max()
            .unwrap_or(0)
    }

    fn unique_right_grammar_promotion(&self, conversion: &Conversion) -> i32 {
        if conversion.segments.len() != 1
            || self.unique_right_grammar_surface != Some(conversion.surface.as_str())
        {
            return 0;
        }
        if self.has_polite_right_context {
            DOCUMENT_UNIQUE_RIGHT_POLITE_PROMOTION
        } else {
            DOCUMENT_UNIQUE_RIGHT_GRAMMAR_PROMOTION
        }
    }

    fn multi_segment_right_phrase_promotion(&self, conversion: &Conversion) -> i32 {
        if self.right_context.is_empty() || conversion.segments.len() < 2 {
            return 0;
        }
        let Some(last) = conversion.segments.last() else {
            return 0;
        };
        let Some(prefix_surface) = conversion.surface.strip_suffix(&last.surface) else {
            return 0;
        };
        let allows_single_character_phrase_prefix = prefix_surface.ends_with('最')
            || (document_context_ends_with_honorific_prefix(prefix_surface)
                && !starts_with_copula(self.right_context));
        if let Some(promotion) = self
            .multi_segment_right_phrase_cache
            .borrow()
            .iter()
            .find(|cached| {
                cached.reading == last.reading && cached.prefix_surface == prefix_surface
            })
            .map(|cached| {
                cached
                    .promotions
                    .iter()
                    .filter(|(surface, _)| *surface == last.surface)
                    .map(|(_, promotion)| *promotion)
                    .max()
                    .unwrap_or(0)
            })
        {
            return promotion;
        }
        let promotions = self
            .dictionary
            .document_multi_segment_right_phrase_promotions(
                &last.reading,
                prefix_surface,
                self.right_context,
                allows_single_character_phrase_prefix,
            );
        let promotion = promotions
            .iter()
            .filter(|(surface, _)| *surface == last.surface)
            .map(|(_, promotion)| *promotion)
            .max()
            .unwrap_or(0);
        self.multi_segment_right_phrase_cache.borrow_mut().push(
            MultiSegmentRightPhraseCacheEntry {
                reading: last.reading.as_str().into(),
                prefix_surface: prefix_surface.into(),
                promotions,
            },
        );
        promotion
    }

    fn multi_segment_right_grammar_promotion(&self, conversion: &Conversion) -> i32 {
        if self.right_grammar_pos_id.is_none() || conversion.segments.len() < 2 {
            return 0;
        }
        let Some(last) = conversion.segments.last() else {
            return 0;
        };
        let cached = self.multi_segment_right_grammar_cache.borrow();
        if let Some(entry) = cached.iter().find(|entry| entry.reading == last.reading) {
            return entry.promotion_for(last.surface.as_str(), self.has_polite_right_context);
        }
        drop(cached);

        let costs = self
            .dictionary
            .document_right_grammar_costs(&last.reading, self.right_context);
        let unique_surface = self
            .dictionary
            .document_unique_right_grammar_surface(&last.reading, self.right_context);
        let entry = MultiSegmentRightGrammarCacheEntry {
            reading: last.reading.as_str().into(),
            costs,
            unique_surface,
            protects_strong_kana_surface: self
                .dictionary
                .document_has_strong_kana_verb_surface(&last.reading),
        };
        let promotion = entry.promotion_for(last.surface.as_str(), self.has_polite_right_context);
        self.multi_segment_right_grammar_cache
            .borrow_mut()
            .push(entry);
        promotion
    }

    fn boundary_adjustment(&self, conversion: &Conversion, specialized_promotion: i32) -> i32 {
        if specialized_promotion > 0
            || self.strong_left_phrase_evidence == StrongLeftPhraseEvidence::Present
        {
            return 0;
        }
        // A post-hoc boundary score is exact only when the complete candidate
        // is one dictionary word. Multi-segment paths would need the context
        // transition inside the lattice search.
        self.boundary_promotions
            .iter()
            .filter(|promotion| {
                conversion.segments.len() == 1
                    && promotion.surface == conversion.surface
                    && promotion.isolated_cost == conversion.cost
            })
            .map(|promotion| promotion.promotion)
            .max()
            .unwrap_or(0)
            .saturating_neg()
    }

    fn numeric_particle_suru_promotion(&self, conversion: &Conversion) -> i32 {
        if !starts_with_suru_inflection(self.right_context) {
            return 0;
        }
        let [.., numeric, particle, verbal_noun] = conversion.segments.as_slice() else {
            return 0;
        };
        if particle.reading != "に" || particle.surface != "に" {
            return 0;
        }
        // さん before に is often the honorific/plural ending in 皆さんに,
        // not the standalone digit 3. Structural evidence on the right must
        // not reinterpret that left boundary as a numeral.
        if numeric.reading == "さん" {
            return 0;
        }
        let whole_numeric_surface = split_trailing_decimal(&numeric.surface)
            .is_some_and(|(prefix, _)| prefix.is_empty())
            || numeric.surface.chars().all(is_japanese_numeric_character);
        if !whole_numeric_surface {
            return 0;
        }
        if self
            .dictionary
            .document_unique_right_suru_surface(&verbal_noun.reading, self.right_context)
            == Some(verbal_noun.surface.as_str())
        {
            DOCUMENT_NUMERIC_PARTICLE_SURU_PROMOTION
        } else {
            0
        }
    }

    fn quotation_reporting_promotion(&self, left_context: &str, conversion: &Conversion) -> i32 {
        if !document_context_ends_with_quotation_case(left_context)
            || document_right_grammar_pos_id(self.right_context).is_none()
        {
            return 0;
        }
        let [.., _recipient, particle, reporting_verb] = conversion.segments.as_slice() else {
            return 0;
        };
        if particle.reading != "に" || particle.surface != "に" {
            return 0;
        }
        if is_reporting_verb_surface(&reporting_verb.surface) {
            DOCUMENT_QUOTATION_REPORTING_PROMOTION
        } else {
            0
        }
    }

    fn numeric_counter_promotion(&self, conversion: &Conversion) -> i32 {
        self.numeric_counter_promotions
            .iter()
            .find_map(|(surface, promotion)| (*surface == conversion.surface).then_some(*promotion))
            .unwrap_or(0)
            .max(preferred_numeric_counter_variant_promotion(conversion))
            .max(assimilated_score_promotion(self.right_context, conversion))
    }

    fn right_structured_promotion(&self, conversion: &Conversion) -> i32 {
        chronological_year_promotion(self.right_context, conversion).max(
            approximate_quantity_promotion(self.right_context, conversion),
        )
    }
}

impl CandidateRanker for DictionaryDocumentContextRanker<'_> {
    fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
        conversion.cost
    }

    fn ranking_cost_with_context(
        &self,
        reading: &str,
        left_context: &str,
        conversion: &Conversion,
    ) -> i32 {
        let repeated_cost =
            DocumentContextRanker.ranking_cost_with_context(reading, left_context, conversion);
        let phrase_promotion = self.phrase_promotion(reading, left_context, &conversion.surface);
        let right_phrase_promotion = self
            .right_phrase_promotions
            .iter()
            .filter(|promotion| {
                conversion.segments.len() == 1
                    && promotion.surface == conversion.surface
                    && promotion.isolated_cost == conversion.cost
            })
            .map(|promotion| promotion.promotion)
            .max()
            .unwrap_or(0)
            .max(self.multi_segment_right_phrase_promotion(conversion));
        let right_function_word_promotion = self.right_function_word_promotion(conversion);
        let right_particle_promotion = self.right_particle_promotion(conversion);
        let right_grammar_promotion = self
            .right_grammar_promotion(conversion)
            .max(self.multi_segment_right_grammar_promotion(conversion));
        let notation_promotion =
            if structured_notation_matches(left_context, reading, &conversion.surface) {
                DOCUMENT_STRUCTURED_NOTATION_PROMOTION
            } else {
                self.surrounding_notation
                    .filter(|(surface, _)| *surface == conversion.surface)
                    .map_or(0, |(_, promotion)| promotion)
            };
        let numeric_counter_promotion = self.numeric_counter_promotion(conversion);
        let numeric_style_promotion = self
            .numeric_style
            .filter(|evidence| {
                conversion_matches_numeric_style(self.dictionary, conversion, *evidence)
            })
            .map_or(0, |_| DOCUMENT_NUMERIC_STYLE_PROMOTION);
        let measurement_abbreviation_promotion =
            measurement_abbreviation_promotion(self.measurement_abbreviation_style, conversion);
        let region_suffix_promotion = if self.follows_region_name {
            document_region_suffix_promotion(reading, &conversion.surface)
        } else {
            0
        };
        let right_inflection_promotion = self
            .right_inflection_promotions
            .iter()
            .filter(|promotion| {
                conversion.segments.len() == 1
                    && promotion.surface == conversion.surface
                    && promotion.isolated_cost == conversion.cost
            })
            .map(|promotion| promotion.promotion)
            .max()
            .unwrap_or(0);
        let right_auxiliary_promotion = self
            .right_auxiliary_costs
            .iter()
            .filter(|contextual| {
                conversion.segments.len() == 1 && contextual.surface == conversion.surface
            })
            .map(|contextual| {
                DOCUMENT_RIGHT_AUXILIARY_PROMOTION_CAP
                    .saturating_sub(contextual.relative_cost)
                    .clamp(0, DOCUMENT_RIGHT_AUXILIARY_PROMOTION_CAP)
            })
            .max()
            .unwrap_or(0);
        let unique_right_grammar_promotion = self.unique_right_grammar_promotion(conversion);
        let unique_right_suru_promotion = if conversion.segments.len() == 1
            && self.unique_right_suru_surface == Some(conversion.surface.as_str())
        {
            DOCUMENT_UNIQUE_RIGHT_SURU_PROMOTION
        } else {
            self.numeric_particle_suru_promotion(conversion)
        };
        let quotation_reporting_promotion =
            self.quotation_reporting_promotion(left_context, conversion);
        let foreign_name_honorific_promotion =
            foreign_name_honorific_promotion(left_context, self.right_context, conversion);
        let specialized_promotion = phrase_promotion
            .max(notation_promotion)
            .max(numeric_counter_promotion)
            .max(numeric_style_promotion)
            .max(measurement_abbreviation_promotion)
            .max(region_suffix_promotion)
            .max(self.right_structured_promotion(conversion));
        let boundary_adjustment = self.boundary_adjustment(conversion, specialized_promotion);
        repeated_cost
            .saturating_add(boundary_adjustment)
            .saturating_sub(specialized_promotion)
            .saturating_sub(
                right_phrase_promotion
                    .max(right_function_word_promotion)
                    .max(right_particle_promotion),
            )
            .saturating_sub(right_inflection_promotion)
            .saturating_sub(right_auxiliary_promotion)
            .saturating_sub(right_grammar_promotion.max(unique_right_grammar_promotion))
            .saturating_sub(unique_right_suru_promotion)
            .saturating_sub(quotation_reporting_promotion)
            .saturating_sub(foreign_name_honorific_promotion)
    }
}

fn approximate_quantity_promotion(right_context: &str, conversion: &Conversion) -> i32 {
    if !right_context_starts_with_quantity(right_context) {
        return 0;
    }
    let [.., case, topic, approximate] = conversion.segments.as_slice() else {
        return 0;
    };
    if case.reading == "に"
        && case.surface == "に"
        && topic.reading == "は"
        && topic.surface == "は"
        && approximate.reading == "やく"
        && approximate.surface == "約"
    {
        DOCUMENT_APPROXIMATE_QUANTITY_PROMOTION
    } else {
        0
    }
}

fn right_context_starts_with_quantity(right_context: &str) -> bool {
    let mut characters = right_context.chars().peekable();
    let decimal = characters
        .peek()
        .copied()
        .is_some_and(|character| decimal_digit(character).is_some());
    let japanese = characters
        .peek()
        .copied()
        .is_some_and(is_japanese_numeric_character);
    if !decimal && !japanese {
        return false;
    }
    if decimal {
        while characters
            .peek()
            .is_some_and(|character| decimal_digit(*character).is_some())
        {
            characters.next();
        }
    } else {
        while characters
            .peek()
            .is_some_and(|character| is_japanese_numeric_character(*character))
        {
            characters.next();
        }
    }
    characters.next().is_some_and(|unit| {
        matches!(
            unit,
            '年' | '月' | '日' | '時' | '分' | '秒' | '件' | '人' | '個' | '回' | '円'
        )
    })
}

fn chronological_year_promotion(right_context: &str, conversion: &Conversion) -> i32 {
    if !right_context.starts_with('年') {
        return 0;
    }
    let [.., era, year] = conversion.segments.as_slice() else {
        return 0;
    };
    let is_chronological_era = matches!(
        (era.reading.as_str(), era.surface.as_str()),
        ("きげんぜん", "紀元前") | ("きげんご", "紀元後") | ("せいれき", "西暦")
    );
    if is_chronological_era
        && !year.surface.is_empty()
        && year.surface.chars().all(|character| {
            decimal_digit(character).is_some() || is_japanese_numeric_character(character)
        })
    {
        DOCUMENT_CHRONOLOGICAL_YEAR_PROMOTION
    } else {
        0
    }
}

fn foreign_name_honorific_promotion(
    left_context: &str,
    right_context: &str,
    conversion: &Conversion,
) -> i32 {
    const PROMOTION: i32 = 500;

    if !left_context.ends_with('・')
        || !right_context.chars().next().is_some_and(|character| {
            matches!(character, 'は' | 'が' | 'を' | 'に' | 'の' | 'と' | 'も')
        })
    {
        return 0;
    }
    let [name @ .., honorific] = conversion.segments.as_slice() else {
        return 0;
    };
    let name_characters = name
        .iter()
        .map(|segment| segment.surface.chars().count())
        .sum::<usize>();
    if honorific.reading == "し"
        && honorific.surface == "氏"
        && name_characters >= 5
        && name
            .iter()
            .all(|segment| is_full_katakana_surface(&segment.surface))
    {
        PROMOTION
    } else {
        0
    }
}

fn measurement_abbreviation_promotion(
    style: Option<DocumentNumericStyle>,
    conversion: &Conversion,
) -> i32 {
    style
        .filter(|style| conversion_matches_measurement_abbreviation(conversion, *style))
        .map_or(0, |_| DOCUMENT_NUMERIC_STYLE_PROMOTION)
}

fn preferred_numeric_counter_variant_promotion(conversion: &Conversion) -> i32 {
    let [numeric @ .., counter] = conversion.segments.as_slice() else {
        return 0;
    };
    if numeric.is_empty()
        || counter.reading != "へん"
        || counter.surface != "編"
        || !numeric.iter().all(|segment| {
            !segment.surface.is_empty()
                && segment
                    .surface
                    .chars()
                    .all(|character| matches!(character, '0'..='9' | '０'..='９'))
        })
    {
        return 0;
    }
    DOCUMENT_PREFERRED_NUMERIC_COUNTER_VARIANT_PROMOTION
}

fn assimilated_score_promotion(right_context: &str, conversion: &Conversion) -> i32 {
    let expected_surface = match right_context.chars().next() {
        Some('1') => "1対",
        Some('１') => "１対",
        _ => return 0,
    };
    if conversion
        .segments
        .iter()
        .any(|segment| segment.reading == "いったい" && segment.surface == expected_surface)
    {
        DOCUMENT_STRONG_STRUCTURED_NOTATION_PROMOTION
    } else {
        0
    }
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
    segment_count: u8,
    substituted_segments: u8,
    katakana_segments: u8,
    ideographic_segments: u8,
}

impl CompoundPath {
    fn pronunciation_safe(&self) -> bool {
        if self.substituted_segments == 0 {
            return true;
        }
        if self.substituted_segments & self.ideographic_segments == 0 {
            return false;
        }
        (1..self.segment_count.saturating_sub(1)).all(|index| {
            let bit = 1_u8 << index;
            self.substituted_segments & bit == 0
                || self.katakana_segments & (bit >> 1) == 0
                || self.katakana_segments & (bit << 1) == 0
        })
    }
}

#[derive(Clone, Debug)]
struct FixedSegmentPath {
    surface: String,
    changed_segments: usize,
    relative_cost: i64,
}

#[derive(Clone, Copy, Debug)]
enum PersonalNameRole {
    Surname,
    GivenName,
}

#[derive(Clone, Copy, Debug, Default)]
struct PersonalNameRoles {
    full_name: bool,
    surname: bool,
    given_name: bool,
}

const COMPOUND_MAX_SEGMENTS: usize = 6;
const COMPOUND_MAX_READING_CHARACTERS: usize = 16;
const COMPOUND_MAX_ENTRIES_PER_SEGMENT: usize = 8;
const COMPOUND_MAX_CANDIDATES: usize = 64;
const LONG_VOWEL_COMPOUND_CANDIDATES_PER_VARIANT: usize = 20;
const LONG_VOWEL_COMPOUND_MAX_VARIANTS: usize = 6;
const PERSONAL_NAME_MIN_READING_CHARACTERS: usize = 2;
const PERSONAL_NAME_MAX_READING_CHARACTERS: usize = 16;
const PERSONAL_NAME_MAX_ENTRIES_PER_PART: usize = 64;
const PERSONAL_NAME_MAX_CANDIDATES: usize = 128;
const MOZC_PERSONAL_GIVEN_NAME_POS_ID: u16 = 1922;
const MOZC_PERSONAL_SURNAME_POS_ID: u16 = 1923;
const MOZC_INDEPENDENT_VERB_POS_ID_START: u16 = 577;
const MOZC_INDEPENDENT_VERB_POS_ID_END: u16 = 856;
const MOZC_GENERAL_GODAN_CONTINUATIVE_POS_ID: u16 = 842;
const MOZC_VERBAL_NOUN_POS_ID: u16 = 1_841;
const MOZC_GENERAL_NOUN_POS_ID: u16 = 1_851;
const MOZC_NOUN_PREFIX_POS_ID_START: u16 = 2_600;
const MOZC_EXPLICIT_NOUN_PREFIX_POS_ID_START: u16 = 2_601;
const MOZC_NOUN_PREFIX_POS_ID_END: u16 = 2_637;

fn is_bounded_coordination_suffix(suffix: &str) -> bool {
    let mut characters = suffix.chars();
    if !characters
        .next()
        .is_some_and(|character| matches!(character, 'や' | 'と'))
    {
        return false;
    }
    let mut remainder_characters = 0;
    for character in characters {
        if !matches!(
            character,
            '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
        ) {
            return false;
        }
        remainder_characters += 1;
        if remainder_characters > 7 {
            return false;
        }
    }
    remainder_characters > 0
}

fn is_bounded_genitive_suffix(suffix: &str) -> bool {
    let mut characters = suffix.chars();
    if characters.next() != Some('の') {
        return false;
    }
    let mut remainder_characters = 0;
    for character in characters {
        if !matches!(
            character,
            '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
        ) {
            return false;
        }
        remainder_characters += 1;
        if remainder_characters > 7 {
            return false;
        }
    }
    remainder_characters > 0
}

fn right_phrase_suffix_has_boundary(suffix: &str, right_context: &str) -> bool {
    match right_context
        .strip_prefix(suffix)
        .and_then(|remainder| remainder.chars().next())
    {
        Some(next) => !matches!(
            next,
            '\u{30a0}'..='\u{30ff}'
                | '\u{3400}'..='\u{4dbf}'
                | '\u{4e00}'..='\u{9fff}'
                | '\u{f900}'..='\u{faff}'
        ),
        None => true,
    }
}

fn is_safe_hiragana_right_phrase_entry(entry: compact::CompactEntry, suffix: &str) -> bool {
    if entry.left_id != entry.right_id {
        return false;
    }
    if entry.left_id == MOZC_GENERAL_NOUN_POS_ID
        && (is_bounded_coordination_suffix(suffix) || is_bounded_genitive_suffix(suffix))
    {
        return true;
    }
    let suffix_characters = suffix.chars().count();
    if suffix_characters == 1 {
        return suffix == "に" && entry.left_id == MOZC_GENERAL_GODAN_CONTINUATIVE_POS_ID;
    }
    suffix_characters >= 2
        && (MOZC_INDEPENDENT_VERB_POS_ID_START..=MOZC_INDEPENDENT_VERB_POS_ID_END)
            .contains(&entry.left_id)
}

fn is_sibling_right_phrase_suffix(suffix: &str, right_context: &str) -> bool {
    if !matches!(suffix, "兄" | "姉" | "弟" | "妹") {
        return false;
    }
    match right_context
        .strip_prefix(suffix)
        .and_then(|remainder| remainder.chars().next())
    {
        Some(next) => !matches!(next, '\u{30a0}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}'),
        None => true,
    }
}

const FIXED_SEGMENT_MAX_READING_CHARACTERS: usize = 128;
const FIXED_SEGMENT_MAX_SEGMENTS: usize = 64;
const FIXED_SEGMENT_MAX_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_MAX_CANDIDATES: usize = 128;
const FIXED_SEGMENT_MAX_STATES: usize = 256;
const DOCUMENT_PHRASE_MIN_PREFIX_CHARACTERS: usize = 2;
const DOCUMENT_PHRASE_MAX_PREFIX_CHARACTERS: usize = 8;
const DOCUMENT_RIGHT_PHRASE_MAX_SUFFIX_CHARACTERS: usize = 8;
const DOCUMENT_RIGHT_CARET_PHRASE_MAX_PREFIX_CHARACTERS: usize = 4;
// A lower-cost whole compound is stronger evidence than a marginal one. The
// cap bounds how far dictionary evidence can move an existing N-best item.
const DOCUMENT_PHRASE_COST_CEILING: i32 = 9_000;
const DOCUMENT_PHRASE_PROMOTION: i32 = 3_500;
// A sole full-phrase continuation is stronger than an isolated homophone.
// Competing left phrases or any exact right phrase retain the regular cap.
const DOCUMENT_UNIQUE_PHRASE_COST_CEILING: i32 = 11_500;
const DOCUMENT_UNIQUE_PHRASE_PROMOTION: i32 = 5_200;
const DOCUMENT_RIGHT_SHORT_PHRASE_COST_CEILING: i32 = 6_300;
const DOCUMENT_RIGHT_SIBLING_PHRASE_COST_CEILING: i32 = 7_500;
const DOCUMENT_RIGHT_COORDINATION_PHRASE_COST_CEILING: i32 = 9_300;
const DOCUMENT_RIGHT_DERIVATIONAL_PHRASE_COST_CEILING: i32 = 7_000;
const DOCUMENT_RIGHT_LONG_PHRASE_COST_CEILING: i32 = 9_000;
// Exact noun-prefix compounds are stronger than an incidental phrase ending
// on the left, but remain bounded so alternatives stay in the candidate list.
const DOCUMENT_RIGHT_NOUN_PREFIX_PHRASE_COST_CEILING: i32 = 9_000;
const DOCUMENT_RIGHT_NOUN_PREFIX_PHRASE_PROMOTION: i32 = 4_500;
const DOCUMENT_STRUCTURED_NOTATION_PROMOTION: i32 = 3_000;
const DOCUMENT_STRONG_STRUCTURED_NOTATION_PROMOTION: i32 = 4_000;
const DOCUMENT_NUMERIC_STYLE_PROMOTION: i32 = 3_000;
const DOCUMENT_CHRONOLOGICAL_YEAR_PROMOTION: i32 = 1_000;
const DOCUMENT_APPROXIMATE_QUANTITY_PROMOTION: i32 = 750;
const DOCUMENT_PREFERRED_NUMERIC_COUNTER_VARIANT_PROMOTION: i32 = 750;
const DOCUMENT_NUMERIC_COMPOUND_COST_CEILING: i32 = 8_500;
const DOCUMENT_NUMERIC_COMPOUND_PROMOTION_CAP: i32 = 2_500;
const DOCUMENT_GRAMMATICAL_PHRASE_PROMOTION: i32 = 400;
const DOCUMENT_REGION_ADMINISTRATIVE_SUFFIX_PROMOTION: i32 = 750;
const DOCUMENT_REGION_MAX_CONTEXT_CHARACTERS: usize = 8;
const DOCUMENT_POS_SURFACE_COST_GAP: i32 = 500;
const DOCUMENT_BOUNDARY_MAX_CONTEXT_CHARACTERS: usize = 8;
const DOCUMENT_BOUNDARY_PROMOTION_CAP: i32 = 1_500;
const DOCUMENT_POLITE_AUXILIARY_PROMOTION: i32 = 1_000;
const DOCUMENT_RIGHT_AUXILIARY_PROMOTION_CAP: i32 = 1_500;
const DOCUMENT_RIGHT_FUNCTION_WORD_PROMOTION_CAP: i32 = 1_100;
const DOCUMENT_RIGHT_PARTICLE_PROMOTION_CAP: i32 = 1_600;
const DOCUMENT_RIGHT_GRAMMAR_PROMOTION_CAP: i32 = 1_500;
const DOCUMENT_MULTI_SEGMENT_RIGHT_GRAMMAR_PROMOTION_CAP: i32 = 1_500;
const DOCUMENT_STRONG_KANA_GRAMMAR_PROMOTION_CAP: i32 = 1_000;
const DOCUMENT_STRONG_KANA_VERB_COST_CEILING: i32 = 500;
const DOCUMENT_UNIQUE_RIGHT_GRAMMAR_PROMOTION: i32 = 1_500;
const DOCUMENT_UNIQUE_RIGHT_POLITE_PROMOTION: i32 = 2_000;
const DOCUMENT_UNIQUE_RIGHT_SURU_PROMOTION: i32 = 2_500;
const DOCUMENT_NUMERIC_PARTICLE_SURU_PROMOTION: i32 = 6_500;
const DOCUMENT_QUOTATION_REPORTING_PROMOTION: i32 = 1_500;
const EMBEDDED_COLLOQUIAL_IMPERATIVE_PROMOTION: i32 = 3_000;
const DOCUMENT_COLLOQUIAL_IMPERATIVE_PROMOTION: i32 = 4_500;
const DOCUMENT_UNIQUE_RIGHT_SURU_COMPATIBILITY_MARGIN: i32 = 1_500;
const DOCUMENT_RIGHT_GRAMMAR_COMPATIBILITY_MARGIN: i32 = 1_000;
const MOZC_PAST_AUXILIARY_POS_ID: u16 = 142;
const MOZC_DESIDERATIVE_AUXILIARY_POS_ID: u16 = 152;
const MOZC_NEGATIVE_AUXILIARY_POS_ID: u16 = 204;
const MOZC_POLITE_AUXILIARY_POS_ID: u16 = 240;
const MOZC_TE_CONNECTIVE_PARTICLE_POS_ID: u16 = 348;
const MOZC_DE_CONNECTIVE_PARTICLE_POS_ID: u16 = 349;
// Zero-cost grammar surfaces in the bundled Mozc dictionary. Keeping these
// IDs static avoids a second dictionary lookup on the right-context hot path.
const MOZC_NODE_CONNECTIVE_PARTICLE_POS_ID: u16 = 359;
const MOZC_MONO_CASE_PARTICLE_POS_ID: u16 = 376;
const MOZC_CAUSATIVE_SASERU_CONTINUATIVE_POS_ID: u16 = 482;
const MOZC_CAUSATIVE_SERU_CONTINUATIVE_POS_ID: u16 = 484;
const MOZC_PASSIVE_RARERU_CONTINUATIVE_POS_ID: u16 = 485;
const MOZC_PASSIVE_RERU_CONTINUATIVE_POS_ID: u16 = 486;
const MOZC_YOU_GENERAL_SUFFIX_NOUN_POS_ID: u16 = 1_950;
const MOZC_COUNTER_POS_ID: u16 = 2_011;
const MOZC_KOTO_NON_INDEPENDENT_NOUN_POS_ID: u16 = 2_066;
const MOZC_TAME_NON_INDEPENDENT_NOUN_POS_ID: u16 = 2_076;
const MOZC_MONO_NON_INDEPENDENT_NOUN_POS_ID: u16 = 2_090;
const MOZC_TAME_ADVERBIAL_NOUN_POS_ID: u16 = 2_140;
const MOZC_YOU_AUXILIARY_STEM_NOUN_POS_ID: u16 = 2_192;
const MOZC_REGION_POS_IDS: [u16; 5] = [1924, 1925, 1926, 1927, 1928];

fn is_mozc_independent_imperative_pos_id(pos_id: u16) -> bool {
    matches!(
        pos_id,
        586..=589
            | 605..=608
            | 619..=620
            | 630..=632
            | 641
            | 666..=679
            | 710..=712
            | 722
            | 730
            | 738
            | 759..=762
            | 789..=790
            | 798
            | 809..=812
            | 836
            | 844
            | 850..=851
    )
}

fn document_context_ends_with_clause_boundary(context: &str) -> bool {
    context
        .trim_end_matches(|character: char| character.is_whitespace() && character != '\n')
        .chars()
        .next_back()
        .is_some_and(|character| {
            matches!(
                character,
                '、' | '。' | ',' | '，' | ':' | '：' | ';' | '；' | '!' | '！' | '?' | '？' | '\n'
            )
        })
}
const CALENDAR_KA_ENDING_DAYS: &[u32] = &[2, 3, 4, 5, 6, 7, 8, 9, 10, 14, 20, 24];
const COMMON_RADICES: &[u32] = &[2, 8, 10, 16];
const STRINGED_INSTRUMENT_PREFIXES: &[&str] = &[
    "ギター",
    "ベース",
    "バイオリン",
    "ヴァイオリン",
    "チェロ",
    "琴",
    "箏",
];
const VESSEL_NOUN_PREFIXES: &[&str] = &["の客船", "の貨物船", "の帆船", "の船", "の艦", "の艇"];
const BASE_STATE_PREFIXES: &[&str] = &[
    "一塁",
    "二塁",
    "三塁",
    "一、二塁",
    "一・二塁",
    "一、三塁",
    "一・三塁",
    "二、三塁",
    "二・三塁",
    "満塁",
];

fn document_context_ends_with_honorific_prefix(left_context: &str) -> bool {
    match left_context.chars().next_back() {
        Some('お' | 'ご') => true,
        Some('御') => !left_context.chars().rev().nth(1).is_some_and(|character| {
            matches!(
                character,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            )
        }),
        _ => false,
    }
}

fn document_context_ends_with_quotation_case(left_context: &str) -> bool {
    left_context
        .strip_suffix('と')
        .and_then(|prefix| prefix.chars().next_back())
        .is_some_and(|character| matches!(character, '、' | '，' | ',' | '」' | '』' | '”' | '"'))
}

fn is_reporting_verb_surface(surface: &str) -> bool {
    matches!(
        surface,
        "言っ" | "言い" | "言わ" | "伝え" | "話し" | "述べ" | "答え"
    )
}

fn document_region_suffix_promotion(reading: &str, surface: &str) -> i32 {
    match (reading, surface) {
        ("し", "市")
        | ("く", "区")
        | ("けん", "県")
        | ("ぐん", "郡")
        | ("ちょう" | "まち", "町")
        | ("そん" | "むら", "村") => DOCUMENT_REGION_ADMINISTRATIVE_SUFFIX_PROMOTION,
        ("せん", "線") => DOCUMENT_GRAMMATICAL_PHRASE_PROMOTION,
        _ => 0,
    }
}

fn is_document_region_suffix_reading(reading: &str) -> bool {
    matches!(
        reading,
        "し" | "く" | "けん" | "ぐん" | "ちょう" | "まち" | "そん" | "むら" | "せん"
    )
}

fn structured_notation_matches(left_context: &str, reading: &str, surface: &str) -> bool {
    structured_notation_surface(left_context, reading).is_some_and(|preferred| preferred == surface)
}

fn left_document_numeric_style(left_context: &str) -> Option<DocumentNumericStyle> {
    left_context
        .chars()
        .rev()
        .take(32)
        .take_while(|character| !matches!(character, '。' | '！' | '？' | '\n' | '\r'))
        .find_map(|character| match character {
            '0'..='9' => Some(DocumentNumericStyle::Ascii),
            '０'..='９' => Some(DocumentNumericStyle::Fullwidth),
            _ => None,
        })
}

fn right_document_numeric_style(right_context: &str) -> Option<DocumentNumericStyle> {
    right_context
        .chars()
        .take(32)
        .take_while(|character| !matches!(character, '。' | '！' | '？' | '\n' | '\r'))
        .find_map(|character| match character {
            '0'..='9' => Some(DocumentNumericStyle::Ascii),
            '０'..='９' => Some(DocumentNumericStyle::Fullwidth),
            _ => None,
        })
}

fn reconcile_document_numeric_style(
    left: Option<DocumentNumericStyle>,
    right: Option<DocumentNumericStyle>,
) -> Option<DocumentNumericStyle> {
    match (left, right) {
        (Some(left), Some(right)) if left != right => None,
        (Some(style), _) | (_, Some(style)) => Some(style),
        (None, None) => None,
    }
}

fn document_measurement_abbreviation_style(
    left_context: &str,
    reading: &str,
) -> Option<DocumentNumericStyle> {
    if !MEASUREMENT_ABBREVIATIONS
        .iter()
        .any(|(unit_reading, _, _, _)| reading.contains(unit_reading))
    {
        return None;
    }
    let recent_start = left_context
        .char_indices()
        .rev()
        .take(32)
        .take_while(|(_, character)| !matches!(character, '。' | '！' | '？' | '\n' | '\r'))
        .last()
        .map_or(left_context.len(), |(index, _)| index);
    let recent = &left_context[recent_start..];
    let ascii = contains_numeric_measurement_abbreviation(
        recent,
        &["km", "cm", "mm", "m"],
        DocumentNumericStyle::Ascii,
    );
    let fullwidth = contains_numeric_measurement_abbreviation(
        recent,
        &["ｋｍ", "ｃｍ", "ｍｍ", "ｍ"],
        DocumentNumericStyle::Fullwidth,
    );
    match (ascii, fullwidth) {
        (true, false) => Some(DocumentNumericStyle::Ascii),
        (false, true) => Some(DocumentNumericStyle::Fullwidth),
        _ => None,
    }
}

fn contains_numeric_measurement_abbreviation(
    text: &str,
    units: &[&str],
    style: DocumentNumericStyle,
) -> bool {
    units.iter().any(|unit| {
        text.match_indices(unit).any(|(index, _)| {
            let has_matching_digit =
                text[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|character| match style {
                        DocumentNumericStyle::Ascii => character.is_ascii_digit(),
                        DocumentNumericStyle::Fullwidth => matches!(character, '０'..='９'),
                    });
            let has_boundary = text[index + unit.len()..]
                .chars()
                .next()
                .is_none_or(|character| {
                    !matches!(
                        character,
                        '0'..='9'
                            | 'A'..='Z'
                            | 'a'..='z'
                            | '０'..='９'
                            | 'Ａ'..='Ｚ'
                            | 'ａ'..='ｚ'
                    )
                });
            has_matching_digit && has_boundary
        })
    })
}

fn conversion_matches_measurement_abbreviation(
    conversion: &Conversion,
    style: DocumentNumericStyle,
) -> bool {
    conversion.segments.iter().any(|segment| {
        let matches_unit = match style {
            DocumentNumericStyle::Ascii => ["km", "cm", "mm", "m"]
                .iter()
                .any(|unit| segment.surface.ends_with(unit)),
            DocumentNumericStyle::Fullwidth => ["ｋｍ", "ｃｍ", "ｍｍ", "ｍ"]
                .iter()
                .any(|unit| segment.surface.ends_with(unit)),
        };
        matches_unit
            && segment
                .surface
                .chars()
                .next()
                .is_some_and(|character| match style {
                    DocumentNumericStyle::Ascii => character.is_ascii_digit(),
                    DocumentNumericStyle::Fullwidth => matches!(character, '０'..='９'),
                })
            && MEASUREMENT_ABBREVIATIONS
                .iter()
                .any(|(reading, _, _, _)| segment.reading.ends_with(reading))
    })
}

fn document_numeric_style_evidence(
    left_context: &str,
    reading: &str,
    right_context: &str,
) -> Option<DocumentNumericStyleEvidence> {
    let left_style = left_document_numeric_style(left_context);
    let right_style = ["いっ", "ろっ", "はっ", "ふたり"]
        .iter()
        .any(|numeric_reading| reading.contains(numeric_reading))
        .then(|| right_document_numeric_style(right_context))
        .flatten();
    let style = reconcile_document_numeric_style(left_style, right_style)?;
    let leading = left_style
        .filter(|left_style| *left_style == style)
        .and_then(|_| {
            [
                ("きゅー", '九'),
                ("きゅう", '九'),
                ("ぜろ", '〇'),
                ("れい", '〇'),
                ("いち", '一'),
                ("さん", '三'),
                ("よん", '四'),
                ("なな", '七'),
                ("しち", '七'),
                ("はち", '八'),
                ("ろく", '六'),
            ]
            .into_iter()
            .find(|(numeric_reading, _)| reading.starts_with(numeric_reading))
            .map(|(numeric_reading, kanji)| DocumentLeadingNumericStyle {
                reading_bytes: numeric_reading.len(),
                kanji,
            })
        });
    Some(DocumentNumericStyleEvidence { style, leading })
}

fn conversion_matches_numeric_style(
    dictionary: &Dictionary,
    conversion: &Conversion,
    evidence: DocumentNumericStyleEvidence,
) -> bool {
    if conversion
        .segments
        .iter()
        .any(|segment| assimilated_counter_surface_matches_style(segment, evidence.style))
    {
        return true;
    }
    let Some(leading) = evidence.leading else {
        return false;
    };
    let [numeric, following, ..] = conversion.segments.as_slice() else {
        return false;
    };
    {
        if numeric.reading.len() != leading.reading_bytes {
            return false;
        }
        let matching_width = !numeric.surface.is_empty()
            && numeric
                .surface
                .chars()
                .all(|character| match evidence.style {
                    DocumentNumericStyle::Ascii => character.is_ascii_digit(),
                    DocumentNumericStyle::Fullwidth => matches!(character, '０'..='９'),
                });
        if !matching_width {
            return false;
        }
        if !is_productive_numeric_style_suffix(&following.surface) {
            return false;
        }

        // Do not promote every parseable number merely because a digit appeared
        // earlier in the sentence. Only a leading single digit may inherit the
        // style of an established kanji compound with a productive numeric
        // suffix. This admits 3+塁 for 三塁 while leaving lexical numeric words
        // such as 一部 and 九州, as well as 仙台 and 前立腺, unchanged.
        let mut compound_reading = String::with_capacity(
            numeric
                .reading
                .len()
                .saturating_add(following.reading.len()),
        );
        compound_reading.push_str(&numeric.reading);
        compound_reading.push_str(&following.reading);
        let mut compound_surface = String::with_capacity(
            leading
                .kanji
                .len_utf8()
                .saturating_add(following.surface.len()),
        );
        compound_surface.push(leading.kanji);
        compound_surface.push_str(&following.surface);
        dictionary.has_exact_entry(&compound_reading, &compound_surface)
    }
}

fn assimilated_counter_surface_matches_style(
    segment: &Segment,
    style: DocumentNumericStyle,
) -> bool {
    if segment.reading == "ふたり" {
        return match style {
            DocumentNumericStyle::Ascii => segment.surface == "2人",
            DocumentNumericStyle::Fullwidth => segment.surface == "２人",
        };
    }
    let Some((_, counter_reading)) = assimilated_numeric_prefix(&segment.reading) else {
        return false;
    };
    if !ASSIMILATED_NUMERIC_COUNTER_READINGS.contains(&counter_reading) {
        return false;
    }
    let surface = segment.surface.as_str();
    let mut saw_digit = false;
    let mut suffix_start = None;
    for (index, character) in surface.char_indices() {
        let matching_digit = match style {
            DocumentNumericStyle::Ascii => character.is_ascii_digit(),
            DocumentNumericStyle::Fullwidth => matches!(character, '０'..='９'),
        };
        if matching_digit {
            saw_digit = true;
        } else {
            suffix_start = Some(index);
            break;
        }
    }
    saw_digit
        && suffix_start
            .and_then(|start| surface.get(start..))
            .is_some_and(is_assimilated_numeric_suffix)
}

fn is_productive_numeric_style_suffix(surface: &str) -> bool {
    matches!(
        surface,
        "人" | "個"
            | "回"
            | "件"
            | "本"
            | "枚"
            | "台"
            | "匹"
            | "冊"
            | "杯"
            | "階"
            | "歳"
            | "年"
            | "月"
            | "日"
            | "時"
            | "分"
            | "秒"
            | "円"
            | "番"
            | "号"
            | "位"
            | "点"
            | "塁"
            | "段"
            | "対"
            | "組"
            | "校"
            | "社"
            | "戸"
            | "軒"
            | "票"
            | "席"
            | "戦"
            | "勝"
            | "敗"
    )
}

fn is_assimilated_numeric_suffix(surface: &str) -> bool {
    is_productive_numeric_style_suffix(surface)
        || matches!(
            surface,
            "版" | "隻"
                | "足"
                | "着"
                | "頭"
                | "棟"
                | "局"
                | "区"
                | "丁"
                | "通"
                | "期"
                | "カ国"
                | "か国"
                | "ヶ国"
                | "ヵ国"
                | "ケ国"
                | "箇所"
                | "カ所"
                | "か所"
                | "ヶ所"
                | "ヵ所"
                | "ケ所"
        )
}

fn structured_notation_context_matches(left_context: &str, reading: &str) -> bool {
    structured_notation_surface(left_context, reading).is_some()
}

fn structured_notation_surface(left_context: &str, reading: &str) -> Option<&'static str> {
    match reading {
        "しん" if radix_suffix_matches(left_context) => Some("進"),
        "か" | "にち" if calendar_day_suffix_matches(left_context, reading) => Some("日"),
        "さい" if trailing_integer(left_context).is_some_and(|age| age <= 150) => Some("歳"),
        "かこく" if trailing_integer(left_context).is_some_and(|countries| countries <= 999) => {
            Some("カ国")
        }
        "だい" => structured_dai_surface(left_context),
        "ひき" if trailing_counter_integer(left_context) => Some("匹"),
        "つい" if trailing_counter_integer(left_context) => Some("対"),
        "はい" if trailing_win_loss_record(left_context) => Some("敗"),
        "せん" if trailing_fractional_yen(left_context) => Some("銭"),
        "げん" if trailing_percentage(left_context) => Some("減"),
        "ぞう" if trailing_percentage(left_context) => Some("増"),
        _ => None,
    }
}

fn structured_dai_surface(left_context: &str) -> Option<&'static str> {
    let (prefix, _) = split_trailing_decimal(left_context)?;
    if prefix.ends_with('第') {
        Some("代")
    } else if matches!(prefix.chars().next_back(), Some('.' | '．' | '-' | '−')) {
        None
    } else {
        Some("台")
    }
}

fn trailing_counter_integer(left_context: &str) -> bool {
    strip_trailing_counter_integer(left_context).is_some()
}

fn trailing_win_loss_record(left_context: &str) -> bool {
    let Some(prefix) = strip_trailing_counter_integer(left_context) else {
        return false;
    };
    prefix
        .strip_suffix('勝')
        .is_some_and(trailing_counter_integer)
}

fn trailing_fractional_yen(left_context: &str) -> bool {
    let Some((prefix, fraction)) = split_trailing_decimal(left_context) else {
        return false;
    };
    fraction <= 99
        && prefix
            .strip_suffix('円')
            .and_then(trailing_numeric_surface)
            .is_some()
}

fn strip_trailing_counter_integer(text: &str) -> Option<&str> {
    if let Some((prefix, _)) = split_trailing_decimal(text) {
        return (!matches!(prefix.chars().next_back(), Some('.' | '．' | '-' | '−')))
            .then_some(prefix);
    }
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_japanese_numeric_character(*character))
        .last()
        .map(|(index, _)| index)?;
    Some(&text[..start])
}

fn trailing_percentage(left_context: &str) -> bool {
    left_context
        .strip_suffix('%')
        .or_else(|| left_context.strip_suffix('％'))
        .is_some_and(has_trailing_numeric_surface)
}

fn has_trailing_numeric_surface(text: &str) -> bool {
    if let Some((prefix, _)) = split_trailing_decimal(text) {
        return if let Some(integer_prefix) = prefix
            .strip_suffix('.')
            .or_else(|| prefix.strip_suffix('．'))
        {
            split_trailing_decimal(integer_prefix)
                .is_some_and(|(prefix, _)| !matches!(prefix.chars().next_back(), Some('-' | '−')))
        } else {
            !matches!(prefix.chars().next_back(), Some('-' | '−'))
        };
    }
    text.chars()
        .next_back()
        .is_some_and(is_japanese_numeric_character)
}

fn surrounding_structured_notation(
    left_context: &str,
    reading: &str,
    right_context: &str,
) -> Option<(&'static str, i32)> {
    if reading == "たい"
        && (trailing_integer(left_context).is_some()
            || left_context
                .chars()
                .next_back()
                .is_some_and(is_japanese_numeric_character))
        && starts_with_score_integer(right_context)
    {
        return Some(("対", DOCUMENT_STRUCTURED_NOTATION_PROMOTION));
    }
    if reading == "けん"
        && (right_context.starts_with('内') || right_context.starts_with('外'))
        && trailing_reach_measurement(left_context)
    {
        return Some(("圏", DOCUMENT_STRUCTURED_NOTATION_PROMOTION));
    }
    if reading == "き"
        && right_context.starts_with("終了")
        && trailing_ordinal_integer(left_context)
    {
        return Some(("期", DOCUMENT_STRUCTURED_NOTATION_PROMOTION));
    }
    if reading == "し"
        && trailing_integer(left_context).is_some_and(|outs| outs <= 2)
        && BASE_STATE_PREFIXES
            .iter()
            .any(|prefix| right_context.starts_with(prefix))
    {
        return Some(("死", DOCUMENT_STRONG_STRUCTURED_NOTATION_PROMOTION));
    }
    if reading == "げん"
        && trailing_counter_integer(left_context)
        && STRINGED_INSTRUMENT_PREFIXES
            .iter()
            .any(|prefix| right_context.starts_with(prefix))
    {
        return Some(("弦", DOCUMENT_STRONG_STRUCTURED_NOTATION_PROMOTION));
    }
    if reading == "せき"
        && trailing_counter_integer(left_context)
        && VESSEL_NOUN_PREFIXES
            .iter()
            .any(|prefix| right_context.starts_with(prefix))
    {
        return Some(("隻", DOCUMENT_STRONG_STRUCTURED_NOTATION_PROMOTION));
    }
    None
}

fn starts_with_score_integer(text: &str) -> bool {
    let mut characters = text.chars().peekable();
    let Some(first) = characters.peek().copied() else {
        return false;
    };
    if decimal_digit(first).is_some() {
        while characters
            .peek()
            .is_some_and(|character| decimal_digit(*character).is_some())
        {
            characters.next();
        }
    } else if is_japanese_numeric_character(first) {
        while characters
            .peek()
            .is_some_and(|character| is_japanese_numeric_character(*character))
        {
            characters.next();
        }
    } else {
        return false;
    }
    !matches!(characters.next(), Some('.' | '．'))
}

fn trailing_ordinal_integer(text: &str) -> bool {
    strip_trailing_counter_integer(text).is_some_and(|prefix| prefix.ends_with('第'))
}

fn trailing_reach_measurement(text: &str) -> bool {
    [
        "キロメートル",
        "センチメートル",
        "ミリメートル",
        "メートル",
        "時間",
        "キロ",
        "ｋｍ",
        "ＫＭ",
        "km",
        "KM",
        "㎞",
        "ｍ",
        "m",
        "駅",
        "秒",
        "分",
    ]
    .iter()
    .find_map(|unit| text.strip_suffix(unit))
    .and_then(trailing_numeric_surface)
    .is_some()
}

fn structured_notation_owns_numeric_context(left_context: &str, reading: &str) -> bool {
    structured_notation_context_matches(left_context, reading)
        || (split_trailing_decimal(left_context).is_some()
            && matches!(reading, "しん" | "か" | "にち" | "さい" | "かこく" | "だい"))
}

fn radix_suffix_matches(left_context: &str) -> bool {
    trailing_integer(left_context).is_some_and(|radix| COMMON_RADICES.contains(&radix))
}

fn trailing_integer(text: &str) -> Option<u32> {
    let (prefix, value) = split_trailing_decimal(text)?;
    (!matches!(prefix.chars().next_back(), Some('.' | '．' | '-' | '−'))).then_some(value)
}

fn trailing_numeric_surface(text: &str) -> Option<CompactString> {
    if let Some((prefix, value)) = split_trailing_decimal(text) {
        let valid = if matches!(prefix.chars().next_back(), Some('.' | '．')) {
            prefix
                .strip_suffix('.')
                .or_else(|| prefix.strip_suffix('．'))
                .and_then(split_trailing_decimal)
                .is_some_and(|(prefix, _)| !matches!(prefix.chars().next_back(), Some('-' | '−')))
        } else {
            !matches!(prefix.chars().next_back(), Some('-' | '−'))
        };
        return valid.then(|| japanese_number_surface(value)).flatten();
    }

    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_japanese_numeric_character(*character))
        .last()
        .map(|(index, _)| index)?;
    Some(CompactString::from(&text[start..]))
}

fn is_japanese_numeric_character(character: char) -> bool {
    matches!(
        character,
        '〇' | '零'
            | '一'
            | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
            | '億'
            | '兆'
    )
}

fn japanese_number_surface(value: u32) -> Option<CompactString> {
    if value > 9_999 {
        return None;
    }
    if value == 0 {
        return Some(CompactString::from("〇"));
    }

    let mut surface = CompactString::new("");
    let mut remainder = value;
    for (unit_value, unit_surface) in [(1_000, '千'), (100, '百'), (10, '十')] {
        let digit = remainder / unit_value;
        if digit > 0 {
            if digit > 1 {
                surface.push(japanese_digit_surface(digit)?);
            }
            surface.push(unit_surface);
            remainder %= unit_value;
        }
    }
    if remainder > 0 {
        surface.push(japanese_digit_surface(remainder)?);
    }
    Some(surface)
}

fn japanese_digit_surface(digit: u32) -> Option<char> {
    match digit {
        1 => Some('一'),
        2 => Some('二'),
        3 => Some('三'),
        4 => Some('四'),
        5 => Some('五'),
        6 => Some('六'),
        7 => Some('七'),
        8 => Some('八'),
        9 => Some('九'),
        _ => None,
    }
}

fn calendar_day_suffix_matches(left_context: &str, reading: &str) -> bool {
    let Some((before_day, day)) = split_trailing_decimal(left_context) else {
        return false;
    };
    let Some(before_month) = before_day.strip_suffix('月') else {
        return false;
    };
    let Some((_, month)) = split_trailing_decimal(before_month) else {
        return false;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }

    match reading {
        "か" => CALENDAR_KA_ENDING_DAYS.contains(&day),
        "にち" => day != 1 && !CALENDAR_KA_ENDING_DAYS.contains(&day),
        _ => false,
    }
}

fn split_trailing_decimal(text: &str) -> Option<(&str, u32)> {
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        if decimal_digit(character).is_none() {
            break;
        }
        start = index;
    }
    if start == text.len() {
        return None;
    }
    let value = text[start..].chars().try_fold(0_u32, |value, character| {
        value
            .checked_mul(10)?
            .checked_add(decimal_digit(character)?)
    })?;
    Some((&text[..start], value))
}

fn decimal_digit(character: char) -> Option<u32> {
    character.to_digit(10).or_else(|| {
        ('０'..='９')
            .contains(&character)
            .then(|| u32::from(character) - u32::from('０'))
    })
}

fn retain_best_candidates(candidates: &mut Vec<Candidate>, limit: usize) {
    candidates.sort_unstable_by(|left, right| {
        left.surface
            .cmp(&right.surface)
            .then_with(|| left.cost.cmp(&right.cost))
    });
    candidates.dedup_by(|left, right| left.surface == right.surface);
    candidates.sort_unstable_by(|left, right| {
        left.cost
            .cmp(&right.cost)
            .then_with(|| left.surface.cmp(&right.surface))
    });
    candidates.truncate(limit);
}

fn is_full_katakana_surface(surface: &str) -> bool {
    !surface.is_empty()
        && surface
            .chars()
            .all(|character| matches!(character, '\u{30a0}'..='\u{30ff}'))
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

fn reading_has_roman_numeral_suffix(reading: &str) -> bool {
    if !reading.contains('の') {
        return false;
    }
    [
        "じゅういちの",
        "じゅうにの",
        "じゅうの",
        "きゅうの",
        "よんの",
        "しちの",
        "いちの",
        "さんの",
        "ろくの",
        "ななの",
        "はちの",
        "にの",
        "しの",
        "ごの",
        "くの",
    ]
    .iter()
    .any(|suffix| reading.contains(suffix))
}

fn should_expand_alphanumeric_numeric_compound(
    reading: &str,
    left_context: &str,
    right_context: &str,
) -> bool {
    if reading.chars().count() > 8
        || !left_context.chars().next_back().is_some_and(
            |character| matches!(character, 'A'..='Z' | 'a'..='z' | 'Ａ'..='Ｚ' | 'ａ'..='ｚ'),
        )
        || !right_context
            .chars()
            .next()
            .is_some_and(is_ideographic_or_digit)
    {
        return false;
    }
    parse_kana_number_prefixes(reading)
        .iter()
        .any(|(length, _)| *length < reading.len())
}

fn should_expand_numeric_particle_suru(reading: &str, right_context: &str) -> bool {
    if reading.chars().count() > 12 || !starts_with_suru_inflection(right_context) {
        return false;
    }
    reading.char_indices().any(|(start, _)| {
        let suffix = &reading[start..];
        parse_kana_number_prefixes(suffix)
            .iter()
            .any(|(numeric_length, _)| {
                let numeric_reading = &suffix[..*numeric_length];
                numeric_reading != "さん"
                    && suffix[*numeric_length..]
                        .strip_prefix('に')
                        .is_some_and(|verbal_noun| !verbal_noun.is_empty())
            })
    })
}

fn is_hiragana_character(character: char) -> bool {
    matches!(character, '\u{3040}'..='\u{309f}')
}

fn trim_compound_paths(
    paths: &mut Vec<CompoundPath>,
    limit: usize,
    preserve_pronunciation_shapes: bool,
) {
    if !preserve_pronunciation_shapes {
        paths.sort_unstable_by(|left, right| {
            left.surface
                .cmp(&right.surface)
                .then_with(|| left.right_id.cmp(&right.right_id))
                .then_with(|| left.cost.cmp(&right.cost))
        });
        paths.dedup_by(|left, right| {
            left.surface == right.surface && left.right_id == right.right_id
        });
        paths.sort_unstable_by(|left, right| {
            left.cost
                .cmp(&right.cost)
                .then_with(|| left.surface.cmp(&right.surface))
                .then_with(|| left.right_id.cmp(&right.right_id))
        });
        paths.truncate(limit);
        return;
    }
    paths.sort_unstable_by(|left, right| {
        left.surface
            .cmp(&right.surface)
            .then_with(|| left.right_id.cmp(&right.right_id))
            .then_with(|| left.segment_count.cmp(&right.segment_count))
            .then_with(|| left.substituted_segments.cmp(&right.substituted_segments))
            .then_with(|| left.katakana_segments.cmp(&right.katakana_segments))
            .then_with(|| left.ideographic_segments.cmp(&right.ideographic_segments))
            .then_with(|| left.cost.cmp(&right.cost))
    });
    paths.dedup_by(|left, right| {
        left.surface == right.surface
            && left.right_id == right.right_id
            && left.segment_count == right.segment_count
            && left.substituted_segments == right.substituted_segments
            && left.katakana_segments == right.katakana_segments
            && left.ideographic_segments == right.ideographic_segments
    });
    paths.sort_unstable_by(|left, right| {
        left.cost
            .cmp(&right.cost)
            .then_with(|| left.surface.cmp(&right.surface))
            .then_with(|| left.right_id.cmp(&right.right_id))
    });
    paths.truncate(limit);
}

fn extend_compound_path(
    path: &CompoundPath,
    entry: EntryView<'_>,
    segment_index: usize,
    segment_start: usize,
    segment_end: usize,
    substituted_offsets: &[usize],
    transition_cost: i32,
) -> CompoundPath {
    let mut surface = String::with_capacity(path.surface.len().saturating_add(entry.surface.len()));
    surface.push_str(&path.surface);
    surface.push_str(entry.surface);
    let segment_bit = 1_u8 << segment_index;
    let substituted = substituted_offsets
        .iter()
        .any(|offset| (segment_start..segment_end).contains(offset));
    CompoundPath {
        surface,
        cost: path
            .cost
            .saturating_add(transition_cost)
            .saturating_add(entry.word_cost),
        right_id: entry.right_id,
        segment_count: path.segment_count + 1,
        substituted_segments: path.substituted_segments | if substituted { segment_bit } else { 0 },
        katakana_segments: path.katakana_segments
            | if is_full_katakana_surface(entry.surface) {
                segment_bit
            } else {
                0
            },
        ideographic_segments: path.ideographic_segments
            | if entry.surface.chars().any(is_ideographic_or_digit) {
                segment_bit
            } else {
                0
            },
    }
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
            katakana_run_character_cost: katakana_run_character_cost(),
        }
    }

    #[must_use]
    pub fn bundled() -> Self {
        Self {
            bundled: Some(CompactDictionary::bundled()),
            layers: Vec::new().into(),
            uses_connection_costs: true,
            katakana_run_character_cost: katakana_run_character_cost(),
        }
    }

    #[must_use]
    pub fn bundled_with_layers(additional_layers: Vec<DictionaryLayer>) -> Self {
        Self {
            bundled: Some(CompactDictionary::bundled()),
            layers: additional_layers.into(),
            uses_connection_costs: true,
            katakana_run_character_cost: katakana_run_character_cost(),
        }
    }

    /// Returns a cheap clone whose wider N-best search recalls unknown
    /// katakana runs for optional model scoring. The ordinary dictionary keeps
    /// the conservative production cost, so enabling no model cannot change
    /// the visible deterministic winner.
    #[must_use]
    pub fn with_model_recall_katakana_cost(&self) -> Self {
        let mut dictionary = self.clone();
        dictionary.katakana_run_character_cost = dictionary
            .katakana_run_character_cost
            .min(MODEL_RECALL_KATAKANA_RUN_CHARACTER_COST);
        dictionary
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

    /// Returns whether an exact reading-surface pair is already present.
    ///
    /// Supplemental dictionary builders use this to avoid reintroducing base
    /// entries with different costs, which could alter unrelated lattice paths
    /// even when the rendered word is not new vocabulary.
    #[must_use]
    pub fn has_exact_entry(&self, reading: &str, surface: &str) -> bool {
        let mut found = false;
        self.for_each_exact(reading, |entry| {
            found |= entry.surface == surface;
        });
        found
    }

    /// Reports whether an exact reading and surface pair carries Mozc's region
    /// proper-noun POS. Evaluation tools use this to reject tokenizer splits
    /// that accidentally reinterpret pieces of an administrative place name.
    #[must_use]
    pub fn has_exact_region_surface(&self, reading: &str, surface: &str) -> bool {
        let mut found = false;
        self.for_each_exact(reading, |entry| {
            found |= entry.surface == surface
                && (MOZC_REGION_POS_IDS.contains(&entry.left_id)
                    || MOZC_REGION_POS_IDS.contains(&entry.right_id));
        });
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
        let mut candidates =
            self.compound_candidates_exact(reading, entries_per_segment, limit, &[]);
        let long_mark_count = reading
            .chars()
            .filter(|character| *character == 'ー')
            .count();
        let mut variants = orthographic_long_vowel_variants(reading);
        variants.sort_by_key(|variant| {
            (
                variant.secondary_options > 0,
                variant.substituted_offsets.len() != long_mark_count,
                variant.substituted_offsets.len(),
            )
        });
        for variant in variants.into_iter().take(LONG_VOWEL_COMPOUND_MAX_VARIANTS) {
            candidates.extend(self.compound_candidates_exact(
                &variant.reading,
                entries_per_segment,
                limit.min(LONG_VOWEL_COMPOUND_CANDIDATES_PER_VARIANT),
                &variant.substituted_offsets,
            ));
        }
        retain_best_candidates(&mut candidates, limit.min(COMPOUND_MAX_CANDIDATES));
        candidates
    }

    fn compound_candidates_exact(
        &self,
        reading: &str,
        entries_per_segment: usize,
        limit: usize,
        substituted_offsets: &[usize],
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
            segment_count: 0,
            substituted_segments: 0,
            katakana_segments: 0,
            ideographic_segments: 0,
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
                            let transition_cost = connection
                                .map_or(0, |matrix| matrix.cost(path.right_id, entry.left_id));
                            destination.push(extend_compound_path(
                                path,
                                *entry,
                                segment_count,
                                boundaries[start_position],
                                boundaries[end_position],
                                substituted_offsets,
                                transition_cost,
                            ));
                        }
                    }
                    trim_compound_paths(destination, state_limit, !substituted_offsets.is_empty());
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
                if path.surface == reading || !path.pronunciation_safe() {
                    continue;
                }
                let cost = path
                    .cost
                    .saturating_add(
                        connection.map_or(0, |matrix| matrix.cost(path.right_id, BOS_EOS_POS_ID)),
                    )
                    .saturating_add(long_vowel_substitution_cost(substituted_offsets.len()));
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

    /// Returns bounded personal-name alternatives for an explicit candidate
    /// expansion. Two-part surname + given-name paths are composed from exact
    /// dictionary entries in the corresponding personal-name POS classes,
    /// independently of their rank in the general N-best search.
    ///
    /// This intentionally stays outside normal and live conversion. Personal
    /// names have many legitimate spellings for one reading, so promoting the
    /// wider set without an explicit user action would create false top-1
    /// changes.
    #[must_use]
    pub fn personal_name_candidates(
        &self,
        reading: &str,
        entries_per_part: usize,
        limit: usize,
    ) -> Vec<Candidate> {
        let character_count = reading.chars().count();
        if entries_per_part == 0
            || limit == 0
            || !(PERSONAL_NAME_MIN_READING_CHARACTERS..=PERSONAL_NAME_MAX_READING_CHARACTERS)
                .contains(&character_count)
        {
            return Vec::new();
        }

        let entries_per_part = entries_per_part.min(PERSONAL_NAME_MAX_ENTRIES_PER_PART);
        let limit = limit.min(PERSONAL_NAME_MAX_CANDIDATES);
        let connection = self.uses_connection_costs.then(ConnectionMatrix::bundled);
        let mut candidates = Vec::new();

        for (boundary, _) in reading.char_indices().skip(1) {
            let surname_reading = &reading[..boundary];
            let given_name_reading = &reading[boundary..];
            let surnames = self.exact_personal_name_entries(
                surname_reading,
                PersonalNameRole::Surname,
                entries_per_part,
            );
            if surnames.is_empty() {
                continue;
            }
            let given_names = self.exact_personal_name_entries(
                given_name_reading,
                PersonalNameRole::GivenName,
                entries_per_part,
            );
            for surname in &surnames {
                for given_name in &given_names {
                    let mut surface = String::with_capacity(
                        surname
                            .surface
                            .len()
                            .saturating_add(given_name.surface.len()),
                    );
                    surface.push_str(surname.surface);
                    surface.push_str(given_name.surface);
                    let cost =
                        connection
                            .map_or(0, |matrix| matrix.cost(BOS_EOS_POS_ID, surname.left_id))
                            .saturating_add(surname.word_cost)
                            .saturating_add(connection.map_or(0, |matrix| {
                                matrix.cost(surname.right_id, given_name.left_id)
                            }))
                            .saturating_add(given_name.word_cost)
                            .saturating_add(connection.map_or(0, |matrix| {
                                matrix.cost(given_name.right_id, BOS_EOS_POS_ID)
                            }));
                    candidates.push(Candidate { surface, cost });
                }
            }
            // Once a surface falls below the global output bound it cannot
            // return to the final top set; a cheaper duplicate at a later
            // boundary is inserted again and considered normally.
            retain_best_candidates(&mut candidates, limit);
        }

        candidates
    }

    /// Returns whether a surface substitution changes a dictionary-confirmed
    /// personal-name segment in the exact path for `current_surface`.
    ///
    /// Model-directed local corrections use this to preserve an already valid
    /// name spelling while still allowing corrections elsewhere in the same
    /// sentence. The check is deliberately limited to exact Mozc surname/name
    /// POS entries; ordinary nouns and unknown strings are unaffected.
    #[must_use]
    pub fn changes_exact_personal_name_segment(
        &self,
        reading: &str,
        current_surface: &str,
        alternative_surface: &str,
    ) -> bool {
        self.changes_exact_named_segment(reading, current_surface, alternative_surface, true, false)
    }

    /// Returns whether the whole surface is one exact full personal name.
    ///
    /// A surname or given name alone is deliberately insufficient. The exact
    /// conversion path must either contain one full-name entry or an adjacent
    /// surname and given-name pair with no surrounding ordinary segments.
    #[must_use]
    pub fn is_exact_full_personal_name_surface(&self, reading: &str, surface: &str) -> bool {
        let Some(conversion) = self
            .convert_n_best_with_surface_prefix(reading, surface, 1)
            .into_iter()
            .find(|conversion| conversion.surface == surface)
        else {
            return false;
        };
        match conversion.segments.as_slice() {
            [name] => {
                self.exact_personal_name_roles(&name.reading, &name.surface)
                    .full_name
            }
            [surname, given_name] => {
                self.exact_personal_name_roles(&surname.reading, &surname.surface)
                    .surname
                    && self
                        .exact_personal_name_roles(&given_name.reading, &given_name.surface)
                        .given_name
            }
            _ => false,
        }
    }

    /// Returns whether a surface substitution changes a dictionary-confirmed
    /// personal-name or sufficiently specific region segment in the exact path
    /// for `current_surface`.
    ///
    /// Region protection deliberately requires one unambiguous all-kanji
    /// surface of at least three characters. Short or competing geographic
    /// spellings remain available to contextual ranking, while an established
    /// specific place spelling is not decomposed by an external model.
    #[must_use]
    pub fn changes_exact_personal_name_or_region_segment(
        &self,
        reading: &str,
        current_surface: &str,
        alternative_surface: &str,
    ) -> bool {
        self.changes_exact_named_segment(reading, current_surface, alternative_surface, true, true)
    }

    /// Returns whether a substitution changes one unambiguous specific region
    /// segment in the exact path for `current_surface`.
    #[must_use]
    pub fn changes_exact_region_segment(
        &self,
        reading: &str,
        current_surface: &str,
        alternative_surface: &str,
    ) -> bool {
        self.changes_exact_named_segment(reading, current_surface, alternative_surface, false, true)
    }

    /// Returns whether a model substitution fragments one exact all-katakana
    /// dictionary segment into a same-length hiragana/katakana mixture.
    ///
    /// A complete hiragana spelling remains available for intentional script
    /// changes. This only rejects artifacts such as `アルゴル -> あるゴル`,
    /// where the model splits an established katakana word between scripts.
    #[must_use]
    pub fn fragments_exact_katakana_segment(
        &self,
        reading: &str,
        current_surface: &str,
        alternative_surface: &str,
    ) -> bool {
        let current_characters = current_surface.chars().collect::<Vec<_>>();
        let alternative_characters = alternative_surface.chars().collect::<Vec<_>>();
        if current_characters.len() != alternative_characters.len()
            || current_characters == alternative_characters
        {
            return false;
        }
        let Some(first_change) = current_characters
            .iter()
            .zip(&alternative_characters)
            .position(|(current, alternative)| current != alternative)
        else {
            return false;
        };
        let last_change = current_characters
            .iter()
            .zip(&alternative_characters)
            .rposition(|(current, alternative)| current != alternative)
            .unwrap_or(first_change);
        let plausible_fragment = current_characters
            .split_inclusive(|character| !matches!(character, 'ァ'..='ヿ'))
            .scan(0usize, |surface_start, run| {
                let start = *surface_start;
                *surface_start += run.len();
                Some((start, run))
            })
            .any(|(start, run)| {
                let katakana_characters = run
                    .iter()
                    .take_while(|character| matches!(character, 'ァ'..='ヿ'))
                    .count();
                let end = start + katakana_characters;
                katakana_characters >= 4
                    && start <= first_change
                    && last_change < end
                    && alternative_characters[start..end]
                        .iter()
                        .any(|character| matches!(character, 'ぁ'..='ゖ'))
                    && alternative_characters[start..end]
                        .iter()
                        .any(|character| matches!(character, 'ァ'..='ヿ'))
            });
        if !plausible_fragment {
            return false;
        }
        let Some(conversion) = self
            .convert_n_best_with_surface_prefix(reading, current_surface, 1)
            .into_iter()
            .find(|conversion| conversion.surface == current_surface)
        else {
            return false;
        };
        let mut surface_start = 0usize;
        conversion.segments.into_iter().any(|segment| {
            let segment_characters = segment.surface.chars().count();
            let surface_end = surface_start + segment_characters;
            let exact_katakana = segment_characters >= 4
                && segment
                    .surface
                    .chars()
                    .all(|character| matches!(character, 'ァ'..='ヿ'))
                && self.has_exact_entry(&segment.reading, &segment.surface);
            let alternative_segment = &alternative_characters[surface_start..surface_end];
            let fragmented = exact_katakana
                && alternative_segment
                    .iter()
                    .any(|character| matches!(character, 'ぁ'..='ゖ'))
                && alternative_segment
                    .iter()
                    .any(|character| matches!(character, 'ァ'..='ヿ'));
            let changes_only_this_segment = current_characters
                .iter()
                .zip(&alternative_characters)
                .enumerate()
                .filter(|(_, (current, alternative))| current != alternative)
                .all(|(index, _)| (surface_start..surface_end).contains(&index));
            surface_start = surface_end;
            fragmented && changes_only_this_segment
        })
    }

    /// Returns whether an alternative replaces an exact ideographic segment
    /// with a hiragana spelling that is not itself one complete dictionary
    /// entry for the same reading.
    ///
    /// This distinguishes an intentional orthographic alternative such as
    /// `言う -> いう` from fragmentation such as `亡くなっ -> なく + なっ`.
    #[must_use]
    pub fn fragments_exact_ideographic_segment_into_hiragana(
        &self,
        reading: &str,
        current_surface: &str,
        alternative_surface: &str,
    ) -> bool {
        let current_characters = current_surface.chars().collect::<Vec<_>>();
        let alternative_characters = alternative_surface.chars().collect::<Vec<_>>();
        if current_characters.len() != alternative_characters.len()
            || current_characters == alternative_characters
        {
            return false;
        }
        let Some(conversion) = self
            .convert_n_best_with_surface_prefix(reading, current_surface, 1)
            .into_iter()
            .find(|conversion| conversion.surface == current_surface)
        else {
            return false;
        };
        let Some(alternative_conversion) = self
            .convert_n_best_with_surface_prefix(reading, alternative_surface, 1)
            .into_iter()
            .find(|conversion| conversion.surface == alternative_surface)
        else {
            return false;
        };
        let mut alternative_start = 0;
        let alternative_segments = alternative_conversion
            .segments
            .into_iter()
            .map(|segment| {
                let start = alternative_start;
                alternative_start += segment.surface.chars().count();
                (start, alternative_start, segment.reading, segment.surface)
            })
            .collect::<Vec<_>>();
        let mut surface_start = 0;
        conversion.segments.into_iter().any(|segment| {
            let segment_characters = segment.surface.chars().count();
            let surface_end = surface_start + segment_characters;
            let alternative_segment = alternative_characters[surface_start..surface_end]
                .iter()
                .collect::<String>();
            let alternative_is_one_segment = alternative_segments.iter().any(
                |(start, end, alternative_reading, alternative_surface)| {
                    *start == surface_start
                        && *end == surface_end
                        && alternative_reading == &segment.reading
                        && alternative_surface == &alternative_segment
                },
            );
            let alternative_has_exact_ideographic_overlap = alternative_segments.iter().any(
                |(start, end, alternative_reading, alternative_surface)| {
                    *start < surface_end
                        && *end > surface_start
                        && alternative_surface.chars().any(is_ideographic_or_digit)
                        && self.has_exact_entry(alternative_reading, alternative_surface)
                },
            );
            surface_start = surface_end;
            segment.surface.chars().any(is_ideographic_or_digit)
                && alternative_segment.chars().all(is_hiragana_character)
                && self.has_exact_entry(&segment.reading, &segment.surface)
                && !alternative_is_one_segment
                && !alternative_has_exact_ideographic_overlap
        })
    }

    /// Returns whether a bounded suffix of a complete conversion forms an exact
    /// dictionary phrase with the confirmed text to its right.
    #[must_use]
    pub fn has_exact_right_phrase_continuation(
        &self,
        reading: &str,
        surface: &str,
        right_context: &str,
    ) -> bool {
        if right_context.is_empty() {
            return false;
        }
        let Some(compact) = self.bundled else {
            return false;
        };
        let context_end = right_context
            .char_indices()
            .nth(DOCUMENT_RIGHT_PHRASE_MAX_SUFFIX_CHARACTERS)
            .map_or(right_context.len(), |(index, _)| index);
        let context_head = &right_context[..context_end];
        surface
            .char_indices()
            .rev()
            .take(DOCUMENT_RIGHT_CARET_PHRASE_MAX_PREFIX_CHARACTERS)
            .any(|(start, _)| {
                let surface_suffix = &surface[start..];
                surface_suffix.chars().any(is_ideographic_or_digit)
                    && self
                        .readings_for_surface(surface_suffix)
                        .into_iter()
                        .take(FIXED_SEGMENT_MAX_ENTRIES_PER_SEGMENT)
                        .any(|reading_suffix| {
                            if !reading.ends_with(&reading_suffix) {
                                return false;
                            }
                            let mut found = false;
                            compact.for_each_joined_surface_reading_prefix(
                                surface_suffix,
                                context_head,
                                &reading_suffix,
                                DOCUMENT_RIGHT_PHRASE_MAX_SUFFIX_CHARACTERS,
                                |suffix, entry| {
                                    found |= !suffix.is_empty()
                                        && entry.word_cost < DOCUMENT_PHRASE_COST_CEILING
                                        && !matches!(
                                            entry.left_id,
                                            MOZC_PERSONAL_GIVEN_NAME_POS_ID
                                                | MOZC_PERSONAL_SURNAME_POS_ID
                                        )
                                        && !matches!(
                                            entry.right_id,
                                            MOZC_PERSONAL_GIVEN_NAME_POS_ID
                                                | MOZC_PERSONAL_SURNAME_POS_ID
                                        )
                                        && right_phrase_suffix_has_boundary(suffix, right_context);
                                },
                            );
                            found
                        })
            })
    }

    fn changes_exact_named_segment(
        &self,
        reading: &str,
        current_surface: &str,
        alternative_surface: &str,
        protect_personal_names: bool,
        protect_regions: bool,
    ) -> bool {
        let current_characters = current_surface.chars().collect::<Vec<_>>();
        let alternative_characters = alternative_surface.chars().collect::<Vec<_>>();
        if current_characters == alternative_characters {
            return false;
        }
        let common_prefix = current_characters
            .iter()
            .zip(&alternative_characters)
            .take_while(|(current, alternative)| current == alternative)
            .count();
        let common_suffix = current_characters[common_prefix..]
            .iter()
            .rev()
            .zip(alternative_characters[common_prefix..].iter().rev())
            .take_while(|(current, alternative)| current == alternative)
            .count();
        let changed_current_end = current_characters.len().saturating_sub(common_suffix);
        let Some(conversion) = self
            .convert_n_best_with_surface_prefix(reading, current_surface, 1)
            .into_iter()
            .find(|conversion| conversion.surface == current_surface)
        else {
            return false;
        };
        let mut surface_start = 0usize;
        let segments = conversion
            .segments
            .into_iter()
            .map(|segment| {
                let surface_end = surface_start + segment.surface.chars().count();
                let roles = self.exact_personal_name_roles(&segment.reading, &segment.surface);
                let region = protect_regions
                    && segment.surface.chars().count() >= 3
                    && segment.surface.chars().all(|character| {
                        matches!(
                            character,
                            '\u{3400}'..='\u{4DBF}'
                                | '\u{4E00}'..='\u{9FFF}'
                                | '\u{F900}'..='\u{FAFF}'
                        )
                    })
                    && self.is_unique_exact_region_surface(&segment.reading, &segment.surface);
                let indexed = (surface_start, surface_end, roles, region);
                surface_start = surface_end;
                indexed
            })
            .collect::<Vec<_>>();
        for (index, &(surface_start, surface_end, roles, protected_region)) in
            segments.iter().enumerate()
        {
            let protected_name = protect_personal_names
                && (roles.full_name
                    || (roles.surname
                        && segments
                            .get(index + 1)
                            .is_some_and(|(_, _, next, _)| next.given_name))
                    || (roles.given_name && index > 0 && segments[index - 1].2.surname));
            let changes_segment = if current_characters.len() == alternative_characters.len() {
                current_characters
                    .iter()
                    .zip(&alternative_characters)
                    .enumerate()
                    .any(|(changed, (current, alternative))| {
                        current != alternative && (surface_start..surface_end).contains(&changed)
                    })
            } else if common_prefix == changed_current_end {
                // A pure insertion only changes a name when it splits the
                // existing segment, not when it is placed before or after it.
                surface_start < common_prefix && common_prefix < surface_end
            } else {
                common_prefix < surface_end && surface_start < changed_current_end
            };
            if changes_segment && (protected_name || protected_region) {
                return true;
            }
        }
        false
    }

    fn is_unique_exact_region_surface(&self, reading: &str, surface: &str) -> bool {
        let mut matched = false;
        let mut competing = false;
        self.for_each_exact(reading, |entry| {
            if MOZC_REGION_POS_IDS.contains(&entry.left_id)
                || MOZC_REGION_POS_IDS.contains(&entry.right_id)
            {
                if entry.surface == surface {
                    matched = true;
                } else {
                    competing = true;
                }
            }
        });
        matched && !competing
    }

    fn exact_personal_name_roles(&self, reading: &str, surface: &str) -> PersonalNameRoles {
        let mut roles = PersonalNameRoles::default();
        self.for_each_exact(reading, |entry| {
            if entry.surface == surface {
                roles.full_name |= entry.left_id == MOZC_PERSONAL_SURNAME_POS_ID
                    && entry.right_id == MOZC_PERSONAL_GIVEN_NAME_POS_ID;
                roles.surname |= entry.right_id == MOZC_PERSONAL_SURNAME_POS_ID;
                roles.given_name |= entry.left_id == MOZC_PERSONAL_GIVEN_NAME_POS_ID;
            }
        });
        roles
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

    fn exact_personal_name_entries<'s>(
        &'s self,
        reading: &str,
        role: PersonalNameRole,
        limit: usize,
    ) -> Vec<EntryView<'s>> {
        let mut entries = Vec::new();
        self.for_each_exact(reading, |entry| {
            let is_personal_name = match role {
                PersonalNameRole::Surname => entry.right_id == MOZC_PERSONAL_SURNAME_POS_ID,
                PersonalNameRole::GivenName => entry.left_id == MOZC_PERSONAL_GIVEN_NAME_POS_ID,
            };
            if is_personal_name && entry.surface != reading {
                entries.push(entry);
            }
        });
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

    fn document_strong_left_phrase_evidence(
        &self,
        reading: &str,
        left_context: &str,
        allows_single_character_prefix: bool,
    ) -> StrongLeftPhraseEvidence {
        if !self.uses_connection_costs {
            return StrongLeftPhraseEvidence::Absent;
        }
        // A weak compound can be valid but wrong for the sentence (for example
        // グループ展). Only explicit phrase boundaries and genitive phrases may
        // suppress the generic connection promotion of competing candidates.
        let context_start = left_context
            .char_indices()
            .rev()
            .nth(DOCUMENT_PHRASE_MAX_PREFIX_CHARACTERS.saturating_sub(1))
            .map_or(0, |(index, _)| index);
        let context_tail = &left_context[context_start..];
        if !left_context.ends_with('の')
            && !context_tail
                .chars()
                .any(|character| !character.is_alphanumeric())
        {
            return StrongLeftPhraseEvidence::Absent;
        }
        let connection = ConnectionMatrix::bundled();
        let mut candidates = Vec::<(&str, i32)>::new();
        // Deduplicate POS variants before reverse lookups. A phrase promotion
        // cannot recover a surface farther away than its maximum promotion.
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading {
                return;
            }
            let isolated_cost = whole_reading_entry_cost(Some(connection), &entry);
            if let Some((_, best_cost)) = candidates
                .iter_mut()
                .find(|(surface, _)| *surface == entry.surface)
            {
                *best_cost = (*best_cost).min(isolated_cost);
            } else {
                candidates.push((entry.surface, isolated_cost));
            }
        });
        let Some(best_isolated_cost) = candidates.iter().map(|(_, cost)| *cost).min() else {
            return StrongLeftPhraseEvidence::Absent;
        };
        if candidates.into_iter().any(|(surface, isolated_cost)| {
            if isolated_cost > best_isolated_cost.saturating_add(DOCUMENT_PHRASE_PROMOTION) {
                return false;
            }
            let promotion = self.document_phrase_promotion(
                left_context,
                reading,
                surface,
                allows_single_character_prefix,
            );
            promotion == DOCUMENT_PHRASE_PROMOTION
                || (left_context.ends_with('の') && promotion >= 2_000)
        }) {
            StrongLeftPhraseEvidence::Present
        } else {
            StrongLeftPhraseEvidence::Absent
        }
    }

    fn document_phrase_promotion(
        &self,
        left_context: &str,
        reading: &str,
        candidate_surface: &str,
        allows_single_character_prefix: bool,
    ) -> i32 {
        self.document_phrase_word_cost(
            left_context,
            reading,
            candidate_surface,
            allows_single_character_prefix,
        )
        .map_or(0, |word_cost| {
            DOCUMENT_PHRASE_COST_CEILING
                .saturating_sub(word_cost)
                .min(DOCUMENT_PHRASE_PROMOTION)
        })
    }

    fn document_phrase_word_cost(
        &self,
        left_context: &str,
        reading: &str,
        candidate_surface: &str,
        allows_single_character_prefix: bool,
    ) -> Option<i32> {
        self.document_phrase_word_cost_with_policy(
            left_context,
            reading,
            candidate_surface,
            allows_single_character_prefix,
            false,
        )
    }

    fn document_non_person_phrase_word_cost(
        &self,
        left_context: &str,
        reading: &str,
        candidate_surface: &str,
        allows_single_character_prefix: bool,
    ) -> Option<i32> {
        self.document_phrase_word_cost_with_policy(
            left_context,
            reading,
            candidate_surface,
            allows_single_character_prefix,
            true,
        )
    }

    fn document_phrase_word_cost_with_policy(
        &self,
        left_context: &str,
        reading: &str,
        candidate_surface: &str,
        allows_single_character_prefix: bool,
        require_non_person: bool,
    ) -> Option<i32> {
        let compact = self.bundled?;
        let context_start = left_context
            .char_indices()
            .rev()
            .nth(DOCUMENT_PHRASE_MAX_PREFIX_CHARACTERS.saturating_sub(1))
            .map_or(0, |(index, _)| index);
        let context_tail = &left_context[context_start..];
        let context_characters = context_tail.chars().count();
        let mut best_word_cost = None;
        for (position, (index, _)) in context_tail.char_indices().enumerate() {
            let prefix_characters = context_characters - position;
            if prefix_characters < DOCUMENT_PHRASE_MIN_PREFIX_CHARACTERS
                && (prefix_characters != 1 || !allows_single_character_prefix)
            {
                break;
            }
            let word_cost = if require_non_person {
                compact.joined_non_person_surface_reading_suffix_cost(
                    &context_tail[index..],
                    candidate_surface,
                    reading,
                )
            } else {
                compact.joined_surface_reading_suffix_cost(
                    &context_tail[index..],
                    candidate_surface,
                    reading,
                )
            };
            if let Some(word_cost) = word_cost {
                best_word_cost =
                    Some(best_word_cost.map_or(word_cost, |best: i32| best.min(word_cost)));
            }
        }
        best_word_cost
    }

    fn document_left_phrase_is_unique(
        &self,
        reading: &str,
        left_context: &str,
        candidate_surface: &str,
        allows_single_character_prefix: bool,
    ) -> bool {
        let candidate_has_non_person_phrase = self
            .document_non_person_phrase_word_cost(
                left_context,
                reading,
                candidate_surface,
                allows_single_character_prefix,
            )
            .is_some_and(|word_cost| word_cost < DOCUMENT_PHRASE_COST_CEILING);
        let mut candidate_is_exact = false;
        let mut has_competing_phrase = false;
        self.for_each_exact(reading, |entry| {
            if has_competing_phrase || entry.surface == reading {
                return;
            }
            if entry.surface == candidate_surface {
                candidate_is_exact = true;
                return;
            }
            let competing_word_cost = if candidate_has_non_person_phrase {
                self.document_non_person_phrase_word_cost(
                    left_context,
                    reading,
                    entry.surface,
                    allows_single_character_prefix,
                )
            } else {
                self.document_phrase_word_cost(
                    left_context,
                    reading,
                    entry.surface,
                    allows_single_character_prefix,
                )
            };
            has_competing_phrase = competing_word_cost
                .is_some_and(|word_cost| word_cost < DOCUMENT_PHRASE_COST_CEILING);
        });
        candidate_is_exact && !has_competing_phrase
    }

    fn document_right_phrase_promotions<'s>(
        &'s self,
        reading: &str,
        left_context: &str,
        right_context: &str,
        allows_single_character_phrase_prefix: bool,
    ) -> Vec<DocumentBoundaryPromotion<'s>> {
        let Some((numeric_left_context, has_left_phrase_evidence)) = self
            .document_right_phrase_requirements(
                reading,
                left_context,
                allows_single_character_phrase_prefix,
            )
        else {
            return Vec::new();
        };
        let connection = ConnectionMatrix::bundled();
        let mut promotions = Vec::new();
        self.for_each_exact(reading, |entry| {
            let promotion = self
                .document_right_phrase_promotion(
                    reading,
                    entry.surface,
                    right_context,
                    numeric_left_context,
                    has_left_phrase_evidence,
                )
                .0;
            if entry.surface != reading && promotion > 0 {
                promotions.push(DocumentBoundaryPromotion {
                    surface: entry.surface,
                    isolated_cost: whole_reading_entry_cost(Some(connection), &entry),
                    promotion,
                });
            }
        });
        promotions
    }

    fn document_multi_segment_right_phrase_promotions<'s>(
        &'s self,
        reading: &str,
        left_context: &str,
        right_context: &str,
        allows_single_character_phrase_prefix: bool,
    ) -> Vec<(&'s str, i32)> {
        let Some((requires_general_noun, requires_noun_prefix)) = self
            .document_right_phrase_requirements(
                reading,
                left_context,
                allows_single_character_phrase_prefix,
            )
        else {
            return Vec::new();
        };
        let mut promotions = Vec::new();
        self.for_each_exact(reading, |entry| {
            let promotion = self
                .document_right_phrase_promotion(
                    reading,
                    entry.surface,
                    right_context,
                    requires_general_noun,
                    requires_noun_prefix,
                )
                .1;
            if entry.surface != reading && promotion > 0 {
                promotions.push((entry.surface, promotion));
            }
        });
        promotions
    }

    fn document_right_phrase_requirements(
        &self,
        reading: &str,
        left_context: &str,
        allows_single_character_phrase_prefix: bool,
    ) -> Option<(bool, bool)> {
        if !self.uses_connection_costs {
            return None;
        }
        let numeric_left_context = trailing_numeric_surface(left_context).is_some();
        // Japanese numerals retain the dedicated counter boundary. Decimal
        // suffixes also appear inside route IDs and section numbers, so they
        // may use only the stricter general-noun phrase evidence below.
        if numeric_left_context && split_trailing_decimal(left_context).is_none() {
            return None;
        }
        let mut has_left_phrase_evidence = false;
        let mut has_noun_prefix_candidate = false;
        self.for_each_exact(reading, |entry| {
            // ID 2600 is Mozc's generic noun-prefix class and contains broad
            // homophones such as 快/皆. Only a lexeme-specific prefix may
            // reopen a boundary that already has exact evidence on the left.
            has_noun_prefix_candidate |= (MOZC_EXPLICIT_NOUN_PREFIX_POS_ID_START
                ..=MOZC_NOUN_PREFIX_POS_ID_END)
                .contains(&entry.left_id)
                && entry.surface.chars().count() == 1;
            has_left_phrase_evidence |= self.document_phrase_promotion(
                left_context,
                reading,
                entry.surface,
                allows_single_character_phrase_prefix,
            ) > 0;
        });
        (!has_left_phrase_evidence || has_noun_prefix_candidate)
            .then_some((numeric_left_context, has_left_phrase_evidence))
    }

    fn document_right_phrase_promotion(
        &self,
        reading: &str,
        candidate_surface: &str,
        right_context: &str,
        requires_general_noun: bool,
        requires_noun_prefix: bool,
    ) -> (i32, i32) {
        let Some(compact) = self.bundled else {
            return (0, 0);
        };
        let Some(first) = right_context.chars().next() else {
            return (0, 0);
        };
        if !candidate_surface
            .chars()
            .any(|character| matches!(character, '\u{3400}'..='\u{9fff}'))
            || !matches!(
                first,
                '\u{3040}'..='\u{309f}' | '\u{30a0}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}'
            )
        {
            return (0, 0);
        }
        let context_end = right_context
            .char_indices()
            .nth(DOCUMENT_RIGHT_PHRASE_MAX_SUFFIX_CHARACTERS)
            .map_or(right_context.len(), |(index, _)| index);
        let context_head = &right_context[..context_end];
        let starts_with_hiragana = matches!(first, '\u{3040}'..='\u{309f}');
        let mut best_promotion = 0;
        let mut best_multi_segment_promotion = 0;
        compact.for_each_joined_surface_reading_prefix(
            candidate_surface,
            context_head,
            reading,
            DOCUMENT_RIGHT_PHRASE_MAX_SUFFIX_CHARACTERS,
            |suffix, entry| {
                let bounded_nominal_suffix =
                    is_bounded_coordination_suffix(suffix) || is_bounded_genitive_suffix(suffix);
                let noun_prefix_entry = (MOZC_NOUN_PREFIX_POS_ID_START
                    ..=MOZC_NOUN_PREFIX_POS_ID_END)
                    .contains(&entry.left_id)
                    && candidate_surface.chars().count() == 1;
                let accepted = !matches!(
                    entry.left_id,
                    MOZC_PERSONAL_GIVEN_NAME_POS_ID | MOZC_PERSONAL_SURNAME_POS_ID
                ) && !matches!(
                    entry.right_id,
                    MOZC_PERSONAL_GIVEN_NAME_POS_ID | MOZC_PERSONAL_SURNAME_POS_ID
                ) && (!requires_general_noun
                    || (entry.left_id == MOZC_GENERAL_NOUN_POS_ID
                        && entry.right_id == MOZC_GENERAL_NOUN_POS_ID))
                    && (!requires_noun_prefix || noun_prefix_entry)
                    && (!starts_with_hiragana
                        || is_safe_hiragana_right_phrase_entry(entry, suffix))
                    && (!bounded_nominal_suffix
                        || right_phrase_suffix_has_boundary(suffix, right_context));
                if !accepted {
                    return;
                }
                let cost_ceiling = if noun_prefix_entry {
                    DOCUMENT_RIGHT_NOUN_PREFIX_PHRASE_COST_CEILING
                } else if is_bounded_coordination_suffix(suffix) {
                    DOCUMENT_RIGHT_COORDINATION_PHRASE_COST_CEILING
                } else if is_sibling_right_phrase_suffix(suffix, right_context) {
                    DOCUMENT_RIGHT_SIBLING_PHRASE_COST_CEILING
                } else if suffix == "的" {
                    DOCUMENT_RIGHT_DERIVATIONAL_PHRASE_COST_CEILING
                } else if suffix.chars().count() >= 2 {
                    DOCUMENT_RIGHT_LONG_PHRASE_COST_CEILING
                } else {
                    DOCUMENT_RIGHT_SHORT_PHRASE_COST_CEILING
                };
                let promotion =
                    cost_ceiling
                        .saturating_sub(entry.word_cost)
                        .min(if noun_prefix_entry {
                            DOCUMENT_RIGHT_NOUN_PREFIX_PHRASE_PROMOTION
                        } else {
                            DOCUMENT_PHRASE_PROMOTION
                        });
                if promotion > best_promotion {
                    best_promotion = promotion;
                }
                if suffix.chars().count() <= 2 || bounded_nominal_suffix {
                    best_multi_segment_promotion = best_multi_segment_promotion.max(promotion);
                }
            },
        );
        (best_promotion, best_multi_segment_promotion)
    }

    fn document_context_has_pos_suffix(
        &self,
        left_context: &str,
        pos_ids: &[u16],
        minimum_characters: usize,
        maximum_characters: usize,
    ) -> bool {
        let Some(compact) = self.bundled else {
            return false;
        };
        let context_start = left_context
            .char_indices()
            .rev()
            .nth(maximum_characters.saturating_sub(1))
            .map_or(0, |(index, _)| index);
        let context_tail = &left_context[context_start..];
        let context_characters = context_tail.chars().count();
        for (position, (index, _)) in context_tail.char_indices().enumerate() {
            if context_characters - position < minimum_characters {
                break;
            }
            let suffix = &context_tail[index..];
            let mut best_cost = i32::MAX;
            let mut best_matching_cost = i32::MAX;
            if suffix
                .chars()
                .all(|character| matches!(character, '\u{3040}'..='\u{30ff}'))
            {
                // Place names are sometimes intentionally written in kana
                // (for example いなべ市). When the best dictionary reading is
                // itself a region entry, retain that POS evidence instead of
                // requiring a kanji surface spelling in the document.
                self.for_each_exact(suffix, |entry| {
                    best_cost = best_cost.min(entry.word_cost);
                    if pos_ids.contains(&entry.right_id) {
                        best_matching_cost = best_matching_cost.min(entry.word_cost);
                    }
                });
            }
            compact.for_each_surface_entry(suffix, |entry| {
                best_cost = best_cost.min(entry.word_cost);
                if pos_ids.contains(&entry.right_id) {
                    best_matching_cost = best_matching_cost.min(entry.word_cost);
                }
            });
            if best_matching_cost != i32::MAX
                && best_matching_cost <= best_cost.saturating_add(DOCUMENT_POS_SURFACE_COST_GAP)
            {
                return true;
            }
        }
        false
    }

    fn document_context_boundary_right_ids(&self, left_context: &str) -> Vec<u16> {
        let context_start = left_context
            .char_indices()
            .rev()
            .nth(DOCUMENT_BOUNDARY_MAX_CONTEXT_CHARACTERS.saturating_sub(1))
            .map_or(0, |(index, _)| index);
        let context_tail = &left_context[context_start..];
        let mut boundaries = context_tail
            .char_indices()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        boundaries.reverse();
        let mut pos_costs = Vec::<(u16, i32)>::new();

        for index in boundaries {
            let suffix = &context_tail[index..];
            // Without a reading boundary, a multi-character kana tail can be
            // an arbitrary slice through an inflection. Fall back to its final
            // one-character grammar entry instead of inventing a word split.
            if suffix.chars().count() > 1
                && suffix
                    .chars()
                    .all(|character| matches!(character, '\u{3040}'..='\u{30ff}'))
            {
                continue;
            }
            pos_costs.clear();
            self.for_each_exact(suffix, |entry| {
                if entry.surface == suffix {
                    pos_costs.push((entry.right_id, entry.word_cost));
                }
            });
            if let Some(compact) = self.bundled {
                compact.for_each_surface_entry(suffix, |entry| {
                    pos_costs.push((entry.right_id, entry.word_cost));
                });
            }
            for layer in self.layers.iter() {
                for entry in layer.entries.iter().filter(|entry| entry.surface == suffix) {
                    pos_costs.push((entry.right_id, entry.word_cost));
                }
            }
            let Some(best_cost) = pos_costs.iter().map(|(_, word_cost)| *word_cost).min() else {
                continue;
            };
            let mut right_ids = Vec::new();
            for &(right_id, word_cost) in &pos_costs {
                if word_cost <= best_cost.saturating_add(DOCUMENT_POS_SURFACE_COST_GAP)
                    && !right_ids.contains(&right_id)
                {
                    right_ids.push(right_id);
                }
            }
            return right_ids;
        }
        Vec::new()
    }

    fn document_boundary_promotions<'s>(
        &'s self,
        reading: &str,
        left_context: &str,
    ) -> Vec<DocumentBoundaryPromotion<'s>> {
        // Numeric notation and verbatim reuse have stronger dedicated rules.
        // Boundary connection is deliberately a one-way, capped promotion:
        // uncertain context must never evict an otherwise visible candidate.
        if !self.uses_connection_costs || trailing_numeric_surface(left_context).is_some() {
            return Vec::new();
        }
        let mut has_repeated_surface = false;
        self.for_each_exact(reading, |entry| {
            if entry.surface != reading
                && entry.surface.chars().count() >= 2
                && left_context.contains(entry.surface)
            {
                has_repeated_surface = true;
            }
        });
        if has_repeated_surface {
            return Vec::new();
        }
        let boundary_right_ids = self.document_context_boundary_right_ids(left_context);
        if boundary_right_ids.is_empty() {
            return Vec::new();
        }
        let connection = ConnectionMatrix::bundled();
        let mut promotions = Vec::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading {
                return;
            }
            let isolated_cost = whole_reading_entry_cost(Some(connection), &entry);
            let adjustment = boundary_right_ids
                .iter()
                .map(|right_id| {
                    connection
                        .cost(*right_id, entry.left_id)
                        .saturating_sub(connection.cost(BOS_EOS_POS_ID, entry.left_id))
                })
                .min()
                .unwrap_or(0);
            let promotion = adjustment
                .saturating_neg()
                .clamp(0, DOCUMENT_BOUNDARY_PROMOTION_CAP);
            if promotion > 0 {
                promotions.push(DocumentBoundaryPromotion {
                    surface: entry.surface,
                    isolated_cost,
                    promotion,
                });
            }
        });
        promotions
    }

    fn document_right_inflection_promotions<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Vec<DocumentBoundaryPromotion<'s>> {
        if !self.uses_connection_costs || !starts_with_polite_auxiliary(right_context) {
            return Vec::new();
        }
        let connection = ConnectionMatrix::bundled();
        let mut promotions = Vec::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading || !looks_like_inflected_kanji_surface(entry.surface) {
                return;
            }
            promotions.push(DocumentBoundaryPromotion {
                surface: entry.surface,
                isolated_cost: whole_reading_entry_cost(Some(connection), &entry),
                promotion: DOCUMENT_POLITE_AUXILIARY_PROMOTION,
            });
        });
        promotions
    }

    fn document_right_auxiliary_costs<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Vec<DocumentContextualCost<'s>> {
        if !self.uses_connection_costs {
            return Vec::new();
        }
        if !right_context.starts_with("たい") {
            return Vec::new();
        }
        let connection = ConnectionMatrix::bundled();
        let mut costs = Vec::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading || !looks_like_inflected_kanji_surface(entry.surface) {
                return;
            }
            costs.push(DocumentContextualCost {
                surface: entry.surface,
                relative_cost: connection
                    .cost(BOS_EOS_POS_ID, entry.left_id)
                    .saturating_add(entry.word_cost)
                    .saturating_add(
                        connection.cost(entry.right_id, MOZC_DESIDERATIVE_AUXILIARY_POS_ID),
                    ),
            });
        });
        // Candidate generation deduplicates equal surfaces, so retain every exact POS path here
        // and normalize the paths that can connect directly to the following auxiliary.
        if let Some(minimum) = costs
            .iter()
            .map(|contextual| contextual.relative_cost)
            .min()
        {
            for contextual in &mut costs {
                contextual.relative_cost = contextual.relative_cost.saturating_sub(minimum);
            }
        }
        costs
    }

    fn document_right_function_word_costs<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Vec<DocumentContextualCost<'s>> {
        if !self.uses_connection_costs {
            return Vec::new();
        }
        let Some(right_left_ids) = document_right_function_word_left_ids(right_context) else {
            return Vec::new();
        };

        let connection = ConnectionMatrix::bundled();
        let mut costs = Vec::<DocumentContextualCost<'s>>::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading
                || !entry
                    .surface
                    .chars()
                    .any(|character| matches!(character, '\u{3400}'..='\u{9fff}'))
            {
                return;
            }
            let transition_cost = right_left_ids
                .iter()
                .map(|left_id| connection.cost(entry.right_id, *left_id))
                .min()
                .unwrap_or(i32::MAX);
            if let Some(contextual) = costs
                .iter_mut()
                .find(|contextual| contextual.surface == entry.surface)
            {
                contextual.relative_cost = contextual.relative_cost.min(transition_cost);
            } else {
                costs.push(DocumentContextualCost {
                    surface: entry.surface,
                    relative_cost: transition_cost,
                });
            }
        });
        if let Some(minimum) = costs
            .iter()
            .map(|contextual| contextual.relative_cost)
            .min()
        {
            for contextual in &mut costs {
                contextual.relative_cost = contextual.relative_cost.saturating_sub(minimum);
            }
        }
        costs
    }

    fn document_right_particle_costs<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Vec<DocumentContextualCost<'s>> {
        if !self.uses_connection_costs {
            return Vec::new();
        }
        let Some(right_left_ids) = document_right_particle_left_ids(right_context) else {
            return Vec::new();
        };

        let connection = ConnectionMatrix::bundled();
        let mut costs = Vec::<DocumentContextualCost<'s>>::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading
                || !entry
                    .surface
                    .chars()
                    .any(|character| matches!(character, '\u{3400}'..='\u{9fff}'))
            {
                return;
            }
            let transition_cost = right_left_ids
                .iter()
                .map(|left_id| connection.cost(entry.right_id, *left_id))
                .min()
                .unwrap_or(i32::MAX);
            if let Some(contextual) = costs
                .iter_mut()
                .find(|contextual| contextual.surface == entry.surface)
            {
                contextual.relative_cost = contextual.relative_cost.min(transition_cost);
            } else {
                costs.push(DocumentContextualCost {
                    surface: entry.surface,
                    relative_cost: transition_cost,
                });
            }
        });
        if let Some(minimum) = costs
            .iter()
            .map(|contextual| contextual.relative_cost)
            .min()
        {
            for contextual in &mut costs {
                contextual.relative_cost = contextual.relative_cost.saturating_sub(minimum);
            }
        }
        costs
    }

    fn document_right_grammar_costs<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Vec<DocumentContextualCost<'s>> {
        if !self.uses_connection_costs
            || starts_with_polite_auxiliary(right_context)
            || right_context.starts_with("させ")
        {
            return Vec::new();
        }
        let Some(right_pos_id) = document_right_grammar_pos_id(right_context) else {
            return Vec::new();
        };

        let connection = ConnectionMatrix::bundled();
        let mut costs = Vec::<DocumentContextualCost<'s>>::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading
                || !entry
                    .surface
                    .chars()
                    .any(|character| matches!(character, '\u{3400}'..='\u{9fff}'))
            {
                return;
            }
            let transition_cost = connection.cost(entry.right_id, right_pos_id);
            if let Some(contextual) = costs
                .iter_mut()
                .find(|contextual| contextual.surface == entry.surface)
            {
                contextual.relative_cost = contextual.relative_cost.min(transition_cost);
            } else {
                costs.push(DocumentContextualCost {
                    surface: entry.surface,
                    relative_cost: transition_cost,
                });
            }
        });
        if let Some(minimum) = costs
            .iter()
            .map(|contextual| contextual.relative_cost)
            .min()
        {
            for contextual in &mut costs {
                contextual.relative_cost = if contextual.relative_cost
                    <= minimum.saturating_add(DOCUMENT_RIGHT_GRAMMAR_COMPATIBILITY_MARGIN)
                {
                    0
                } else {
                    DOCUMENT_RIGHT_GRAMMAR_PROMOTION_CAP
                };
            }
        }
        costs
    }

    fn document_has_strong_kana_verb_surface(&self, reading: &str) -> bool {
        if reading.chars().count() < 2 || !reading.chars().all(is_hiragana_character) {
            return false;
        }
        let mut found = false;
        self.for_each_exact(reading, |entry| {
            found |= entry.surface == reading
                && entry.left_id == entry.right_id
                && (MOZC_INDEPENDENT_VERB_POS_ID_START..=MOZC_INDEPENDENT_VERB_POS_ID_END)
                    .contains(&entry.left_id)
                && entry.word_cost <= DOCUMENT_STRONG_KANA_VERB_COST_CEILING;
        });
        found
    }

    fn document_unique_right_grammar_surface<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Option<&'s str> {
        if !self.uses_connection_costs {
            return None;
        }
        let right_pos_id = document_right_grammar_pos_id(right_context)?;
        let connection = ConnectionMatrix::bundled();
        let mut surface_costs = Vec::<(&str, i32)>::new();
        self.for_each_exact(reading, |entry| {
            if entry.surface == reading
                || !entry
                    .surface
                    .chars()
                    .any(|character| matches!(character, '\u{3400}'..='\u{9fff}'))
            {
                return;
            }
            let transition_cost = connection.cost(entry.right_id, right_pos_id);
            if let Some((_, cost)) = surface_costs
                .iter_mut()
                .find(|(surface, _)| *surface == entry.surface)
            {
                *cost = (*cost).min(transition_cost);
            } else {
                surface_costs.push((entry.surface, transition_cost));
            }
        });
        let minimum = surface_costs.iter().map(|(_, cost)| *cost).min()?;
        // A grammatical continuation is only structural evidence. If several
        // surfaces connect almost equally well, choosing among them needs
        // semantic context and must remain with the existing ranker.
        let mut compatible = surface_costs.into_iter().filter(|(_, cost)| {
            *cost <= minimum.saturating_add(DOCUMENT_RIGHT_GRAMMAR_COMPATIBILITY_MARGIN)
        });
        let (surface, _) = compatible.next()?;
        compatible.next().is_none().then_some(surface)
    }

    fn document_unique_right_suru_surface<'s>(
        &'s self,
        reading: &str,
        right_context: &str,
    ) -> Option<&'s str> {
        if !self.uses_connection_costs || !starts_with_suru_inflection(right_context) {
            return None;
        }
        let mut surface_costs = Vec::<(&str, i32)>::new();
        self.for_each_exact(reading, |entry| {
            if entry.left_id != MOZC_VERBAL_NOUN_POS_ID
                || entry.right_id != MOZC_VERBAL_NOUN_POS_ID
                || entry.surface == reading
            {
                return;
            }
            if let Some((_, cost)) = surface_costs
                .iter_mut()
                .find(|(surface, _)| *surface == entry.surface)
            {
                *cost = (*cost).min(entry.word_cost);
            } else {
                surface_costs.push((entry.surface, entry.word_cost));
            }
        });
        surface_costs.sort_unstable_by_key(|(_, cost)| *cost);
        let (surface, cost) = surface_costs.first().copied()?;
        surface_costs
            .get(1)
            .is_none_or(|(_, next_cost)| {
                *next_cost > cost.saturating_add(DOCUMENT_UNIQUE_RIGHT_SURU_COMPATIBILITY_MARGIN)
            })
            .then_some(surface)
    }

    fn document_numeric_counter_promotions(
        &self,
        reading: &str,
        left_context: &str,
    ) -> Vec<(&'static str, i32)> {
        if structured_notation_owns_numeric_context(left_context, reading) {
            return Vec::new();
        }
        let Some(numeric_surface) = trailing_numeric_surface(left_context) else {
            return Vec::new();
        };
        let Some(compact) = self.bundled else {
            return Vec::new();
        };

        let mut promotions = Vec::new();
        compact.for_each_exact(reading, |entry| {
            if entry.right_id != MOZC_COUNTER_POS_ID
                || promotions
                    .iter()
                    .any(|(surface, _)| *surface == entry.surface)
            {
                return;
            }
            let Some(word_cost) = compact.joined_surface_reading_suffix_cost(
                &numeric_surface,
                entry.surface,
                reading,
            ) else {
                return;
            };
            let promotion = DOCUMENT_NUMERIC_COMPOUND_COST_CEILING
                .saturating_sub(word_cost)
                .min(DOCUMENT_NUMERIC_COMPOUND_PROMOTION_CAP);
            if promotion > 0 {
                promotions.push((entry.surface, promotion));
            }
        });
        promotions
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

    fn segment_has_exact_pos(&self, segment: &Segment, predicate: impl Fn(u16) -> bool) -> bool {
        let mut found = false;
        self.for_each_exact(&segment.reading, |entry| {
            found |= entry.surface == segment.surface
                && (predicate(entry.left_id) || predicate(entry.right_id));
        });
        found
    }

    /// Promotes the colloquial `目的語 + 命令形 + って` frame without teaching
    /// the dictionary a phrase-specific surface. Mozc's conjugation POS keeps
    /// homographic nouns such as 賭け out, while a noun immediately before the
    /// verb distinguishes the ellipsis from arbitrary kana fragmentation.
    fn colloquial_imperative_quotation_promotion(
        &self,
        left_context: &str,
        conversion: &Conversion,
    ) -> i32 {
        let [prefix @ .., imperative, quotation] = conversion.segments.as_slice() else {
            return 0;
        };
        let Some(object) = prefix.last() else {
            return 0;
        };
        if quotation.reading != "って"
            || quotation.surface != "って"
            || imperative.surface == imperative.reading
            || !imperative.surface.chars().any(is_ideographic_or_digit)
            || !self.segment_has_exact_pos(imperative, is_mozc_independent_imperative_pos_id)
            || !self.segment_has_exact_pos(object, |pos_id| {
                matches!(pos_id, MOZC_VERBAL_NOUN_POS_ID | MOZC_GENERAL_NOUN_POS_ID)
            })
        {
            return 0;
        }
        if document_context_ends_with_clause_boundary(left_context) {
            DOCUMENT_COLLOQUIAL_IMPERATIVE_PROMOTION
        } else if prefix.len() >= 2 {
            EMBEDDED_COLLOQUIAL_IMPERATIVE_PROMOTION
        } else {
            0
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

    fn for_each_prefix_guarding_numeric_starts<'s>(
        &'s self,
        reading: &str,
        start: usize,
        protected: &mut [u8],
        mut callback: impl FnMut(usize, EntryView<'s>),
    ) {
        self.for_each_prefix(&reading[start..], |relative_end, entry| {
            protect_numeric_starts_inside_dictionary_entry(
                reading,
                start,
                relative_end,
                entry,
                protected,
            );
            callback(relative_end, entry);
        });
    }

    #[must_use]
    pub fn candidates(&self, reading: &str) -> Vec<Candidate> {
        self.candidates_with_ranker(reading, DEFAULT_N_BEST, &CostOnlyRanker)
    }

    /// Returns candidates ranked with bounded, input-client-supplied document
    /// context. No context is retained by the dictionary.
    #[must_use]
    pub fn candidates_with_context(&self, reading: &str, left_context: &str) -> Vec<Candidate> {
        self.candidates_with_context_ranker(
            reading,
            left_context,
            DEFAULT_N_BEST,
            &DictionaryDocumentContextRanker::new(self, reading, left_context),
        )
    }

    /// Returns default-width candidates using committed text on both sides of
    /// the editing position.
    #[must_use]
    pub fn candidates_with_surrounding_context(
        &self,
        reading: &str,
        left_context: &str,
        right_context: &str,
    ) -> Vec<Candidate> {
        self.candidates_with_surrounding_context_limit(
            reading,
            left_context,
            right_context,
            DEFAULT_N_BEST,
        )
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

    /// Context-aware counterpart of [`Self::candidates_with_limit`].
    #[must_use]
    pub fn candidates_with_context_limit(
        &self,
        reading: &str,
        left_context: &str,
        limit: usize,
    ) -> Vec<Candidate> {
        self.candidates_with_context_ranker(
            reading,
            left_context,
            limit,
            &DictionaryDocumentContextRanker::new(self, reading, left_context),
        )
    }

    /// Context-aware conversion using both committed sides of an editing
    /// position. Only fixed grammatical prefixes are inspected in the right
    /// context, and neither side is retained.
    #[must_use]
    pub fn candidates_with_surrounding_context_limit(
        &self,
        reading: &str,
        left_context: &str,
        right_context: &str,
        limit: usize,
    ) -> Vec<Candidate> {
        let expands_search =
            should_expand_alphanumeric_numeric_compound(reading, left_context, right_context)
                || should_expand_numeric_particle_suru(reading, right_context);
        let search_limit = if expands_search { limit.max(32) } else { limit };
        let mut candidates = self.candidates_with_context_ranker(
            reading,
            left_context,
            search_limit,
            &DictionaryDocumentContextRanker::new_with_surrounding_context(
                self,
                reading,
                left_context,
                right_context,
            ),
        );
        if expands_search {
            candidates.truncate(limit);
        }
        candidates
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
        if reading_has_roman_numeral_suffix(reading) {
            Self::append_roman_numeral_variants(&mut conversions);
        }

        for conversion in conversions {
            let cost = if conversion.surface == reading {
                LITERAL_CANDIDATE_COST
            } else {
                ranker
                    .ranking_cost_with_context(reading, left_context, &conversion)
                    .saturating_sub(
                        self.colloquial_imperative_quotation_promotion(left_context, &conversion),
                    )
                    .saturating_sub(Self::contextual_roman_numeral_suffix_promotion(
                        left_context,
                        &conversion,
                    ))
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

    fn append_roman_numeral_variants(conversions: &mut Vec<Conversion>) {
        const ROMAN_VARIANT_COST: i32 = 4_000;

        let variants =
            conversions
                .iter()
                .filter_map(|conversion| {
                    let (index, numeral) = conversion.segments.windows(3).enumerate().find_map(
                        |(index, segments)| {
                            let [prefix, number, particle] = segments else {
                                return None;
                            };
                            if prefix.surface.chars().count() < 4
                                || !is_full_katakana_surface(&prefix.surface)
                                || particle.reading != "の"
                                || particle.surface != "の"
                            {
                                return None;
                            }
                            Self::roman_numeral(&number.surface).map(|numeral| (index + 1, numeral))
                        },
                    )?;
                    let mut variant = conversion.clone();
                    numeral.clone_into(&mut variant.segments[index].surface);
                    variant.segments[index].cost = variant.segments[index]
                        .cost
                        .saturating_add(ROMAN_VARIANT_COST);
                    variant.cost = variant.cost.saturating_add(ROMAN_VARIANT_COST);
                    variant.surface = variant
                        .segments
                        .iter()
                        .map(|segment| segment.surface.as_str())
                        .collect();
                    Some(variant)
                })
                .collect::<Vec<_>>();
        conversions.extend(variants);
    }

    fn roman_numeral(surface: &str) -> Option<&'static str> {
        match surface {
            "1" => Some("Ⅰ"),
            "2" => Some("Ⅱ"),
            "3" => Some("Ⅲ"),
            "4" => Some("Ⅳ"),
            "5" => Some("Ⅴ"),
            "6" => Some("Ⅵ"),
            "7" => Some("Ⅶ"),
            "8" => Some("Ⅷ"),
            "9" => Some("Ⅸ"),
            "10" => Some("Ⅹ"),
            "11" => Some("Ⅺ"),
            "12" => Some("Ⅻ"),
            _ => None,
        }
    }

    fn contextual_roman_numeral_suffix_promotion(
        left_context: &str,
        conversion: &Conversion,
    ) -> i32 {
        const PROMOTION: i32 = 6_000;

        if !left_context.ends_with('・') {
            return 0;
        }
        if conversion.segments.windows(3).any(|segments| {
            let [name, numeral, particle] = segments else {
                return false;
            };
            name.surface.chars().count() >= 4
                && is_full_katakana_surface(&name.surface)
                && Self::is_roman_numeral_surface(&numeral.surface)
                && particle.reading == "の"
                && particle.surface == "の"
        }) {
            PROMOTION
        } else {
            0
        }
    }

    fn is_roman_numeral_surface(surface: &str) -> bool {
        surface
            .chars()
            .all(|character| matches!(character, 'Ⅰ'..='Ⅻ'))
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
        let mut conversions = self.convert_n_best_exact(reading, limit);
        let variants = orthographic_long_vowel_variants(reading);
        if variants.is_empty() {
            return conversions;
        }
        let exact_surfaces = conversions
            .iter()
            .map(|conversion| conversion.surface.clone())
            .collect::<HashSet<_>>();
        let mut pronunciation_conversions = Vec::new();
        for variant in variants {
            let variant_conversions = self
                .convert_n_best_exact(&variant.reading, limit.min(LONG_VOWEL_PATHS_PER_VARIANT));
            pronunciation_conversions.extend(
                variant_conversions
                    .into_iter()
                    .filter_map(|conversion| {
                        remap_pronunciation_conversion(
                            conversion,
                            reading,
                            &variant.substituted_offsets,
                        )
                    })
                    .filter(|conversion| !exact_surfaces.contains(&conversion.surface)),
            );
        }
        sort_and_deduplicate_conversions(
            &mut pronunciation_conversions,
            LONG_VOWEL_MAX_ADDED_CONVERSIONS,
        );
        conversions.extend(pronunciation_conversions);
        sort_and_deduplicate_conversions(&mut conversions, limit);
        conversions
    }

    fn convert_n_best_exact(&self, reading: &str, limit: usize) -> Vec<Conversion> {
        if self.uses_connection_costs {
            self.convert_n_best_connected(reading, limit)
        } else {
            self.convert_n_best_heuristic(reading, limit)
        }
    }

    /// Returns complete conversion paths whose rendered surface starts with
    /// `surface_prefix`. The constraint is applied while expanding the lattice
    /// so paths that disagree with it do not consume the beam.
    #[must_use]
    pub fn convert_n_best_with_surface_prefix(
        &self,
        reading: &str,
        surface_prefix: &str,
        limit: usize,
    ) -> Vec<Conversion> {
        if reading.is_empty() || limit == 0 {
            return Vec::new();
        }
        if surface_prefix.is_empty() {
            return self.convert_n_best(reading, limit);
        }
        if u16::try_from(surface_prefix.len()).is_err() {
            return Vec::new();
        }
        let mut conversions =
            self.convert_n_best_with_surface_prefix_exact(reading, surface_prefix, limit);
        let variants = orthographic_long_vowel_variants(reading);
        if variants.is_empty() {
            return conversions;
        }
        let exact_surfaces = conversions
            .iter()
            .map(|conversion| conversion.surface.clone())
            .collect::<HashSet<_>>();
        let mut pronunciation_conversions = Vec::new();
        for variant in variants {
            let variant_conversions = self.convert_n_best_with_surface_prefix_exact(
                &variant.reading,
                surface_prefix,
                limit.min(LONG_VOWEL_PATHS_PER_VARIANT),
            );
            pronunciation_conversions.extend(
                variant_conversions
                    .into_iter()
                    .filter_map(|conversion| {
                        remap_pronunciation_conversion(
                            conversion,
                            reading,
                            &variant.substituted_offsets,
                        )
                    })
                    .filter(|conversion| !exact_surfaces.contains(&conversion.surface)),
            );
        }
        sort_and_deduplicate_conversions(
            &mut pronunciation_conversions,
            LONG_VOWEL_MAX_ADDED_CONVERSIONS,
        );
        conversions.extend(pronunciation_conversions);
        sort_and_deduplicate_conversions(&mut conversions, limit);
        conversions
    }

    fn convert_n_best_with_surface_prefix_exact(
        &self,
        reading: &str,
        surface_prefix: &str,
        limit: usize,
    ) -> Vec<Conversion> {
        if self.uses_connection_costs {
            self.convert_n_best_connected_with_surface_prefix(reading, Some(surface_prefix), limit)
        } else {
            self.convert_n_best_heuristic_with_surface_prefix(reading, Some(surface_prefix), limit)
        }
    }

    #[must_use]
    pub fn convert_best(&self, reading: &str) -> Option<Conversion> {
        let mut best = self.convert_best_exact(reading);
        for variant in orthographic_long_vowel_variants(reading) {
            let Some(conversion) =
                self.convert_best_exact(&variant.reading)
                    .and_then(|conversion| {
                        remap_pronunciation_conversion(
                            conversion,
                            reading,
                            &variant.substituted_offsets,
                        )
                    })
            else {
                continue;
            };
            if best
                .as_ref()
                .is_some_and(|current| conversion.surface == current.surface)
            {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|current| conversion.cost < current.cost)
            {
                best = Some(conversion);
            }
        }
        best
    }

    fn convert_best_exact(&self, reading: &str) -> Option<Conversion> {
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
        let synthetic_by_start = synthetic_entries_by_start(
            self,
            reading,
            &synthetic_arena,
            self.katakana_run_character_cost,
        );
        let mut numeric_start_states = numeric_start_states(reading, synthetic_by_start.as_slice());
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
            self.for_each_prefix_guarding_numeric_starts(
                reading,
                start,
                &mut numeric_start_states,
                |relative_end, entry| {
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
                },
            );

            insert_best_synthetic_nodes(
                reading,
                start,
                &synthetic_by_start[start],
                numeric_start_states[start] == NUMERIC_START_PROTECTED,
                &mut lattice,
                connection,
                &mut predecessor_cache,
            );

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
        self.convert_n_best_connected_with_surface_prefix(reading, None, limit)
    }

    fn convert_n_best_connected_with_surface_prefix(
        &self,
        reading: &str,
        surface_prefix: Option<&str>,
        limit: usize,
    ) -> Vec<Conversion> {
        let connection = ConnectionMatrix::bundled();
        let synthetic_arena = Bump::new();
        let synthetic_by_start = synthetic_entries_by_start(
            self,
            reading,
            &synthetic_arena,
            self.katakana_run_character_cost,
        );
        let mut numeric_start_states = numeric_start_states(reading, synthetic_by_start.as_slice());
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

            self.for_each_prefix_guarding_numeric_starts(
                reading,
                start,
                &mut numeric_start_states,
                |relative_end, entry| {
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
                        surface_prefix,
                        limit,
                    );
                },
            );

            for synthetic in &synthetic_by_start[start] {
                let synthetic_cost = guarded_cost(
                    synthetic,
                    numeric_start_states[start] == NUMERIC_START_PROTECTED,
                );
                insert_connected_word(
                    &mut arena,
                    &mut lattice[synthetic.end],
                    &predecessors,
                    connection,
                    start,
                    &reading[start..synthetic.end],
                    synthetic.surface,
                    (synthetic.left_id, synthetic.right_id),
                    synthetic_cost,
                    surface_prefix,
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
                surface_prefix,
                limit,
            );
        }

        let mut completed: Vec<_> = lattice[reading.len()]
            .states
            .iter()
            .filter(|&&node| {
                surface_prefix.is_none_or(|prefix| {
                    usize::from(arena[node].matched_prefix_bytes) == prefix.len()
                })
            })
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
        self.convert_n_best_heuristic_with_surface_prefix(reading, None, limit)
    }

    fn convert_n_best_heuristic_with_surface_prefix(
        &self,
        reading: &str,
        surface_prefix: Option<&str>,
        limit: usize,
    ) -> Vec<Conversion> {
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
                            matched_prefix_bytes: 0,
                        },
                        surface_prefix,
                        limit,
                    );
                } else {
                    for &predecessor in &predecessors {
                        let total_cost = arena[predecessor].total_cost.saturating_add(segment_cost);
                        let matched_prefix_bytes = arena[predecessor].matched_prefix_bytes;
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
                                matched_prefix_bytes,
                            },
                            surface_prefix,
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
                surface_prefix,
                limit,
            );
        }

        let mut completed: Vec<_> = lattice[reading.len()]
            .states
            .iter()
            .filter(|&&node| {
                surface_prefix.is_none_or(|prefix| {
                    usize::from(arena[node].matched_prefix_bytes) == prefix.len()
                })
            })
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
    matched_prefix_bytes: u16,
    total_cost: i32,
}

#[derive(Debug, Default)]
struct NBestBucket {
    states: Vec<usize>,
    worst_position: usize,
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

#[allow(clippy::too_many_arguments)]
fn insert_connected_unknown<'a>(
    reading: &'a str,
    start: usize,
    predecessors: &[usize],
    arena: &mut Vec<NBestNode<'a>>,
    lattice: &mut [NBestBucket],
    connection: ConnectionMatrix,
    surface_prefix: Option<&str>,
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
                matched_prefix_bytes: 0,
            },
            surface_prefix,
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
                matched_prefix_bytes: previous.matched_prefix_bytes,
            },
            surface_prefix,
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
    surface_prefix: Option<&str>,
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
                matched_prefix_bytes: 0,
            },
            surface_prefix,
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
                matched_prefix_bytes: arena[predecessor].matched_prefix_bytes,
            },
            surface_prefix,
            limit,
        );
    }
}

/// Inserts one word (dictionary or synthetic) into the n-best lattice,
/// fanning out over every predecessor state at `start`.
fn advance_surface_prefix(
    surface_prefix: Option<&str>,
    matched_bytes: u16,
    surface: &str,
) -> Option<u16> {
    let Some(prefix) = surface_prefix else {
        return Some(0);
    };
    let matched_bytes = usize::from(matched_bytes);
    if matched_bytes == prefix.len() {
        return u16::try_from(matched_bytes).ok();
    }
    let remaining = &prefix[matched_bytes..];
    if remaining.starts_with(surface) {
        u16::try_from(matched_bytes + surface.len()).ok()
    } else if surface.starts_with(remaining) {
        u16::try_from(prefix.len()).ok()
    } else {
        None
    }
}

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
    surface_prefix: Option<&str>,
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
                matched_prefix_bytes: 0,
            },
            surface_prefix,
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
                matched_prefix_bytes: previous.matched_prefix_bytes,
            },
            surface_prefix,
            limit,
        );
    }
}

fn insert_n_best_node<'a>(
    arena: &mut Vec<NBestNode<'a>>,
    bucket: &mut NBestBucket,
    mut candidate: NBestNode<'a>,
    surface_prefix: Option<&str>,
    limit_per_state: usize,
) {
    let Some(matched_prefix_bytes) = advance_surface_prefix(
        surface_prefix,
        candidate.matched_prefix_bytes,
        candidate.surface,
    ) else {
        return;
    };
    candidate.matched_prefix_bytes = matched_prefix_bytes;
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
    }

    if same_state_count < limit_per_state {
        if bucket.states.len() >= beam_size {
            let Some(worst_cost) = bucket.worst_total_cost else {
                return;
            };
            if candidate.total_cost >= worst_cost {
                return;
            }
            let worst_index = bucket.states[bucket.worst_position];
            arena[worst_index] = candidate;
            refresh_worst_n_best_cost(arena, bucket);
            return;
        }

        let index = arena.len();
        let position = bucket.states.len();
        let total_cost = candidate.total_cost;
        arena.push(candidate);
        bucket.states.push(index);
        if bucket
            .worst_total_cost
            .is_none_or(|worst_cost| total_cost >= worst_cost)
        {
            bucket.worst_position = position;
            bucket.worst_total_cost = Some(total_cost);
        }
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
    let worst = bucket
        .states
        .iter()
        .enumerate()
        .max_by_key(|&(position, &index)| (arena[index].total_cost, position));
    if let Some((position, _)) = worst {
        bucket.worst_position = position;
    }
    bucket.worst_total_cost = worst.map(|(_, &index)| arena[index].total_cost);
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

fn insert_best_synthetic_nodes<'a>(
    reading: &'a str,
    start: usize,
    entries: &[SyntheticEntry<'a>],
    protected_start: bool,
    lattice: &mut [Vec<LatticeNode<'a>>],
    connection: ConnectionMatrix,
    predecessor_cache: &mut Vec<(u16, i32, Option<NodeIndex>)>,
) {
    for entry in entries {
        let entry_reading = &reading[start..entry.end];
        let entry_cost = guarded_cost(entry, protected_start);
        let Some((predecessor_cost, predecessor)) = cached_connected_predecessor(
            lattice,
            start,
            entry.left_id,
            connection,
            predecessor_cache,
        ) else {
            continue;
        };
        let total_cost = predecessor_cost.saturating_add(entry_cost);
        insert_lattice_node(
            &mut lattice[entry.end],
            LatticeNode {
                start,
                predecessor,
                reading: entry_reading,
                surface: entry.surface,
                segment_cost: entry_cost,
                right_id: entry.right_id,
                total_cost,
            },
        );
    }
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
const MODEL_RECALL_KATAKANA_RUN_CHARACTER_COST: i32 = 3_000;

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

fn starts_with_polite_auxiliary(right_context: &str) -> bool {
    ["ました", "まして", "ましょう", "ません", "ます"]
        .iter()
        .any(|prefix| right_context.starts_with(prefix))
}

fn starts_with_copula(right_context: &str) -> bool {
    [
        "でした",
        "でしょう",
        "です",
        "だ",
        "であっ",
        "であり",
        "である",
        "であれ",
        "であろ",
        "でない",
        "でなか",
        "でなく",
        "では",
        "じゃ",
    ]
    .iter()
    .any(|prefix| right_context.starts_with(prefix))
}

fn starts_with_de_connective_continuation(right_context: &str) -> bool {
    [
        "でい",
        "でお",
        "でく",
        "でしま",
        "でみ",
        "でもら",
        "であげ",
        "でほし",
        "でる",
        "でた",
        "でます",
        "でました",
    ]
    .iter()
    .any(|prefix| right_context.starts_with(prefix))
}

fn document_right_function_word_left_ids(right_context: &str) -> Option<&'static [u16]> {
    if right_context.starts_with("こと") {
        return Some(&[MOZC_KOTO_NON_INDEPENDENT_NOUN_POS_ID]);
    }
    if right_context.starts_with("もの") {
        return Some(&[
            MOZC_MONO_CASE_PARTICLE_POS_ID,
            MOZC_MONO_NON_INDEPENDENT_NOUN_POS_ID,
        ]);
    }
    if right_context.starts_with("ため") {
        return Some(&[
            MOZC_TAME_NON_INDEPENDENT_NOUN_POS_ID,
            MOZC_TAME_ADVERBIAL_NOUN_POS_ID,
        ]);
    }
    if right_context.starts_with("ので") {
        return Some(&[MOZC_NODE_CONNECTIVE_PARTICLE_POS_ID]);
    }
    right_context.starts_with("よう").then_some(&[
        MOZC_YOU_GENERAL_SUFFIX_NOUN_POS_ID,
        MOZC_YOU_AUXILIARY_STEM_NOUN_POS_ID,
    ])
}

fn document_right_particle_left_ids(right_context: &str) -> Option<&'static [u16]> {
    // Keep this table narrower than the full particle inventory. Common forms
    // such as の, が, and と also follow verbs and names often enough that a
    // connection-only promotion can reverse otherwise correct homophones.
    if starts_with_copula(right_context) || starts_with_de_connective_continuation(right_context) {
        return None;
    }
    if right_context.starts_with("まで") {
        return Some(&[314]);
    }
    match right_context.chars().next()? {
        'で' => Some(&[168, 349, 370, 420]),
        _ => None,
    }
}

fn starts_with_suru_inflection(right_context: &str) -> bool {
    if right_context.starts_with("する") {
        return true;
    }
    let Some(remainder) = right_context.strip_prefix('し') else {
        return false;
    };
    match remainder.chars().next() {
        None | Some('た' | 'て') => true,
        Some('な') => ["ない", "なか", "なく", "ながら"]
            .iter()
            .any(|prefix| remainder.starts_with(prefix)),
        Some('ま') => ["ます", "まし", "ませ"]
            .iter()
            .any(|prefix| remainder.starts_with(prefix)),
        Some('よ') => remainder.starts_with("よう"),
        Some('つ') => remainder.starts_with("つつ"),
        Some(character) => {
            character.is_whitespace()
                || matches!(
                    character,
                    '、' | '。' | '，' | '．' | ',' | '.' | '!' | '?' | '！' | '？'
                )
        }
    }
}

fn document_right_grammar_pos_id(right_context: &str) -> Option<u16> {
    if right_context.starts_with("たい") || starts_with_copula(right_context) {
        return None;
    }
    if right_context.starts_with("られ") {
        return Some(MOZC_PASSIVE_RARERU_CONTINUATIVE_POS_ID);
    }
    if right_context.starts_with("させ") {
        return Some(MOZC_CAUSATIVE_SASERU_CONTINUATIVE_POS_ID);
    }
    if ["せた", "せて", "せる", "せない", "せられ"]
        .iter()
        .any(|prefix| right_context.starts_with(prefix))
    {
        return Some(MOZC_CAUSATIVE_SERU_CONTINUATIVE_POS_ID);
    }
    if ["れた", "れて", "れる", "れない"]
        .iter()
        .any(|prefix| right_context.starts_with(prefix))
    {
        return Some(MOZC_PASSIVE_RERU_CONTINUATIVE_POS_ID);
    }
    if right_context.starts_with("ない") {
        return Some(MOZC_NEGATIVE_AUXILIARY_POS_ID);
    }
    if starts_with_polite_auxiliary(right_context) {
        return Some(MOZC_POLITE_AUXILIARY_POS_ID);
    }
    if right_context.starts_with('て') {
        return Some(MOZC_TE_CONNECTIVE_PARTICLE_POS_ID);
    }
    if starts_with_de_connective_continuation(right_context) {
        return Some(MOZC_DE_CONNECTIVE_PARTICLE_POS_ID);
    }
    right_context
        .starts_with('た')
        .then_some(MOZC_PAST_AUXILIARY_POS_ID)
}

fn looks_like_inflected_kanji_surface(surface: &str) -> bool {
    surface
        .chars()
        .last()
        .is_some_and(|character| matches!(character, '\u{3041}'..='\u{3096}'))
        && surface
            .chars()
            .any(|character| matches!(character, '\u{3400}'..='\u{9fff}'))
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
    numeric: bool,
}

const NUMERIC_INTERIOR_DICTIONARY_COST_CEILING: i32 = 6_000;
const NUMERIC_START_UNPROTECTED: u8 = 1;
const NUMERIC_START_PROTECTED: u8 = 2;

fn numeric_start_states(reading: &str, entries_by_start: &[Vec<SyntheticEntry<'_>>]) -> Vec<u8> {
    let mut states = vec![0; reading.len() + 1];
    for (start, entries) in entries_by_start.iter().enumerate() {
        if entries.iter().any(|entry| {
            entry.numeric
                && is_risky_numeric_start(&reading[start..entry.end])
                && !is_strong_sokuon_number(&reading[start..entry.end])
        }) {
            states[start] = NUMERIC_START_UNPROTECTED;
        }
    }
    states
}

fn protect_numeric_starts_inside_dictionary_entry(
    reading: &str,
    start: usize,
    relative_end: usize,
    entry: EntryView<'_>,
    states: &mut [u8],
) {
    if entry.word_cost > NUMERIC_INTERIOR_DICTIONARY_COST_CEILING {
        return;
    }
    let entry_reading = &reading[start..start + relative_end];
    let protect_entry_start = states[start] == NUMERIC_START_UNPROTECTED
        && ["し", "く"]
            .iter()
            .any(|prefix| entry_reading.starts_with(prefix))
        && entry_reading.chars().take(2).count() == 2;
    let has_protectable_interior =
        entry_reading
            .char_indices()
            .skip(1)
            .any(|(interior, character)| {
                let is_final_ku =
                    interior + character.len_utf8() == entry_reading.len() && character == 'く';
                !is_final_ku && states[start + interior] == NUMERIC_START_UNPROTECTED
            });
    if (!protect_entry_start && !has_protectable_interior)
        || !entry.surface.chars().any(|character| {
            matches!(
                character,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            )
        })
    {
        return;
    }
    if protect_entry_start {
        states[start] = NUMERIC_START_PROTECTED;
    }
    for (interior, character) in entry_reading.char_indices().skip(1) {
        if interior + character.len_utf8() == entry_reading.len() && character == 'く' {
            continue;
        }
        if states[start + interior] == NUMERIC_START_UNPROTECTED {
            states[start + interior] = NUMERIC_START_PROTECTED;
        }
    }
}

fn numeric_interior_dictionary_penalty() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| tuning_parameter("SLIME_NUMERIC_INTERIOR_DICTIONARY_PENALTY", 750))
}

fn guarded_cost(entry: &SyntheticEntry<'_>, protected_start: bool) -> i32 {
    let penalty = if entry.numeric && protected_start {
        numeric_interior_dictionary_penalty()
    } else {
        0
    };
    entry.cost.saturating_add(penalty)
}

fn is_risky_numeric_start(reading: &str) -> bool {
    ["に", "し", "ご", "く", "せん", "ぜん"]
        .iter()
        .any(|prefix| reading.starts_with(prefix))
}

fn is_strong_sokuon_number(reading: &str) -> bool {
    ["いっ", "はっ", "ろっ"].iter().any(|prefix| {
        reading.strip_prefix(prefix).is_some_and(|remainder| {
            [
                "じゅう",
                "じゅっ",
                "ひゃく",
                "びゃく",
                "ぴゃく",
                "せん",
                "ぜん",
            ]
            .iter()
            .any(|unit| remainder.starts_with(unit))
        })
    })
}

fn synthetic_entries_by_start<'a>(
    dictionary: &Dictionary,
    reading: &'a str,
    arena: &'a Bump,
    katakana_character_cost: i32,
) -> Vec<Vec<SyntheticEntry<'a>>> {
    let mut by_start: Vec<Vec<SyntheticEntry>> = (0..=reading.len()).map(|_| Vec::new()).collect();
    let has_measurement_reading = reading.contains("めーとる");
    for (start, _) in reading.char_indices() {
        let numeric_prefixes = parse_kana_number_prefixes(&reading[start..]);
        push_digit_run_entry(reading, start, &mut by_start[start]);
        push_number_entries_with_prefixes(&numeric_prefixes, start, arena, &mut by_start[start]);
        if has_measurement_reading && !numeric_prefixes.is_empty() {
            push_numbered_measurement_entries(
                dictionary,
                reading,
                start,
                &numeric_prefixes,
                arena,
                &mut by_start[start],
            );
        }
        push_spoken_digit_entries(reading, start, arena, &mut by_start[start]);
        push_spoken_latin_letter_entries(reading, start, arena, &mut by_start[start]);
        push_spoken_percent_width_entry(dictionary, reading, start, arena, &mut by_start[start]);
        if ASSIMILATED_NUMERIC_PREFIXES
            .iter()
            .any(|prefix| reading[start..].starts_with(prefix.reading))
        {
            push_assimilated_counter_number_entries(
                dictionary,
                reading,
                start,
                arena,
                &mut by_start[start],
            );
        }
        if reading[start..].starts_with("ふたり") {
            push_native_two_person_entries(dictionary, start, arena, &mut by_start[start]);
        }
        push_katakana_entries(
            reading,
            start,
            arena,
            katakana_character_cost,
            &mut by_start[start],
        );
    }
    by_start
}

const MEASUREMENT_ABBREVIATION_COST: i32 = 250;
const MEASUREMENT_ABBREVIATIONS: &[(&str, &str, &str, &str)] = &[
    ("きろめーとる", "キロメートル", "km", "ｋｍ"),
    ("せんちめーとる", "センチメートル", "cm", "ｃｍ"),
    ("みりめーとる", "ミリメートル", "mm", "ｍｍ"),
    ("めーとる", "メートル", "m", "ｍ"),
];

fn push_numbered_measurement_entries<'a>(
    dictionary: &Dictionary,
    reading: &'a str,
    start: usize,
    numeric_prefixes: &[(usize, u64)],
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    let connection = ConnectionMatrix::bundled();
    for &(numeric_length, value) in numeric_prefixes {
        let remainder = &reading[start + numeric_length..];
        for &(unit_reading, unit_surface, ascii_unit, fullwidth_unit) in MEASUREMENT_ABBREVIATIONS {
            if !remainder.starts_with(unit_reading) {
                continue;
            }
            dictionary.for_each_exact(unit_reading, |entry| {
                if entry.surface != unit_surface {
                    return;
                }
                let connection_cost = connection.cost(ARABIC_NUMBER_POS_ID, entry.left_id);
                if connection_cost >= INVALID_CONNECTION_COST {
                    return;
                }
                let cost = number_cost()
                    .saturating_add(connection_cost)
                    .saturating_add(entry.word_cost)
                    .saturating_add(MEASUREMENT_ABBREVIATION_COST);
                let digits = value.to_string();
                let fullwidth_digits = to_fullwidth_digits(&digits);
                for (digits, unit, variant_cost) in [
                    (digits.as_str(), ascii_unit, cost),
                    (
                        fullwidth_digits.as_str(),
                        fullwidth_unit,
                        cost.saturating_add(NUMBER_VARIANT_STEP),
                    ),
                ] {
                    let mut surface =
                        BumpString::with_capacity_in(digits.len() + unit.len(), arena);
                    surface.push_str(digits);
                    surface.push_str(unit);
                    out.push(SyntheticEntry {
                        end: start + numeric_length + unit_reading.len(),
                        surface: arena.alloc_str(surface.as_str()),
                        left_id: ARABIC_NUMBER_POS_ID,
                        right_id: entry.right_id,
                        cost: variant_cost,
                        numeric: true,
                    });
                }
            });
        }
    }
}

fn push_native_two_person_entries<'a>(
    dictionary: &Dictionary,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    dictionary.for_each_exact("ふたり", |entry| {
        if entry.surface != "二人" {
            return;
        }
        for (surface, variant_cost) in [
            ("2人", entry.word_cost.saturating_add(NUMBER_VARIANT_STEP)),
            (
                "２人",
                entry.word_cost.saturating_add(2 * NUMBER_VARIANT_STEP),
            ),
        ] {
            out.push(SyntheticEntry {
                end: start + "ふたり".len(),
                surface: arena.alloc_str(surface),
                left_id: entry.left_id,
                right_id: entry.right_id,
                cost: variant_cost,
                numeric: true,
            });
        }
    });
}

fn push_assimilated_counter_number_entries<'a>(
    dictionary: &Dictionary,
    reading: &'a str,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    let Some((numeric, counter_reading)) = assimilated_numeric_prefix(&reading[start..]) else {
        return;
    };
    if counter_reading.is_empty() {
        return;
    }
    let connection = ConnectionMatrix::bundled();
    let prefix_length = reading[start..].len() - counter_reading.len();
    for &counter_prefix in ASSIMILATED_NUMERIC_COUNTER_READINGS {
        if !counter_reading.starts_with(counter_prefix) {
            continue;
        }
        dictionary.for_each_exact(counter_prefix, |entry| {
            if !is_assimilated_numeric_suffix(entry.surface) {
                return;
            }
            let counter_connection = connection.cost(ARABIC_NUMBER_POS_ID, entry.left_id);
            if counter_connection >= INVALID_CONNECTION_COST {
                return;
            }
            let cost = number_cost()
                .saturating_add(counter_connection)
                .saturating_add(entry.word_cost);
            for (digit, variant_cost) in [
                (numeric.ascii, cost),
                (numeric.fullwidth, cost.saturating_add(NUMBER_VARIANT_STEP)),
            ] {
                let mut surface =
                    BumpString::with_capacity_in(digit.len() + entry.surface.len(), arena);
                surface.push_str(digit);
                surface.push_str(entry.surface);
                out.push(SyntheticEntry {
                    end: start + prefix_length + counter_prefix.len(),
                    surface: arena.alloc_str(surface.as_str()),
                    left_id: ARABIC_NUMBER_POS_ID,
                    right_id: entry.right_id,
                    cost: variant_cost,
                    numeric: true,
                });
            }
        });
    }
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
        numeric: true,
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
    ("きゅー", NumberToken::Digit(9)),
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
    ("じっ", NumberToken::Small(10)),
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

const RISKY_SINGLE_NUMBER_READINGS: &[&str] = &["に", "し", "ご", "く", "ぜん", "じゅっ", "じっ"];

#[derive(Clone, Copy)]
struct AssimilatedNumericPrefix {
    reading: &'static str,
    ascii: &'static str,
    fullwidth: &'static str,
}

const ASSIMILATED_NUMERIC_PREFIXES: &[AssimilatedNumericPrefix] = &[
    AssimilatedNumericPrefix {
        reading: "いっ",
        ascii: "1",
        fullwidth: "１",
    },
    AssimilatedNumericPrefix {
        reading: "ろっ",
        ascii: "6",
        fullwidth: "６",
    },
    AssimilatedNumericPrefix {
        reading: "はっ",
        ascii: "8",
        fullwidth: "８",
    },
];

const ASSIMILATED_NUMERIC_COUNTER_READINGS: &[&str] = &[
    "かい",
    "かこく",
    "かしょ",
    "こ",
    "けん",
    "き",
    "ぽん",
    "ぴき",
    "ぷん",
    "さつ",
    "せき",
    "そく",
    "たい",
    "ちゃく",
    "とう",
    "ぱい",
    "さい",
    "せん",
    "しょう",
    "きょく",
    "く",
    "ちょう",
    "つう",
    "てん",
];

fn assimilated_numeric_prefix(reading: &str) -> Option<(AssimilatedNumericPrefix, &str)> {
    ASSIMILATED_NUMERIC_PREFIXES.iter().find_map(|prefix| {
        reading
            .strip_prefix(prefix.reading)
            .map(|rest| (*prefix, rest))
    })
}

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
                // A leading zero cannot multiply a positional unit either:
                // ぜろせん is a lexical compound, not the quantity 1,000.
                if pending_digits > 1
                    || (pending_digits > 0 && pending == 0)
                    || pending >= 10
                    || unit >= last_small_unit
                {
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

        // `じっ` is a common pronunciation of ten in compounds such as
        // `さんじっかい` and before the assimilated minute counter
        // (`じっぷん`). It is also the beginning of ordinary words such as
        // `じっけん` and `じっこう`, so never expose a standalone numeric
        // node except at that unambiguous counter boundary.
        let single_sokuon_ten_before_minutes =
            token_count == 1 && first_token == "じっ" && suffix[consumed..].starts_with("ぷん");
        let single_and_risky = token_count == 1
            && RISKY_SINGLE_NUMBER_READINGS.contains(&first_token)
            && !single_sokuon_ten_before_minutes;
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
    let prefixes = parse_kana_number_prefixes(&reading[start..]);
    push_number_entries_with_prefixes(&prefixes, start, arena, out);
}

fn push_number_entries_with_prefixes<'a>(
    prefixes: &[(usize, u64)],
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    for &(length, value) in prefixes {
        let arabic = value.to_string();
        if let Some(mixed) = mixed_numeral(value) {
            out.push(SyntheticEntry {
                end: start + length,
                surface: arena.alloc_str(&mixed),
                left_id: ARABIC_NUMBER_POS_ID,
                right_id: ARABIC_NUMBER_POS_ID,
                cost: number_cost() - NUMBER_VARIANT_STEP,
                numeric: true,
            });
        }
        out.push(SyntheticEntry {
            end: start + length,
            surface: arena.alloc_str(&to_fullwidth_digits(&arabic)),
            left_id: ARABIC_NUMBER_POS_ID,
            right_id: ARABIC_NUMBER_POS_ID,
            cost: number_cost() + NUMBER_VARIANT_STEP,
            numeric: true,
        });
        out.push(SyntheticEntry {
            end: start + length,
            surface: arena.alloc_str(&kanji_numeral(value)),
            left_id: KANJI_NUMBER_POS_ID,
            right_id: KANJI_NUMBER_POS_ID,
            cost: number_cost() + 2 * NUMBER_VARIANT_STEP,
            numeric: true,
        });
        out.push(SyntheticEntry {
            end: start + length,
            surface: arena.alloc_str(&arabic),
            left_id: ARABIC_NUMBER_POS_ID,
            right_id: ARABIC_NUMBER_POS_ID,
            cost: number_cost(),
            numeric: true,
        });
    }
}

/// Adds readings where the speaker names each digit instead of saying a
/// quantity. The ordinary numeral parser deliberately collapses leading zeroes
/// (`ぜろぜろぜろ` -> `0`) and treats `せん` as the positional unit 1,000.
/// Codes, years, and decimal subunits need the spoken sequence to survive:
/// `ぜろぜろぜろにん` -> `000人`, `にぜろいちよねん` -> `2014年`, and
/// `ななはっせん` -> `78銭`.
///
/// Only forms that cannot be represented by the quantity parser are emitted.
/// In particular, counter-specific `よ` requires at least two preceding digits,
/// and assimilated `はっ` is accepted only immediately before `せん`. This
/// keeps ordinary `よねん` and `はっせん` on their existing dictionary and
/// quantity paths.
fn push_spoken_digit_entries<'a>(
    reading: &str,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    const MAX_SPOKEN_DIGITS: usize = 16;

    let suffix = &reading[start..];
    let mut digits = [0_u8; MAX_SPOKEN_DIGITS];
    let mut digit_count = 0_usize;
    let mut consumed = 0_usize;
    let mut special_ending = false;

    while consumed < suffix.len() && digit_count < MAX_SPOKEN_DIGITS {
        let rest = &suffix[consumed..];
        let digit = NUMBER_TOKENS.iter().find_map(|(text, token)| {
            if rest.starts_with(text)
                && let NumberToken::Digit(value) = token
            {
                Some((*text, *value))
            } else {
                None
            }
        });
        if let Some((text, value)) = digit {
            digits[digit_count] = u8::try_from(value).expect("number token is one digit");
            digit_count += 1;
            consumed += text.len();
            if digit_count >= 2 && digits[0] == 0 {
                push_spoken_digit_surface(&digits[..digit_count], start + consumed, arena, out);
            }
            continue;
        }

        // Four is commonly pronounced よ before 年. Restrict this abbreviated
        // counter form to an already established multi-digit sequence.
        if digit_count >= 2
            && let Some(remainder) = rest.strip_prefix("よ")
            && remainder.starts_with("ねん")
        {
            digits[digit_count] = 4;
            digit_count += 1;
            consumed += "よ".len();
            special_ending = true;
        // In digit-by-digit financial readings, 八 before 銭 can assimilate to
        // はっ. It must follow another spoken digit; standalone はっせん keeps
        // its established 8,000 interpretation.
        } else if digit_count >= 1
            && let Some(remainder) = rest.strip_prefix("はっ")
            && remainder.starts_with("せん")
        {
            digits[digit_count] = 8;
            digit_count += 1;
            consumed += "はっ".len();
            special_ending = true;
        }
        break;
    }

    if special_ending {
        push_spoken_digit_surface(&digits[..digit_count], start + consumed, arena, out);
    }
}

fn push_spoken_digit_surface<'a>(
    digits: &[u8],
    end: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    let mut ascii = [0_u8; 16];
    for (output, digit) in ascii.iter_mut().zip(digits) {
        *output = b'0' + digit;
    }
    let ascii = std::str::from_utf8(&ascii[..digits.len()]).expect("ASCII digits are valid UTF-8");
    out.push(SyntheticEntry {
        end,
        surface: arena.alloc_str(ascii),
        left_id: ARABIC_NUMBER_POS_ID,
        right_id: ARABIC_NUMBER_POS_ID,
        cost: number_cost() - NUMBER_VARIANT_STEP,
        numeric: true,
    });
    let fullwidth = to_fullwidth_digits(ascii);
    out.push(SyntheticEntry {
        end,
        surface: arena.alloc_str(&fullwidth),
        left_id: ARABIC_NUMBER_POS_ID,
        right_id: ARABIC_NUMBER_POS_ID,
        cost: number_cost(),
        numeric: true,
    });
}

const SPOKEN_LATIN_LETTERS: &[(&str, u8)] = &[
    ("だぶりゅー", b'W'),
    ("えっくす", b'X'),
    ("えいち", b'H'),
    ("じぇー", b'J'),
    ("きゅー", b'Q'),
    ("あーる", b'R'),
    ("えす", b'S'),
    ("てぃー", b'T'),
    ("ぜっと", b'Z'),
    ("でぃー", b'D'),
    ("えー", b'A'),
    ("びー", b'B'),
    ("しー", b'C'),
    ("いー", b'E'),
    ("えふ", b'F'),
    ("じー", b'G'),
    ("あい", b'I'),
    ("けー", b'K'),
    ("える", b'L'),
    ("えむ", b'M'),
    ("えぬ", b'N'),
    ("おー", b'O'),
    ("ぴー", b'P'),
    ("ゆー", b'U'),
    ("ぶい", b'V'),
    ("わい", b'Y'),
];

const SPOKEN_ENGLISH_DIGITS: &[(&str, u8)] = &[
    ("しっくす", 6),
    ("ふぁいぶ", 5),
    ("すりー", 3),
    ("ふぉー", 4),
    ("せぶん", 7),
    ("えいと", 8),
    ("ないん", 9),
    ("とぅー", 2),
    ("ぜろ", 0),
    ("わん", 1),
    ("つー", 2),
];

fn push_spoken_latin_letter_entries<'a>(
    reading: &str,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    const MAX_LETTERS: usize = 8;
    const MAX_DIGITS: usize = 4;
    const LETTER_COST: i32 = 3_000;

    let suffix = &reading[start..];
    if !suffix.chars().next().is_some_and(|character| {
        matches!(
            character,
            'あ' | 'い'
                | 'え'
                | 'お'
                | 'き'
                | 'け'
                | 'し'
                | 'じ'
                | 'ぜ'
                | 'だ'
                | 'て'
                | 'で'
                | 'び'
                | 'ぴ'
                | 'ぶ'
                | 'ゆ'
                | 'わ'
        )
    }) {
        return;
    }
    let mut surface = BumpString::with_capacity_in(MAX_LETTERS + MAX_DIGITS, arena);
    let mut consumed = 0_usize;
    while surface.len() < MAX_LETTERS {
        let rest = &suffix[consumed..];
        let Some(&(letter_reading, letter)) = SPOKEN_LATIN_LETTERS
            .iter()
            .find(|(letter_reading, _)| rest.starts_with(letter_reading))
        else {
            break;
        };
        surface.push(char::from(letter));
        consumed += letter_reading.len();
        if surface.len() >= 2 {
            out.push(SyntheticEntry {
                end: start + consumed,
                surface: arena.alloc_str(surface.as_str()),
                left_id: UNKNOWN_POS_ID,
                right_id: UNKNOWN_POS_ID,
                cost: katakana_run_base_cost()
                    .saturating_add(LETTER_COST.saturating_mul(
                        i32::try_from(surface.len()).expect("letter run fits i32"),
                    )),
                numeric: false,
            });
        }
    }

    if surface.len() < 2 {
        return;
    }
    let digit_start = consumed;
    let mut digit_count = 0_usize;
    while digit_count < MAX_DIGITS {
        let rest = &suffix[consumed..];
        let Some(&(digit_reading, digit)) = SPOKEN_ENGLISH_DIGITS
            .iter()
            .find(|(digit_reading, _)| rest.starts_with(digit_reading))
        else {
            break;
        };
        surface.push(char::from(b'0' + digit));
        consumed += digit_reading.len();
        digit_count += 1;
    }
    if consumed > digit_start && is_spoken_identifier_boundary(&suffix[consumed..]) {
        out.push(SyntheticEntry {
            end: start + consumed,
            surface: arena.alloc_str(surface.as_str()),
            left_id: UNKNOWN_POS_ID,
            right_id: UNKNOWN_POS_ID,
            cost: katakana_run_base_cost().saturating_add(
                LETTER_COST
                    .saturating_mul(i32::try_from(surface.len()).expect("identifier fits i32")),
            ),
            numeric: false,
        });
    }
}

fn is_spoken_identifier_boundary(rest: &str) -> bool {
    rest.is_empty()
        || [
            "から",
            "まで",
            "より",
            "として",
            "では",
            "には",
            "なら",
            "は",
            "が",
            "を",
            "に",
            "で",
            "と",
            "の",
            "へ",
            "も",
            "や",
        ]
        .iter()
        .any(|particle| rest.starts_with(particle))
}

fn push_spoken_percent_width_entry<'a>(
    dictionary: &Dictionary,
    reading: &str,
    start: usize,
    arena: &'a Bump,
    out: &mut Vec<SyntheticEntry<'a>>,
) {
    const PERCENT_READING: &str = "ぱーせんと";
    if !reading[start..].starts_with(PERCENT_READING) {
        return;
    }
    dictionary.for_each_exact(PERCENT_READING, |entry| {
        if entry.surface == "%" {
            out.push(SyntheticEntry {
                end: start + PERCENT_READING.len(),
                surface: arena.alloc_str("％"),
                left_id: entry.left_id,
                right_id: entry.right_id,
                cost: entry.word_cost.saturating_add(NUMBER_VARIANT_STEP),
                numeric: false,
            });
        }
    });
}

fn is_katakana_run_character(character: char) -> bool {
    matches!(character, 'ぁ'..='ゖ' | 'ー')
}

fn push_katakana_entries<'a>(
    reading: &str,
    start: usize,
    arena: &'a Bump,
    character_cost: i32,
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
                    + character_cost * i32::try_from(characters).expect("run length fits i32"),
                numeric: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ARABIC_NUMBER_POS_ID, Candidate, CandidateRanker, ConnectionCostCache, ConnectionMatrix,
        Conversion, Dictionary, DictionaryEntry, DictionaryLayer, MOZC_PERSONAL_GIVEN_NAME_POS_ID,
        MOZC_PERSONAL_SURNAME_POS_ID, MOZC_REGION_POS_IDS, MOZC_VERBAL_NOUN_POS_ID, NBestBucket,
        NBestNode, NUMERIC_START_PROTECTED, SyntheticEntry, UNKNOWN_POS_ID,
        document_context_ends_with_honorific_prefix, document_region_suffix_promotion,
        guarded_cost, insert_n_best_node, numeric_interior_dictionary_penalty,
        numeric_start_states, orthographic_long_vowel_variants, synthetic_entries_by_start,
        trailing_numeric_surface,
    };
    use crate::pronunciation::{
        MAX_READING_CHARACTERS as LONG_VOWEL_MAX_READING_CHARACTERS,
        MAX_VARIANTS as LONG_VOWEL_MAX_VARIANTS,
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
                    matched_prefix_bytes: 0,
                    total_cost,
                },
                None,
                1,
            );
        }

        assert_eq!(bucket.states.len(), 8);
        assert_eq!(bucket.worst_position, 7);
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
                matched_prefix_bytes: 0,
                total_cost: 80,
            },
            None,
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
                matched_prefix_bytes: 0,
                total_cost: 5,
            },
            None,
            1,
        );
        assert_eq!(bucket.worst_position, 6);
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
                matched_prefix_bytes: 0,
                total_cost: 60,
            },
            None,
            1,
        );
        assert_eq!(bucket.states.len(), 8);
        assert_eq!(bucket.worst_position, 6);
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
    fn personal_name_recall_combines_a_surname_with_deep_given_names() {
        let mut entries = vec![DictionaryEntry::with_pos(
            "やまだ",
            "山田",
            MOZC_PERSONAL_SURNAME_POS_ID,
            MOZC_PERSONAL_SURNAME_POS_ID,
            100,
        )];
        entries.extend((0_i32..48).map(|index| {
            DictionaryEntry::with_pos(
                "ふかな",
                format!("候補{index:02}"),
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                index,
            )
        }));
        entries.push(DictionaryEntry::with_pos(
            "ふかな",
            "深名",
            MOZC_PERSONAL_GIVEN_NAME_POS_ID,
            MOZC_PERSONAL_GIVEN_NAME_POS_ID,
            5_000,
        ));
        let dictionary = Dictionary::new(entries);

        assert!(
            dictionary
                .compound_candidates("やまだふかな", 8, 64)
                .iter()
                .all(|candidate| candidate.surface != "山田深名")
        );
        let personal_names = dictionary.personal_name_candidates("やまだふかな", 64, 64);
        assert!(
            personal_names
                .iter()
                .any(|candidate| candidate.surface == "山田深名")
        );
        assert!(personal_names.len() <= 64);
    }

    #[test]
    fn exact_personal_name_change_detection_is_segment_local() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos(
                "かたせしま",
                "片瀬志麻",
                MOZC_PERSONAL_SURNAME_POS_ID,
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                10,
            ),
            DictionaryEntry::with_pos(
                "かたせ",
                "片瀬",
                MOZC_PERSONAL_SURNAME_POS_ID,
                MOZC_PERSONAL_SURNAME_POS_ID,
                20,
            ),
            DictionaryEntry::with_pos(
                "しま",
                "志摩",
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                20,
            ),
            DictionaryEntry::new("かてい", "課程", 10),
            DictionaryEntry::new("かてい", "過程", 20),
            DictionaryEntry::new("ふ", "不", 10),
            DictionaryEntry::with_pos(
                "ちゅう",
                "忠",
                MOZC_PERSONAL_SURNAME_POS_ID,
                MOZC_PERSONAL_SURNAME_POS_ID,
                20,
            ),
            DictionaryEntry::with_pos(
                "ちゅう",
                "忠",
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                MOZC_PERSONAL_GIVEN_NAME_POS_ID,
                20,
            ),
            DictionaryEntry::new("しゃ", "社", 10),
            DictionaryEntry::new("しゃ", "者", 20),
        ]);

        assert!(dictionary.changes_exact_personal_name_segment(
            "かたせしまかてい",
            "片瀬志麻課程",
            "片瀬志摩課程",
        ));
        assert!(dictionary.changes_exact_personal_name_segment(
            "かたせしまかてい",
            "片瀬志麻課程",
            "片瀬志課程",
        ));
        assert!(dictionary.changes_exact_personal_name_segment(
            "かたせしまかてい",
            "片瀬志麻課程",
            "片瀬志の麻課程",
        ));
        assert!(!dictionary.changes_exact_personal_name_segment(
            "かたせしまかてい",
            "片瀬志麻課程",
            "片瀬志麻過程",
        ));
        assert!(!dictionary.changes_exact_personal_name_segment(
            "かたせしまかてい",
            "片瀬志麻課程",
            "片瀬志麻の課程",
        ));
        assert!(!dictionary.changes_exact_personal_name_segment(
            "ふちゅうしゃ",
            "不忠社",
            "不忠者",
        ));
        assert!(dictionary.is_exact_full_personal_name_surface("かたせしま", "片瀬志麻"));
        assert!(dictionary.is_exact_full_personal_name_surface("かたせしま", "片瀬志摩"));
        assert!(
            !dictionary.is_exact_full_personal_name_surface("かたせしまかてい", "片瀬志麻課程",)
        );
        assert!(!dictionary.is_exact_full_personal_name_surface("ふちゅう", "不忠"));
    }

    #[test]
    fn exact_region_change_detection_is_specific_and_segment_local() {
        let region_pos_id = MOZC_REGION_POS_IDS[0];
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("くるみだて", "胡桃舘", region_pos_id, region_pos_id, 10),
            DictionaryEntry::new("にちかい", "に近い", 10),
            DictionaryEntry::with_pos("かんさい", "関西", region_pos_id, region_pos_id, 10),
            DictionaryEntry::new("ちいき", "地域", 10),
            DictionaryEntry::with_pos("たちあらい", "大刀洗", region_pos_id, region_pos_id, 10),
            DictionaryEntry::with_pos("たちあらい", "太刀洗", region_pos_id, region_pos_id, 20),
        ]);

        assert!(dictionary.changes_exact_personal_name_or_region_segment(
            "くるみだてにちかい",
            "胡桃舘に近い",
            "くるみだてに近い",
        ));
        assert!(!dictionary.changes_exact_personal_name_or_region_segment(
            "くるみだてにちかい",
            "胡桃舘に近い",
            "胡桃舘にちかい",
        ));
        assert!(!dictionary.changes_exact_personal_name_or_region_segment(
            "かんさいちいき",
            "関西地域",
            "かんさい地域",
        ));
        assert!(!dictionary.changes_exact_personal_name_or_region_segment(
            "たちあらい",
            "大刀洗",
            "太刀洗",
        ));
    }

    #[test]
    fn exact_katakana_segment_rejects_only_mixed_script_fragmentation() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あるごる", "アルゴル", 10),
            DictionaryEntry::new("たいようけい", "太陽系", 10),
            DictionaryEntry::new("そふと", "ソフト", 10),
            DictionaryEntry::new("はむ", "ハム", 10),
        ]);

        assert!(dictionary.fragments_exact_katakana_segment(
            "あるごるたいようけい",
            "アルゴル太陽系",
            "あるゴル太陽系",
        ));
        assert!(!dictionary.fragments_exact_katakana_segment(
            "あるごるたいようけい",
            "アルゴル太陽系",
            "アルゴル太陽けい",
        ));
        assert!(!dictionary.fragments_exact_katakana_segment(
            "あるごるたいようけい",
            "アルゴル太陽系",
            "あるゴル太陽けい",
        ));
        assert!(!dictionary.fragments_exact_katakana_segment("あるごる", "アルゴル", "あるごる",));
        assert!(!dictionary.fragments_exact_katakana_segment("そふと", "ソフト", "そふと",));
        assert!(!dictionary.fragments_exact_katakana_segment("はむ", "ハム", "はム"));
    }

    #[test]
    fn exact_ideographic_segment_distinguishes_hiragana_words_from_fragments() {
        let dictionary = Dictionary::bundled();

        assert!(
            dictionary.fragments_exact_ideographic_segment_into_hiragana(
                "なくなったためあに",
                "亡くなったため兄",
                "なくなったため兄",
            )
        );
        assert!(
            !dictionary.fragments_exact_ideographic_segment_into_hiragana(
                "いういみあい",
                "言う意味合い",
                "いう意味合い",
            )
        );
        assert!(
            !dictionary.fragments_exact_ideographic_segment_into_hiragana(
                "にはねてくる",
                "には寝てくる",
                "に跳ねてくる",
            )
        );
    }

    #[test]
    fn exact_right_phrase_continuation_uses_a_bounded_surface_suffix() {
        let dictionary = Dictionary::bundled();

        assert!(dictionary.has_exact_right_phrase_continuation(
            "どーろとしん",
            "道路と新",
            "湘南バイパスは、終日5割引。",
        ));
        assert!(!dictionary.has_exact_right_phrase_continuation(
            "どーろとしん",
            "道路都心",
            "湘南バイパスは、終日5割引。",
        ));
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
    fn pronunciation_style_long_marks_recover_orthographic_dictionary_paths() {
        let dictionary = Dictionary::bundled();
        for (reading, expected) in [
            ("ちゅーごく", "中国"),
            ("しょとー", "諸島"),
            ("ほのー", "炎"),
            ("こーてー", "皇帝"),
        ] {
            let conversions = dictionary.convert_n_best(reading, 20);
            let conversion = conversions
                .iter()
                .find(|conversion| conversion.surface == expected)
                .unwrap_or_else(|| panic!("missing {reading} -> {expected}: {conversions:?}"));
            assert_eq!(
                conversion
                    .segments
                    .iter()
                    .map(|segment| segment.reading.as_str())
                    .collect::<String>(),
                reading
            );
        }
    }

    #[test]
    fn exact_foreign_words_outrank_orthographic_long_vowel_variants() {
        let dictionary = Dictionary::bundled();
        for (reading, expected) in [
            ("らーめん", "ラーメン"),
            ("ぱふぉーまんす", "パフォーマンス"),
            ("こんぴゅーたー", "コンピューター"),
            ("ぐれーど", "グレード"),
        ] {
            assert_eq!(dictionary.convert_best(reading).unwrap().surface, expected);
            assert_eq!(dictionary.candidates(reading)[0].surface, expected);
        }
    }

    #[test]
    fn pronunciation_variants_do_not_rewrite_foreign_katakana_spans() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("とう", "トウ", 10),
            DictionaryEntry::new("とらんす", "トランス", 10),
            DictionaryEntry::new("こうか", "効果", 10),
            DictionaryEntry::new("しあ", "シア", 10),
            DictionaryEntry::new("にゅーす", "ニュース", 10),
            DictionaryEntry::new("ぎょうかい", "業界", 10),
        ]);

        assert!(
            dictionary
                .convert_n_best("とー", 10)
                .iter()
                .all(|conversion| conversion.surface != "トウ")
        );
        assert!(
            dictionary
                .convert_n_best("とらんすこーかしあ", 20)
                .iter()
                .all(|conversion| conversion.surface != "トランス効果シア")
        );
        assert!(
            dictionary
                .convert_n_best("にゅーすぎょーかい", 20)
                .iter()
                .any(|conversion| conversion.surface == "ニュース業界")
        );
    }

    #[test]
    fn surface_prefix_search_can_use_an_orthographic_long_vowel_variant() {
        let dictionary = Dictionary::bundled();
        let conversions = dictionary.convert_n_best_with_surface_prefix("ちゅーごく", "中国", 5);

        assert_eq!(conversions[0].surface, "中国");
        assert_eq!(conversions[0].segments[0].reading, "ちゅーごく");
    }

    #[test]
    fn pronunciation_compounds_recover_safe_low_rank_paths() {
        let dictionary = Dictionary::bundled();
        assert_eq!(
            dictionary.compound_candidates("あさいり", 8, 32),
            dictionary.compound_candidates_exact("あさいり", 8, 32, &[]),
            "readings without long marks must retain the exact compound path",
        );
        assert!(
            dictionary
                .compound_candidates("こーてーけん", 8, 32)
                .iter()
                .any(|candidate| candidate.surface == "皇帝兼")
        );
        assert!(
            dictionary
                .compound_candidates("とらんすこーかしあ", 8, 32)
                .iter()
                .all(|candidate| candidate.surface != "トランス効果シア")
        );

        for (reading, expected) in [
            ("こーてーけん", "皇帝兼"),
            ("こーこてんき", "後古典期"),
            ("きょーがしき", "卿が指揮"),
        ] {
            assert!(
                dictionary
                    .convert_n_best_with_surface_prefix(reading, expected, 1)
                    .iter()
                    .any(|conversion| conversion.surface == expected),
                "missing constrained path for {reading} -> {expected}",
            );
        }
    }

    #[test]
    fn pronunciation_long_vowel_variants_are_bounded_and_keep_partial_substitutions() {
        let variants = orthographic_long_vowel_variants("めーるのせーのー");
        let readings = variants
            .iter()
            .map(|variant| variant.reading.as_str())
            .collect::<Vec<_>>();

        assert!(readings.contains(&"めーるのせいのー"));
        assert!(readings.contains(&"めいるのせーのー"));
        assert!(readings.contains(&"めーるのせーのう"));
        assert!(readings.contains(&"めーるのせいのう"));
        assert!(variants.len() <= LONG_VOWEL_MAX_VARIANTS);
        assert!(orthographic_long_vowel_variants("こーこーこーこーこー").is_empty());
        assert!(
            orthographic_long_vowel_variants(&format!(
                "{}こー",
                "あ".repeat(LONG_VOWEL_MAX_READING_CHARACTERS)
            ))
            .is_empty()
        );
    }

    #[test]
    fn pronunciation_search_keeps_unaffected_readings_on_the_exact_fast_path() {
        let dictionary = Dictionary::bundled();
        let overlong = format!("{}こー", "あ".repeat(LONG_VOWEL_MAX_READING_CHARACTERS));
        for reading in ["わたしはにほん", overlong.as_str()] {
            assert_eq!(
                dictionary.convert_n_best(reading, 20),
                dictionary.convert_n_best_exact(reading, 20)
            );
        }
    }

    #[test]
    fn surface_prefix_constraint_prunes_cheaper_incompatible_paths() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あ", "亜", 10),
            DictionaryEntry::new("あ", "阿", 20),
            DictionaryEntry::new("い", "伊", 10),
        ]);

        let conversions = dictionary.convert_n_best_with_surface_prefix("あい", "阿", 5);

        assert_eq!(conversions[0].surface, "阿伊");
        assert!(
            conversions
                .iter()
                .all(|conversion| conversion.surface.starts_with("阿"))
        );
    }

    #[test]
    fn surface_prefix_constraint_can_end_inside_a_dictionary_surface() {
        let dictionary = Dictionary::new(vec![DictionaryEntry::new("あい", "愛情", 10)]);

        let conversions = dictionary.convert_n_best_with_surface_prefix("あい", "愛", 5);

        assert_eq!(conversions[0].surface, "愛情");
        assert!(
            dictionary
                .convert_n_best_with_surface_prefix("あい", "不一致", 5)
                .is_empty()
        );
    }

    #[test]
    fn connected_surface_prefix_constraint_keeps_only_matching_paths() {
        let dictionary = Dictionary::bundled();

        let conversions = dictionary.convert_n_best_with_surface_prefix("はしでたべる", "箸", 10);

        assert!(
            conversions
                .iter()
                .any(|conversion| conversion.surface == "箸で食べる")
        );
        assert!(
            conversions
                .iter()
                .all(|conversion| conversion.surface.starts_with("箸"))
        );
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
    fn document_context_reuses_a_repeated_multi_character_surface() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あさの", "浅野", 2_500),
            DictionaryEntry::new("あさ", "朝", 0),
            DictionaryEntry::new("の", "の", 0),
        ]);

        assert_eq!(dictionary.candidates("あさの")[0].surface, "朝の");
        assert_eq!(
            dictionary.candidates_with_context("あさの", "浅野木材工業の")[0].surface,
            "浅野"
        );
        assert_eq!(
            dictionary.candidates_with_context("あさの", "無関係な文脈")[0].surface,
            "朝の"
        );
        assert_eq!(
            dictionary.candidates("あさの")[0].surface,
            "朝の",
            "a transient query must not be retained by the dictionary"
        );
    }

    #[test]
    fn document_context_reuses_a_repeated_tail_segment_in_a_long_conversion() {
        let dictionary = Dictionary::bundled();
        let reading = "、そしてしょきがいんようする";

        assert_eq!(
            dictionary.candidates_with_context(reading, "漢城陥落は『三国史記』と『日本書紀』")[0]
                .surface,
            "、そして書紀が引用する"
        );
        assert_eq!(
            dictionary.candidates_with_context(reading, "無関係な文脈")[0].surface,
            "、そして初期が引用する"
        );
    }

    #[test]
    fn document_context_promotes_a_dictionary_backed_phrase_continuation() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("こ")[0].surface, "個");
        assert_ne!(dictionary.candidates("けん")[0].surface, "券");
        assert_eq!(
            dictionary.candidates_with_context("けん", "大勢の信者が傍聴")[0].surface,
            "券"
        );
        assert_eq!(
            dictionary.candidates_with_context("こ", "胸部は格納")[0].surface,
            "庫"
        );
        assert_eq!(
            dictionary.candidates_with_context("こ", "無関係な文脈")[0].surface,
            "個"
        );
        assert_eq!(
            dictionary.candidates_with_context("し", "高木守道")[0].surface,
            "氏",
            "a one-character homographic prefix must not promote 道士"
        );
        assert_eq!(
            dictionary.candidates_with_context("かい", "もうひとつは最")[0].surface,
            "下位",
            "Mozc's dedicated superlative prefix may complete 最下位"
        );
        assert_eq!(
            dictionary.candidates_with_context("き", "大正")[0].surface,
            "期",
            "an ordinary final kanji must not reinterpret 大正 as 正気"
        );
        assert_eq!(
            dictionary.candidates_with_context("じ", "第一")[0].surface,
            "次",
            "a numeric final kanji must not reinterpret 第一 as 一時"
        );
        assert_eq!(
            dictionary.candidates_with_context("やく", "経済の牽引")[0].surface,
            "役"
        );
        assert_eq!(
            dictionary.candidates_with_context("ひ", "予算の調査")[0].surface,
            "費"
        );
        assert_eq!(
            dictionary.candidates_with_context("き", "循環")[0].surface,
            "器"
        );
        assert_eq!(
            dictionary.candidates_with_context("かん", "価値")[0].surface,
            "観"
        );
        assert_eq!(
            dictionary.candidates_with_context("せん", "宇宙")[0].surface,
            "船"
        );
        assert_eq!(
            dictionary.candidates_with_context("ひろし", "渡辺")[0].surface,
            "博"
        );
        assert_eq!(
            dictionary.candidates_with_context("たい", "ナスタアリーク")[0].surface,
            "体"
        );
        assert_eq!(
            dictionary.candidates_with_context("そ", "線形作用")[0].surface,
            "素"
        );
        assert_ne!(
            dictionary.candidates_with_context("そ", "線形")[0].surface,
            "素"
        );
        assert_eq!(
            dictionary.candidates_with_context("おき", "太平洋の三陸")[0].surface,
            "沖"
        );
        assert_ne!(
            dictionary.candidates_with_context("おき", "太平洋")[0].surface,
            "沖"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("くん", "叙正三位", "一等授瑞宝章。",)
                [0]
            .surface,
            "勲"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context("くん", "叙正三位", "を授与した")[0]
                .surface,
            "勲"
        );
        assert_eq!(
            dictionary.candidates_with_context("しかい", "多数の番組の")[0].surface,
            "司会"
        );
        assert_ne!(
            dictionary.candidates_with_context("しかい", "多数の番組")[0].surface,
            "司会"
        );
    }

    #[test]
    fn document_context_strengthens_only_unique_phrase_continuations() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            ("なん", "ダドリーは資金", "の不足を補う", "難"),
            ("し", "同年12月には『三國", "』が発売された", "志"),
            ("か", "ガソリンのオクタン", "103以上", "価"),
            ("しき", "脳外科医の遠野", "の担当である", "志貴"),
        ] {
            assert_eq!(
                dictionary.candidates_with_surrounding_context(
                    reading,
                    left_context,
                    right_context,
                )[0]
                .surface,
                expected
            );
        }

        assert_eq!(
            dictionary.candidates_with_context("かん", "価値")[0].surface,
            "観",
            "competing exact phrases must retain their dictionary cost difference"
        );
        assert!(!document_context_ends_with_honorific_prefix("防御"));
        assert!(document_context_ends_with_honorific_prefix("天皇の御"));
        assert_ne!(
            dictionary.candidates_with_context("かき", "防御")[0].surface,
            "垣",
            "a kanji word ending in 御 is not an honorific boundary"
        );
    }

    #[test]
    fn document_context_does_not_treat_a_person_name_as_competing_phrase_evidence() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "こ",
                "日本初のスケート競技大会となった諏訪",
                "一周スケート大会"
            )[0]
            .surface,
            "湖"
        );
        assert!(
            dictionary
                .readings_for_surface("諏訪子")
                .iter()
                .any(|reading| reading == "すわこ")
        );
    }

    #[test]
    fn strong_left_phrase_evidence_outweighs_generic_boundary_promotions() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates_with_surrounding_context("しんかんせん", "劇団☆", "所属。")[0]
                .surface,
            "新感線"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("なか", "犬猿の", "です。")[0].surface,
            "仲"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("さい", "現在は", "下等に相当する。")[0]
                .surface,
            "最",
            "right-side phrase evidence must retain its existing boundary support"
        );
    }

    #[test]
    fn document_context_promotes_katakana_general_noun_compounds() {
        let dictionary = Dictionary::bundled();

        let candidates = dictionary.candidates_with_surrounding_context(
            "きん",
            "米国でリステリア",
            "が繁殖した。",
        );
        assert_eq!(candidates[0].surface, "菌");
    }

    #[test]
    fn document_context_promotes_katakana_ideographic_tail_compounds() {
        let dictionary = Dictionary::bundled();

        assert_ne!(dictionary.candidates("か")[0].surface, "家");
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "か",
                "各地のアマチュア天文",
                "が観測した。",
            )[0]
            .surface,
            "家"
        );
    }

    #[test]
    fn document_context_promotes_honorific_noun_phrase_continuations() {
        let dictionary = Dictionary::bundled();

        assert_ne!(dictionary.candidates("かし")[0].surface, "菓子");
        assert_eq!(
            dictionary.candidates_with_context("かし", "仏具店にはお")[0].surface,
            "菓子"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "かね",
                "被害者から騙し取ったお",
                "を含める"
            )[0]
            .surface,
            "金"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("はなし", "期待通りのお", "でした")[0]
                .surface,
            "話し",
            "a following copula must not reinterpret an inflected verb as an honorific noun"
        );
    }

    #[test]
    fn document_context_uses_connection_cost_at_a_single_word_boundary() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("いせき")[0].surface, "遺跡");
        assert_eq!(
            dictionary.candidates_with_context("いせき", "オランダへ")[0].surface,
            "移籍"
        );
        assert_eq!(dictionary.candidates("みせ")[0].surface, "見せ");
        assert_eq!(
            dictionary.candidates_with_context("みせ", "教育が残念な")[0].surface,
            "店"
        );
        assert_eq!(dictionary.candidates("いか")[0].surface, "以下");
        assert_eq!(
            dictionary.candidates_with_context("いか", "病院に")[0].surface,
            "行か"
        );
    }

    #[test]
    fn document_boundary_connection_does_not_reorder_long_conversions() {
        let dictionary = Dictionary::bundled();
        let reading = "かんじをわくんにあてているゆらい";
        let baseline = dictionary.candidates_with_limit(reading, 10);
        let contextual = dictionary.candidates_with_context_limit(reading, "「飛鳥」の", 10);

        assert_eq!(contextual, baseline);
    }

    #[test]
    fn document_context_promotes_structured_numeric_continuations() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しん", "新", 0),
            DictionaryEntry::new("しん", "進", 2_500),
            DictionaryEntry::new("か", "化", 0),
            DictionaryEntry::new("か", "日", 2_500),
            DictionaryEntry::new("にち", "日時", 0),
            DictionaryEntry::new("にち", "日", 2_500),
            DictionaryEntry::new("たい", "体", 0),
            DictionaryEntry::new("たい", "対", 2_500),
            DictionaryEntry::new("き", "機", 0),
            DictionaryEntry::new("き", "期", 2_500),
        ]);

        assert_eq!(
            dictionary.candidates_with_context("しん", "16")[0].surface,
            "進"
        );
        assert_eq!(
            dictionary.candidates_with_context("しん", "１６")[0].surface,
            "進"
        );
        assert_eq!(
            dictionary.candidates_with_context("か", "8月3")[0].surface,
            "日"
        );
        assert_eq!(
            dictionary.candidates_with_context("にち", "8月12")[0].surface,
            "日"
        );
        for (left_context, right_context) in [("1", "1"), ("１", "１"), ("一", "一")] {
            assert_eq!(
                dictionary.candidates_with_surrounding_context("たい", left_context, right_context)
                    [0]
                .surface,
                "対"
            );
        }
        assert_eq!(
            dictionary.candidates_with_surrounding_context("き", "第1", "終了後")[0].surface,
            "期"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("き", "第1", "機能を示す")[0].surface,
            "機"
        );
        for (left_context, right_context) in [
            ("相手と", "1"),
            ("1", "人"),
            ("1.5", "1"),
            ("1", "1.5"),
            ("１", "１．５"),
        ] {
            assert_eq!(
                dictionary.candidates_with_surrounding_context("たい", left_context, right_context)
                    [0]
                .surface,
                "体"
            );
        }
    }

    #[test]
    fn trailing_numeric_context_handles_a_multibyte_decimal_separator() {
        assert_eq!(
            trailing_numeric_surface("1．3"),
            trailing_numeric_surface("1.3")
        );
        assert_eq!(trailing_numeric_surface("Ｆ．Ｅ．Ａ．Ｒ．２０"), None);
    }

    #[test]
    fn document_context_rejects_invalid_numeric_continuations() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("しん", "新", 0),
            DictionaryEntry::new("しん", "進", 2_500),
            DictionaryEntry::new("か", "化", 0),
            DictionaryEntry::new("か", "日", 2_500),
            DictionaryEntry::new("にち", "日時", 0),
            DictionaryEntry::new("にち", "日", 2_500),
        ]);

        assert_eq!(
            dictionary.candidates_with_context("しん", "1")[0].surface,
            "新"
        );
        assert_eq!(
            dictionary.candidates_with_context("しん", "3")[0].surface,
            "新"
        );
        assert_eq!(
            dictionary.candidates_with_context("しん", "37")[0].surface,
            "新"
        );
        assert_eq!(
            dictionary.candidates_with_context("しん", "1.3")[0].surface,
            "新"
        );
        assert_eq!(
            dictionary.candidates_with_context("しん", "10016")[0].surface,
            "新"
        );
        assert_eq!(
            dictionary.candidates_with_context("か", "8月1")[0].surface,
            "化"
        );
        assert_eq!(
            dictionary.candidates_with_context("か", "13月3")[0].surface,
            "化"
        );
        assert_eq!(
            dictionary.candidates_with_context("にち", "8月3")[0].surface,
            "日時"
        );
        assert_eq!(
            dictionary.candidates_with_context("にち", "8月1")[0].surface,
            "日時"
        );
    }

    #[test]
    fn document_context_promotes_numeric_counter_notation() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("さい")[0].surface, "際");
        assert_eq!(
            dictionary.candidates_with_context("さい", "5")[0].surface,
            "歳"
        );
        assert_eq!(dictionary.candidates("かこく")[0].surface, "過酷");
        assert_eq!(
            dictionary.candidates_with_context("かこく", "6")[0].surface,
            "カ国"
        );
        assert_eq!(dictionary.candidates("だい")[0].surface, "第");
        assert_eq!(
            dictionary.candidates_with_context("だい", "第33")[0].surface,
            "代"
        );
        assert_eq!(
            dictionary.candidates_with_context("だい", "4")[0].surface,
            "台"
        );
        assert_eq!(
            dictionary.candidates_with_context("か", "8月3")[0].surface,
            "日"
        );
        assert_eq!(
            dictionary.candidates_with_context("だい", "1.5")[0].surface,
            "第"
        );
        assert_eq!(
            dictionary.candidates_with_context("だい", "第1.5")[0].surface,
            "第"
        );
        assert_ne!(dictionary.candidates("げん")[0].surface, "減");
        assert_eq!(
            dictionary.candidates_with_context("げん", "0.5%")[0].surface,
            "減"
        );
        assert_eq!(
            dictionary.candidates_with_context("げん", "0.5％")[0].surface,
            "減"
        );
        assert_eq!(
            dictionary.candidates_with_context("ぞう", "12%")[0].surface,
            "増"
        );
        assert_eq!(
            dictionary.candidates_with_context("ぞう", "５％")[0].surface,
            "増"
        );
        assert_eq!(
            dictionary.candidates_with_context("ぞう", "五％")[0].surface,
            "増"
        );
        assert_eq!(
            dictionary.candidates_with_context("ひき", "42")[0].surface,
            "匹"
        );
        assert_eq!(
            dictionary.candidates_with_context("ひき", "四十二")[0].surface,
            "匹"
        );
        assert_eq!(
            dictionary.candidates_with_context("ひき", "４２")[0].surface,
            "匹"
        );
        assert_eq!(
            dictionary.candidates_with_context("つい", "4")[0].surface,
            "対"
        );
        assert_eq!(
            dictionary.candidates_with_context("つい", "四")[0].surface,
            "対"
        );
        assert_ne!(dictionary.candidates("ひき")[0].surface, "匹");
        assert_ne!(
            dictionary.candidates_with_context("ひき", "1.5")[0].surface,
            "匹"
        );
        assert_ne!(
            dictionary.candidates_with_context("つい", "-4")[0].surface,
            "対"
        );
        assert_ne!(
            dictionary.candidates_with_context("げん", "-3%")[0].surface,
            "減"
        );
    }

    #[test]
    fn document_context_preserves_recent_arabic_digit_width_in_numeric_segments() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("さんるいから")[0].surface, "三塁から");
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "さんるいから",
                "桜井高校はランナー1.",
                "内野ゴロで先制します。",
            )[0]
            .surface,
            "3塁から"
        );
        assert_eq!(
            dictionary.candidates_with_context("さんるいから", "打者はランナー１．")[0].surface,
            "３塁から"
        );
        assert_eq!(
            dictionary.candidates_with_context("さんるいから", "2024年。次の打者は")[0].surface,
            "三塁から",
            "numeric style does not cross a completed sentence"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "せんだいさんきゅう",
                "地域別では田老66・0%、",
                "・6%と地域差が大きかった。",
            )[0]
            .surface,
            "仙台39",
            "a multi-digit lexical prefix is not resegmented as a number"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "らいぶらりのいちぶ",
                "Enterprise Linuxのバージョン3から搭載され、現在ではGNU C",
                "となっている。",
            )[0]
            .surface,
            "ライブラリの一部",
            "an internal numeral compound keeps its lexical spelling"
        );
        assert_eq!(
            dictionary.candidates_with_context("いちぶ", "バージョン3では")[0].surface,
            "一部",
            "a standalone lexical numeral compound is not treated as a counter"
        );
        assert_eq!(
            dictionary.candidates_with_context("きゅうしゅう", "2024年は")[0].surface,
            "九州",
            "a proper noun beginning with a numeral keeps its lexical spelling"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "おにぎりいっこ",
                "毛布にくるまって寒さをしのぎ、",
                "を家族4人で分け合った。",
            )[0]
            .surface,
            "おにぎり1個",
            "a productive internal counter inherits confirmed right-context style"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "おにぎりいっこ",
                "2人に",
                "を家族４人で分けた。",
            )[0]
            .surface,
            "おにぎり一個",
            "conflicting confirmed digit widths do not impose either style"
        );
        assert!(
            super::document_numeric_style_evidence(
                "第",
                "さんしゃのか",
                "半数、具体的には367票が必要である。",
            )
            .is_none_or(|evidence| evidence.leading.is_none()),
            "right-context digits cannot style a leading numeric interpretation"
        );
    }

    #[test]
    fn document_context_preserves_measurement_abbreviations() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates("ながさいちめーとる")[0].surface,
            "長さ1メートル"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ながさいちめーとる",
                "麺棒は直径2~3cm、",
                "程度のものが一般的だ。",
            )[0]
            .surface,
            "長さ1m"
        );
        assert_eq!(
            dictionary.candidates_with_context("ながさいちめーとる", "直径２〜３ｃｍ、")[0].surface,
            "長さ１ｍ"
        );
        assert_eq!(
            dictionary.candidates_with_context("ながさいちめーとる", "直径2cmだった。次は")[0]
                .surface,
            "長さ1メートル",
            "measurement style does not cross a completed sentence"
        );
        assert_eq!(
            dictionary.candidates_with_context("ながさいちめーとる", "version2millionでは")[0]
                .surface,
            "長さ1メートル",
            "a unit letter inside an alphanumeric word is not style evidence"
        );
    }

    #[test]
    fn document_context_promotes_win_loss_record() {
        let dictionary = Dictionary::bundled();

        for context in ["2勝2", "２勝２", "二勝二"] {
            assert_eq!(
                dictionary.candidates_with_context("はい", context)[0].surface,
                "敗"
            );
        }
        assert_ne!(
            dictionary.candidates_with_context("はい", "2")[0].surface,
            "敗"
        );
    }

    #[test]
    fn surrounding_context_promotes_structured_numeric_units() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates_with_surrounding_context("し", "さらに1", "一、二塁から")[0]
                .surface,
            "死"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("げん", "現在は7", "ギターを使う")[0]
                .surface,
            "弦"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("せき", "客船は3", "の客船で運航")[0]
                .surface,
            "隻"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("せん", "139円78", "で引けた")[0]
                .surface,
            "銭"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context("し", "さらに3", "一塁から")[0].surface,
            "死"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context("げん", "現在は7", "件を使う")[0]
                .surface,
            "弦"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context("せき", "客船は3", "の座席")[0].surface,
            "隻"
        );
    }

    #[test]
    fn document_context_promotes_dictionary_counter_pos_after_numbers() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates_with_context("だん", "3")[0].surface,
            "段"
        );
        assert_eq!(
            dictionary.candidates_with_context("わ", "3")[0].surface,
            "話"
        );
        assert_eq!(
            dictionary.candidates_with_context("るい", "1.3")[0].surface,
            "塁"
        );
        assert_eq!(
            dictionary.candidates_with_context("だい", "六")[0].surface,
            "代"
        );
        assert_eq!(
            dictionary.candidates_with_context("だい", "4")[0].surface,
            "台"
        );
        assert_eq!(
            dictionary.candidates_with_context("わ", "-3")[0].surface,
            "話"
        );
    }

    #[test]
    fn surrounding_context_promotes_an_inflected_word_before_polite_auxiliary() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("のめ")[0].surface, "の目");
        let candidates =
            dictionary.candidates_with_surrounding_context("のめ", "うまいコーヒーが", "ました。");
        assert_eq!(candidates[0].surface, "飲め", "{candidates:?}");
        for (reading, left_context, right_context, expected) in [
            (
                "ちかい",
                "問答があり文鮮明が読み上げる内容に応えて“",
                "ます”と恩恵を受ける者は",
                "誓い",
            ),
            ("あき", "二階の古美術品も見て", "ませんでした!", "飽き"),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }
        assert_eq!(
            dictionary.candidates_with_surrounding_context("ぶ", "第三", "まで")[0].surface,
            dictionary.candidates_with_context("ぶ", "第三")[0].surface,
            "まで is a particle and must not be mistaken for the polite auxiliary"
        );
    }

    #[test]
    fn surrounding_context_promotes_a_continuative_verb_before_desiderative_auxiliary() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("かい")[0].surface, "回");
        let candidates = dictionary.candidates_with_surrounding_context(
            "かい",
            "丁寧に案内してもらい、",
            "たい物が買えました。",
        );
        assert_eq!(candidates[0].surface, "買い", "{candidates:?}");
    }

    #[test]
    fn surrounding_context_uses_pos_connections_before_function_words() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            (
                "よる",
                "泉谷小学校、泉谷中学校からすぐの学校帰りに",
                "ことなども簡単です。",
                "寄る",
            ),
            (
                "とく",
                "メルダーザへかけられた呪は、マリシーユにも",
                "ことができなかった。",
                "解く",
            ),
            (
                "かよう",
                "いろいろなお店に体験に行った結果、このお店へ",
                "ことにしました。",
                "通う",
            ),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }

        let left_only = dictionary.candidates_with_context("おう", "責任は広告主が");
        let with_function_word = dictionary.candidates_with_surrounding_context(
            "おう",
            "責任は広告主が",
            "ものとします。",
        );
        let surface_position = |candidates: &[Candidate], surface: &str| {
            candidates
                .iter()
                .position(|candidate| candidate.surface == surface)
                .expect("surface must remain visible")
        };
        assert_eq!(
            surface_position(&left_only, "追う") < surface_position(&left_only, "負う"),
            surface_position(&with_function_word, "追う")
                < surface_position(&with_function_word, "負う"),
            "the POS boundary may prefer verbs over nouns but must not reverse semantic peers"
        );
    }

    #[test]
    fn surrounding_context_uses_pos_connections_before_bounded_particles() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            (
                "おしゃれ",
                "店内のインテリアはとっても",
                "で雰囲気があります。",
                "お洒落",
            ),
            ("じこ", "文総裁の長男と二男は", "で死亡した。", "事故"),
            (
                "らいか",
                "再稼働する原発がなければ、",
                "までに停止する。",
                "来夏",
            ),
            (
                "きせん",
                "ロシア極東サハリンから",
                "で国後島に到着した。",
                "汽船",
            ),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }
    }

    #[test]
    fn surrounding_context_promotes_a_unique_form_before_following_grammar() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("こ")[0].surface, "個");
        let candidates = dictionary.candidates_with_surrounding_context(
            "こ",
            "有名な先生方が講師として",
            "られています。",
        );
        assert_eq!(candidates[0].surface, "来", "{candidates:?}");

        assert_eq!(dictionary.candidates("わたし")[0].surface, "私");
        let candidates = dictionary.candidates_with_surrounding_context(
            "わたし",
            "彼らは更に自らの救命胴衣を他の兵士に",
            "た。",
        );
        assert_eq!(candidates[0].surface, "渡し", "{candidates:?}");

        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "もし",
                "旅の途中で出会うモンスターを",
                "た家具を"
            )[0]
            .surface,
            "模試",
            "ambiguous compatible verb surfaces require semantic evidence"
        );
    }

    #[test]
    fn surrounding_context_applies_grammar_to_the_last_converted_segment() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            (
                "こうしとしてこ",
                "有名な先生方が",
                "られています。",
                "講師として来",
            ),
            (
                "でざーとまでつい",
                "最後に穀物の珈琲と",
                "てくるのです。",
                "デザートまで付い",
            ),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }

        assert_eq!(
            dictionary.candidates_with_context("こうしとしてこ", "有名な先生方が")[0].surface,
            "講師としてこ",
            "the unconfirmed continuation must not be inferred"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "とおもいもよら",
                "父から励まされる",
                "ない言葉だった。"
            )[0]
            .surface,
            "と思いもよら",
            "grammar compatibility alone must not erase a conventional kana form"
        );
    }

    #[test]
    fn surrounding_context_promotes_grammar_compatible_forms_without_broad_de_matching() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            ("たけ", "スポーツにも", "ている。", "長け"),
            ("わけ", "インクは、大きく", "てビン入りと", "分け"),
            ("こん", "いつも割りと", "でる。", "混ん"),
            (
                "しん",
                "管理していたデボルとポポルも",
                "でしまった。",
                "死ん",
            ),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }

        for (reading, left_context, right_context, expected) in [
            ("きき", "放送やオーディオ", "での音楽", "機器"),
            ("うみ", "かつて浅い", "であった。", "海"),
            (
                "たいき",
                "スペアとしてバックステージに",
                "させていた。",
                "待機",
            ),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }
    }

    #[test]
    fn surrounding_context_promotes_a_unique_verbal_noun_before_suru() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            ("ながい", "そのまま", "してしまいます", "長居"),
            ("かんしん", "参加者が", "すること", "感心"),
            ("くし", "能力を", "するスナイパー", "駆使"),
            ("いち", "中心に", "する", "位置"),
            (
                "ぜんしん",
                "一般的な層流翼型と比べ負圧中心が",
                "し、圧力勾配はなだらかである。",
                "前進",
            ),
            ("そくし", "腹部を切開しただけでは人は", "しない。", "即死"),
            ("たいい", "これにより、ギャネンドラ国王は", "した。", "退位"),
        ] {
            let candidates = dictionary.candidates_with_surrounding_context(
                reading,
                left_context,
                right_context,
            );
            assert_eq!(candidates[0].surface, expected, "{candidates:?}");
        }

        assert_eq!(
            dictionary.candidates_with_surrounding_context("ながい", "そのまま", "道路")[0].surface,
            "長い",
            "unrelated right context must not promote a verbal noun"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context_limit(
                "ぐれーどよんにいち",
                "彼らは、全国平均",
                "し、全国平均グレード8よりも優れている。",
                32,
            )[0]
            .surface,
            "グレード4に位置",
            "a numeric run must not swallow a particle before a unique verbal noun"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ぐれーどよんにいち",
                "彼らは、全国平均",
                "、全国平均グレード8よりも優れている。"
            )[0]
            .surface,
            "グレード421",
            "the segmented form needs an explicit suru continuation"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context_limit(
                "さんにおつたえ",
                "尋常ではないこの現実をみな",
                "したいのです。",
                32,
            )[0]
            .surface,
            "さんにお伝え",
            "the honorific ending さん must not be reinterpreted as the digit 3"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "とまる",
                "学校のまわりにはホテルも無いので、成田か香取で",
                "しかありません。"
            )[0]
            .surface,
            "止まる",
            "the binding particle しか must not be parsed as a suru inflection"
        );
        assert!(
            dictionary
                .candidates_with_surrounding_context(
                    "し",
                    "星取り参加は当然とされ,不参加は白眼",
                    "される。"
                )
                .iter()
                .any(|candidate| candidate.surface == "視"),
            "a broad passive-form rule must not evict an otherwise visible candidate"
        );
    }

    #[test]
    fn surrounding_context_uses_a_bounded_quotation_reporting_frame() {
        let dictionary = Dictionary::bundled();
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "けいさつにいっ",
                "証人は、被疑者を攻撃した、と",
                "た。"
            )[0]
            .surface,
            "警察に言っ"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "けいさつにいっ",
                "証人は昨日、",
                "た。"
            )[0]
            .surface,
            "警察に行っ",
            "a destination without a quotation case must keep the motion verb"
        );
    }

    #[test]
    fn colloquial_imperative_quotation_uses_structure_and_confirmed_context() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates("だいじょうぶじしんもてって")[0].surface,
            "大丈夫自信持てって",
            "an embedded noun plus imperative must beat a fragmented kana suffix"
        );
        assert_eq!(
            dictionary.candidates_with_context("じしんもてって", "大丈夫、")[0].surface,
            "自信持てって",
            "a confirmed clause boundary must supply the missing left phrase"
        );
        assert_eq!(
            dictionary.candidates("じしんもてって")[0].surface,
            "自身もてって",
            "a single noun without document context is too ambiguous to promote"
        );
        assert_eq!(
            dictionary.candidates_with_context("あのかけって", "これは、")[0].surface,
            "あの賭けって",
            "an adnominal noun phrase must not be reinterpreted as an imperative"
        );
    }

    #[test]
    fn surrounding_context_does_not_choose_between_ambiguous_verbal_nouns() {
        let dictionary = Dictionary::bundled_with_layers(vec![DictionaryLayer::new(
            "ambiguous-verbal-noun",
            "Ambiguous verbal noun",
            vec![DictionaryEntry::with_pos(
                "ながい",
                "長異",
                MOZC_VERBAL_NOUN_POS_ID,
                MOZC_VERBAL_NOUN_POS_ID,
                6_000,
            )],
        )]);

        assert_eq!(
            dictionary.candidates_with_surrounding_context("ながい", "そのまま", "してしまいます"),
            dictionary.candidates_with_context("ながい", "そのまま"),
            "semantic ambiguity must remain with the existing ranker"
        );
    }

    #[test]
    fn document_context_keeps_a_particle_led_fragment_attached() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates_with_context("のうむ", "経験");

        assert_eq!(candidates[0].surface, "の有無", "{candidates:?}");

        let candidates = dictionary.candidates_with_context("のやま", "アルプス");
        assert_eq!(candidates[0].surface, "の山", "{candidates:?}");
    }

    #[test]
    fn surrounding_context_promotes_dictionary_compounds_to_the_right() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates_with_context("まち", "患者と患者の")[0].surface,
            "街"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("まち", "患者と患者の", "時間は少ない")
                [0]
            .surface,
            "待ち"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("わ", "古典演奏から", "楽器を")[0]
                .surface,
            "和"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "み",
                "犯人は検挙されておらず、2012年8月現在",
                "解決。"
            )[0]
            .surface,
            "未",
            "an exact noun-prefix phrase should beat an incidental left phrase"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "かぶ",
                "現在、その名称はアトレティコの",
                "組織の名前として残っている"
            )[0]
            .surface,
            "下部"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "き",
                "カラフルで色合いがいいデザインがあったので",
                "に入りました"
            )[0]
            .surface,
            "気"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "し",
                "これが無い場合、作業者は",
                "に至る致命傷を負う"
            )[0]
            .surface,
            "死"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "かた",
                "デスクワークで固まった",
                "や背中をほぐす"
            )[0]
            .surface,
            "肩"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "はな",
                "生え際を評価するパターン、",
                "や口の大きさを評価する"
            )[0]
            .surface,
            "鼻"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "かた",
                "デスクワークで固まった",
                "や背中合わせの配置"
            ),
            dictionary.candidates_with_context("かた", "デスクワークで固まった"),
            "a coordination phrase inside a longer noun is not a word boundary"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("いぼ", "劉裕の", "弟にあたる")[0]
                .surface,
            "異母"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("いぼ", "皮膚の", "弟子にあたる")[0]
                .surface,
            dictionary.candidates_with_context("いぼ", "皮膚の")[0].surface,
            "a sibling character inside a longer noun is not a word boundary"
        );
    }

    #[test]
    fn surrounding_context_promotes_a_right_compound_after_converted_segments() {
        let dictionary = Dictionary::bundled();
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "とうしょばってりふ",
                "韓国の会社は、",
                "具合が問題だと考え、自社製の"
            )[0]
            .surface,
            "当初バッテリ不"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "かたまったかた",
                "デスクワークで",
                "や背中を、アロママッサージで"
            )[0]
            .surface,
            "固まった肩"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "じけんのさい",
                "最新作の監督は、その",
                "セットにはいなかった"
            )[0]
            .surface,
            "事件の際",
            "a three-character following word must not override a confident phrase"
        );
    }

    #[test]
    fn surrounding_context_promotes_bounded_genitive_phrases() {
        let dictionary = Dictionary::bundled();

        for (reading, left_context, right_context, expected) in [
            ("しん", "だがワールドヒーローズの", "の目的とは", "真"),
            ("みち", "人間の心、", "の世界を探究する", "未知"),
            ("い", "たとえば唇が腫れるのは", "の調子が悪い現れ", "胃"),
            ("す", "コンサートや記者会見ではなく、", "の状態だった", "素"),
            ("け", "体表面は水にぬれても", "の根元は油分を含む", "毛"),
        ] {
            assert_eq!(
                dictionary.candidates_with_surrounding_context(
                    reading,
                    left_context,
                    right_context
                )[0]
                .surface,
                expected
            );
        }

        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "みち",
                "人間の心、",
                "の世界観を探究する"
            ),
            dictionary.candidates_with_context("みち", "人間の心、"),
            "a genitive phrase inside a longer noun is not a word boundary"
        );
    }

    #[test]
    fn right_compound_evidence_respects_stronger_left_boundaries() {
        let dictionary = Dictionary::bundled();

        for left_context in ["デドフスクにはM9", "デドフスクにはM９"] {
            assert_eq!(
                dictionary.candidates_with_surrounding_context(
                    "かんせん",
                    left_context,
                    "道路が通る"
                )[0]
                .surface,
                "幹線"
            );
        }
        for left_context in ["県警2", "県警２"] {
            assert_eq!(
                dictionary.candidates_with_surrounding_context("か", left_context, "長を務めた")[0]
                    .surface,
                "課"
            );
        }
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "だい",
                "特に六",
                "目尾上梅幸を相方とした"
            ),
            dictionary.candidates_with_context("だい", "特に六"),
            "Japanese numerals must retain the dedicated numeric boundary"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("けん", "福岡都市", "内外から")[0]
                .surface,
            "圏"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("かん", "第1", "冒頭では")[0].surface,
            "巻"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("あき", "現在は", "名跡となっている")[0]
                .surface,
            dictionary.candidates_with_context("あき", "現在は")[0].surface,
            "a surname-only compound must not become document phrase evidence"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "し",
                "破裂した心臓すら治せるという超能力なのに、佐藤",
                "の病気は治せなかった"
            )[0]
            .surface,
            "氏",
            "an unrelated inflection must not override a noun before a particle"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ねがい",
                "今後ともよろしくお",
                "いたします"
            )[0]
            .surface,
            "願い",
            "right-side evidence must not reuse a surface already covered by the reading"
        );
    }

    #[test]
    fn alphanumeric_numeric_context_recalls_a_deep_compound() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates_with_surrounding_context_limit(
            "きゅーかんせん",
            "デドフスクにはM",
            "道路が通るほか、モスクワからリガに向かう鉄道も通る。 ",
            5,
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.surface == "9幹線"),
            "missing M9 compound: {candidates:?}"
        );
        assert_eq!(
            candidates[0].surface, "9幹線",
            "wrong M9 winner: {candidates:?}"
        );
        assert_eq!(candidates.len(), 5, "the visible candidate width is stable");

        for (left_context, right_context) in [
            ("デドフスクには", "道路が通る"),
            ("デドフスクにはM。", "道路が通る"),
            ("デドフスクにはM", "として知られる"),
        ] {
            assert!(!super::should_expand_alphanumeric_numeric_compound(
                "きゅーかんせん",
                left_context,
                right_context,
            ));
        }
    }

    #[test]
    fn numeric_work_counter_prefers_the_modern_variant() {
        let candidates = Dictionary::bundled().candidates_with_surrounding_context(
            "さんきゅーへん",
            "1887年6月にかけて、",
            "の物語を発表した。",
        );
        assert_eq!(
            candidates[0].surface, "39編",
            "wrong counter: {candidates:?}"
        );
    }

    #[test]
    fn numeric_particle_suru_context_recalls_a_deep_verbal_noun() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates_with_surrounding_context_limit(
            "ぐれーどよんにいち",
            "彼らは、全国平均",
            "し、全国平均グレード8よりも優れている。",
            5,
        );
        assert_eq!(
            candidates[0].surface, "グレード4に位置",
            "wrong numeric verbal noun: {candidates:?}"
        );
        assert_eq!(candidates.len(), 5, "the visible candidate width is stable");

        for (reading, right_context) in [
            ("ぐれーどよんにいち", "を示す"),
            ("さんにいち", "した"),
            ("ぐれーどよんに", "した"),
        ] {
            assert!(!super::should_expand_numeric_particle_suru(
                reading,
                right_context
            ));
        }
    }

    #[test]
    fn assimilated_numeric_score_recalls_one_to_one_notation() {
        let dictionary = Dictionary::bundled();
        assert_eq!(dictionary.candidates("じーけー")[0].surface, "GK");
        assert_eq!(dictionary.candidates("えーあい")[0].surface, "AI");
        assert_eq!(dictionary.candidates("えすえぬえす")[0].surface, "SNS");
        assert_eq!(dictionary.candidates("えむあいしっくす")[0].surface, "MI6");
        assert_eq!(dictionary.candidates("えむぴーすりー")[0].surface, "MP3");
        assert!(
            dictionary
                .candidates("えーあいふぉーらむ")
                .iter()
                .all(|candidate| !candidate.surface.starts_with("AI4")),
            "an English digit reading inside a word is not an identifier boundary"
        );
        assert!(
            dictionary
                .candidates("じー")
                .iter()
                .all(|candidate| candidate.surface != "G"),
            "a single spoken letter is too ambiguous"
        );
        let score = dictionary.candidates_with_surrounding_context("いったい", "", "1になった。");
        assert!(
            score
                .iter()
                .take(2)
                .any(|candidate| candidate.surface == "1対"),
            "missing score: {score:?}"
        );
        let ascii = dictionary.candidates_with_surrounding_context(
            "じーけーといったい",
            "相手",
            "1になったりだとか、完全にフリーでシュートを打つ。",
        );
        assert_eq!(
            ascii[0].surface, "GKと1対",
            "wrong score notation: {ascii:?}"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "じーけーといったい",
                "相手",
                "１になった。",
            )[0]
            .surface,
            "GKと１対"
        );
        assert_eq!(dictionary.candidates("いったい")[0].surface, "一体");
    }

    #[test]
    fn surrounding_context_prefers_a_reach_range_after_a_measurement() {
        let dictionary = Dictionary::bundled();

        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "けん",
                "大阪駅から徒歩10分",
                "内のホテル"
            )[0]
            .surface,
            "圏"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("けん", "半径５ｋｍ", "外から通勤する")
                [0]
            .surface,
            "圏"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("けん", "福岡", "内のホテル")[0].surface,
            "県",
            "a place name without a measured reach remains an administrative region"
        );
    }

    #[test]
    fn document_context_promotes_bounded_region_phrases() {
        let dictionary = Dictionary::bundled();

        assert_eq!(dictionary.candidates("せん")[0].surface, "1000");
        assert_eq!(
            dictionary.candidates_with_context("せん", "京義・東海")[0].surface,
            "線"
        );
        assert_eq!(
            dictionary.candidates_with_context("せん", "超長距離")[0].surface,
            "戦"
        );
        assert_eq!(
            dictionary.candidates_with_context("し", "いなべ")[0].surface,
            "市"
        );
        assert_eq!(
            dictionary.candidates_with_context("し", "高木守道")[0].surface,
            "氏",
            "a person's name must not be treated as a region"
        );
    }

    #[test]
    fn region_suffix_promotion_is_limited_to_administrative_surfaces() {
        for (reading, surface) in [
            ("し", "市"),
            ("く", "区"),
            ("けん", "県"),
            ("ぐん", "郡"),
            ("ちょう", "町"),
            ("まち", "町"),
            ("そん", "村"),
            ("むら", "村"),
            ("せん", "線"),
        ] {
            assert!(document_region_suffix_promotion(reading, surface) > 0);
        }
        assert_eq!(document_region_suffix_promotion("し", "氏"), 0);
        assert_eq!(document_region_suffix_promotion("まち", "街"), 0);
    }

    #[test]
    fn exact_region_surface_requires_matching_reading_surface_and_pos() {
        let region_pos_id = MOZC_REGION_POS_IDS[0];
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("おきなわけん", "沖縄県", region_pos_id, region_pos_id, 100),
            DictionaryEntry::new("とし", "都市", 100),
        ]);

        assert!(dictionary.has_exact_region_surface("おきなわけん", "沖縄県"));
        assert!(!dictionary.has_exact_region_surface("おきなわけん", "沖縄市"));
        assert!(!dictionary.has_exact_region_surface("とし", "都市"));
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
    fn model_recall_clone_adds_unknown_katakana_prefix_without_changing_base() {
        let dictionary = Dictionary::bundled();
        let reading = "あけめねいどぐん";
        let base = dictionary.candidates_with_limit(reading, 32);
        let recalled = dictionary
            .with_model_recall_katakana_cost()
            .candidates_with_limit(reading, 32);

        assert_ne!(base[0].surface, "アケメネイド軍");
        assert_eq!(
            recalled[0].surface, "アケメネイド軍",
            "model recall candidates: {recalled:?}"
        );
        assert!(
            recalled
                .iter()
                .any(|candidate| candidate.surface == "アケメネイド軍"),
            "model recall candidates: {recalled:?}"
        );
        assert_eq!(dictionary.candidates_with_limit(reading, 32), base);
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
        assert!(
            super::parse_kana_number_prefixes("ぜろせん")
                .iter()
                .all(|(length, value)| *length < "ぜろせん".len() || *value != 1_000),
            "a leading zero must not multiply a positional unit"
        );
        let lexical_zero = dictionary.candidates_with_limit("ぜろせん", 64);
        assert!(
            lexical_zero
                .iter()
                .any(|candidate| candidate.surface == "ゼロ戦"),
            "candidates: {lexical_zero:?}"
        );

        let percent = dictionary.candidates("ぱーせんと");
        assert_eq!(percent[0].surface, "%");
        assert!(
            percent.iter().any(|candidate| candidate.surface == "％"),
            "candidates: {percent:?}"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ぱーせんとたかく",
                "り10％高くなると、紛争の可能性が約3％低下し、その一方で成長率が研究平均より1",
                "なると、内戦が発生する確率が約1％低下したのである。",
            )[0]
            .surface,
            "％高く"
        );
    }

    #[test]
    fn spoken_digit_sequences_preserve_the_explicit_digits() {
        let dictionary = Dictionary::bundled();
        for (reading, expected) in [
            ("ぜろぜろぜろにん", "000人"),
            ("にぜろいちよねん", "2014年"),
            ("ななはっせん", "78銭"),
        ] {
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
    fn small_numbers_after_katakana_names_recall_roman_numerals() {
        let dictionary = Dictionary::bundled();
        let candidates = dictionary.candidates_with_limit("ぷらいすさんのかお", 64);
        assert_eq!(candidates[0].surface, "プライスさんの顔");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.surface == "プライスⅢの顔"),
            "candidates: {candidates:?}"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ぷらいすさんのかお",
                "アンドレ・",
                "をエアマットレスに押し付けた。",
            )[0]
            .surface,
            "プライスⅢの顔"
        );
        assert!(
            dictionary
                .candidates_with_limit("おじいさんのかお", 64)
                .iter()
                .all(|candidate| !candidate.surface.contains('Ⅲ')),
            "ordinary honorific readings must not gain a roman numeral"
        );
    }

    #[test]
    fn surrounding_context_preserves_a_foreign_name_honorific_boundary() {
        let dictionary = Dictionary::bundled();
        assert_ne!(
            dictionary.candidates("すたーんりーぶし")[0].surface,
            "スターンリーブ氏"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "すたーんりーぶし",
                "ジョー・",
                "は発表した。",
            )[0]
            .surface,
            "スターンリーブ氏"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context(
                "すたーんりーぶし",
                "無関係な文脈",
                "は発表した。",
            )[0]
            .surface,
            "スターンリーブ氏"
        );
    }

    #[test]
    fn surrounding_context_recognizes_a_chronological_year_boundary() {
        let dictionary = Dictionary::bundled();
        assert_ne!(
            dictionary.candidates("きげんぜんごいちいち")[0].surface,
            "紀元前511"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "きげんぜんごいちいち",
                "結局、",
                "年から512年にマケドニアの王は",
            )[0]
            .surface,
            "紀元前511"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context(
                "きげんぜんごいちいち",
                "結局、",
                "件を調査した。",
            )[0]
            .surface,
            "紀元前511",
            "an unrelated numeric continuation must not imply a chronological year"
        );
    }

    #[test]
    fn surrounding_context_recognizes_an_approximate_quantity_boundary() {
        let dictionary = Dictionary::bundled();
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "するにはやく",
                "実現可能性調査は、川を横断",
                "4分かかるだろう。",
            )[0]
            .surface,
            "するには約"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context("はやく", "彼は", "4分で終えた。",)[0]
                .surface,
            "早く",
            "a quantity alone must not split the adverb into a topic and approximation"
        );
        assert_ne!(
            dictionary.candidates_with_surrounding_context(
                "するにはやく",
                "実現可能性調査は、川を横断",
                "分ほどかかるだろう。",
            )[0]
            .surface,
            "するには約",
            "a unit without a confirmed number is not a quantity boundary"
        );
    }

    #[test]
    fn spoken_digit_counter_forms_do_not_reinterpret_standalone_words() {
        let dictionary = Dictionary::bundled();
        for reading in ["よねん", "はっせん"] {
            let conversions = dictionary.convert_n_best(reading, 32);
            assert!(
                conversions.iter().all(|conversion| {
                    !conversion.surface.starts_with('4')
                        && !conversion.surface.starts_with("４")
                        && !conversion.surface.starts_with("8銭")
                        && !conversion.surface.starts_with("８銭")
                }),
                "standalone counter form changed for {reading}: {conversions:?}"
            );
        }
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
    fn assimilated_numbers_compose_only_before_dictionary_counters() {
        let dictionary = Dictionary::bundled();
        let arena = bumpalo::Bump::new();
        let generated = synthetic_entries_by_start(
            &dictionary,
            "しゅういっかい",
            &arena,
            super::katakana_run_character_cost(),
        );
        assert!(
            generated["しゅう".len()]
                .iter()
                .any(|entry| entry.surface == "1回"),
            "missing combined counter node: {:?}",
            generated["しゅう".len()]
        );

        for (reading, expected) in [
            ("しゅういっかい", "週1回"),
            ("おにぎりいっこ", "おにぎり1個"),
            ("さつじんといっけん", "殺人と1件"),
            ("ろっかこく", "6カ国"),
            ("いっかしょ", "1カ所"),
            ("だいいっき", "第1期"),
            ("ふたりをはけん", "2人を派遣"),
        ] {
            let candidates = dictionary.candidates_with_limit(reading, 32);
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.surface == expected),
                "missing {expected} for {reading}: {candidates:?}"
            );
        }

        let homicide_candidates = dictionary.candidates_with_limit("さつじんといっけん", 32);
        assert!(homicide_candidates.iter().all(|candidate| {
            !candidate.surface.contains("1県")
                && !candidate.surface.contains("1兼")
                && !candidate.surface.contains("１県")
                && !candidate.surface.contains("１兼")
        }));

        for (reading, expected) in [
            ("いっけん", "一見"),
            ("いっぱん", "一般"),
            ("いっこう", "一行"),
            ("いっき", "一気"),
            ("はっきり", "ハッキリ"),
            ("いった", "行った"),
        ] {
            assert_eq!(
                dictionary.candidates(reading)[0].surface,
                expected,
                "an assimilated counter alternative must not replace the established word"
            );
        }
        let conversions = dictionary.convert_n_best("いった", 32);
        assert!(
            conversions.iter().all(|conversion| {
                !conversion.surface.starts_with('1') && !conversion.surface.starts_with('１')
            }),
            "a reading without a following counter gained a number: {conversions:?}"
        );

        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ろっかこく",
                "前回は2地域、",
                "から代表が集まった。",
            )[0]
            .surface,
            "6カ国"
        );
        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "ふたりをはけん",
                "前回は２人、今回は",
                "する。",
            )[0]
            .surface,
            "２人を派遣"
        );
    }

    #[test]
    fn sokuon_ten_reading_composes_inside_numbers_and_before_minutes() {
        let dictionary = Dictionary::bundled();
        for (reading, expected) in [
            ("さんじっかい", "30回"),
            ("ごじっこ", "50個"),
            ("じっぷんけん", "10分圏"),
        ] {
            let candidates = dictionary.candidates(reading);
            assert!(
                candidates
                    .iter()
                    .any(|candidate| candidate.surface == expected),
                "missing {expected} for {reading}: {candidates:?}"
            );
        }

        for reading in ["じっけん", "じっこう", "じっかい"] {
            let conversions = dictionary.convert_n_best(reading, 32);
            assert!(
                conversions
                    .iter()
                    .all(|conversion| !conversion.surface.starts_with("10")
                        && !conversion.surface.starts_with("１０")),
                "standalone じっ must not become 10 in {reading}: {conversions:?}"
            );
        }
    }

    #[test]
    fn numeric_guard_preserves_overlapping_dictionary_words() {
        let dictionary = Dictionary::bundled();
        let states_after_dictionary_scan = |reading: &str| {
            let arena = bumpalo::Bump::new();
            let entries = synthetic_entries_by_start(
                &dictionary,
                reading,
                &arena,
                super::katakana_run_character_cost(),
            );
            let mut states = numeric_start_states(reading, entries.as_slice());
            for (start, _) in reading.char_indices() {
                dictionary.for_each_prefix_guarding_numeric_starts(
                    reading,
                    start,
                    &mut states,
                    |_, _| {},
                );
            }
            states
        };

        assert_eq!(
            states_after_dictionary_scan("よせんなない")["よ".len()],
            NUMERIC_START_PROTECTED
        );
        assert_eq!(
            states_after_dictionary_scan("とくにろくだい")["と".len()],
            NUMERIC_START_PROTECTED
        );
        assert_eq!(
            states_after_dictionary_scan("にはしぜんこ")["には".len()],
            NUMERIC_START_PROTECTED
        );
        let preliminary = dictionary.candidates_with_limit("よせんなない", 32);
        assert!(
            preliminary
                .iter()
                .take(3)
                .any(|candidate| candidate.surface == "予選7位")
        );
        let preliminary = dictionary.candidates_with_limit("とくにろくだい", 32);
        assert!(
            preliminary
                .iter()
                .take(3)
                .any(|candidate| candidate.surface == "特に6代")
        );
        let natural_lake = dictionary.candidates_with_limit("にはしぜんこ", 32);
        assert!(
            natural_lake
                .iter()
                .any(|candidate| candidate.surface == "には自然湖")
        );

        assert_eq!(
            dictionary.candidates_with_surrounding_context(
                "なくいちにん",
                "でも、まずはハーフドリアでは",
                "前のドリアをお試し下さい。"
            )[0]
            .surface,
            "なく1人"
        );
        assert!(
            dictionary
                .candidates("はっせん")
                .iter()
                .any(|candidate| candidate.surface == "8000")
        );

        let numeric = SyntheticEntry {
            end: 0,
            surface: "4000",
            left_id: ARABIC_NUMBER_POS_ID,
            right_id: ARABIC_NUMBER_POS_ID,
            cost: 100,
            numeric: true,
        };
        assert_eq!(guarded_cost(&numeric, false), 100);
        assert_eq!(
            guarded_cost(&numeric, true),
            100 + numeric_interior_dictionary_penalty()
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
        assert!(dictionary.has_exact_entry("らすとげんご", "Rust言語"));
        assert!(!dictionary.has_exact_entry("らすとげんご", "ラスト言語"));
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
