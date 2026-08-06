//! Platform-independent IME state machine.

use slime_converter::{Dictionary, Segment};
use slime_romaji::RomajiComposer;

mod date_time_candidates;
mod dictionary_packs;
mod domain_dictionaries;
mod english_reverse;
mod live_conversion;
mod session_history;
mod text_transform;
mod user_data;

use dictionary_packs::DictionaryPackStore;
use english_reverse::ReverseMatch;
use live_conversion::Decision as LiveConversionDecision;
use session_history::SessionHistory;

pub use dictionary_packs::{
    DictionaryPackInfo, DictionaryPackLoadError, DictionaryPackWord, validate_dictionary_pack,
};
pub use domain_dictionaries::{
    ALL_DOMAIN_DICTIONARIES, BUSINESS_DICTIONARY, CREATIVE_DICTIONARY, TECHNOLOGY_DICTIONARY,
    words as domain_dictionary_words,
};
pub use user_data::{HistoryEntry, UserData, UserDictionaryEntry};

/// Every built-in date candidate format, used as the default by adapters.
pub const ALL_DATE_FORMATS: u32 = date_time_candidates::ALL_FORMATS;

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
        selected: usize,
    },
    HideCandidates,
    Commit(String),
    Clear,
    ForwardKey,
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditableSegment {
    reading: String,
    surface: String,
}

#[derive(Clone, Debug)]
struct LivePreview {
    /// Complete kana reading covered by `surface`.
    reading: String,
    surface: String,
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
    selected: usize,
    candidate_kind: Option<CandidateKind>,
    completion_selected: bool,
    segments: Vec<EditableSegment>,
    active_segment: usize,
    transformed_surface: Option<String>,
    preferences: EnginePreferences,
    live_preview: Option<LivePreview>,
    live_preview_suppressed: bool,
    user_data: UserData,
    installed_packs: DictionaryPackStore,
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
            selected: 0,
            candidate_kind: None,
            completion_selected: false,
            segments: Vec::new(),
            active_segment: 0,
            transformed_surface: None,
            preferences: EnginePreferences::default(),
            live_preview: None,
            live_preview_suppressed: false,
            user_data: UserData::default(),
            installed_packs: DictionaryPackStore::default(),
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
        let installed_packs = DictionaryPackStore::load(user_data.directory());
        let dictionary = bundled_dictionary_with_packs(0, &user_data, &installed_packs);
        let mut engine = Self::with_user_data(dictionary, user_data);
        engine.installed_packs = installed_packs;
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
        self.user_data.reload();
        self.installed_packs = DictionaryPackStore::load(self.user_data.directory());
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

    /// Starts explicit reconversion for a selected committed surface. An
    /// empty action list means the surface has no safe dictionary reading.
    pub fn begin_reconversion(&mut self, surface: &str) -> Vec<SlimeAction> {
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
            let reading = self.reading.clone();
            self.record_history(&reading, &committed);
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

        self.candidates = self.conversion_candidates_for_reading(&self.reading);
        self.selected = 0;
        self.candidate_kind = Some(CandidateKind::Conversion);
        self.completion_selected = false;
        actions.extend(self.candidate_actions());
        actions
    }

    fn conversion_candidates_for_reading(&self, reading: &str) -> Vec<String> {
        let mut candidates = Vec::new();
        for surface in self.user_data.exact_dictionary_surfaces(reading) {
            push_unique(&mut candidates, surface.to_owned());
        }
        if self.history_is_available() {
            for surface in self.session_history.exact_surfaces(reading, 9) {
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
        for surface in self
            .dictionary
            .candidates(reading)
            .into_iter()
            .map(|candidate| candidate.surface)
        {
            push_unique(&mut candidates, surface);
        }
        insert_visible_katakana_candidate(&mut candidates, reading);
        insert_unique_candidates_after_first(
            &mut candidates,
            date_time_candidates::candidates(reading, self.preferences.date_format_mask),
        );
        candidates
    }

    fn history_is_available(&self) -> bool {
        self.preferences.history_completion && !self.preferences.private_mode
    }

    fn next_candidate(&mut self) -> Vec<SlimeAction> {
        if self.candidates.is_empty() {
            return vec![SlimeAction::ForwardKey];
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
        if self.candidate_kind == Some(CandidateKind::SegmentedConversion) {
            self.candidate_actions()
        } else {
            vec![SlimeAction::UpdatePreedit(
                self.selected_candidate().to_owned(),
            )]
        }
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
            candidates: self.candidates.clone(),
            selected: self.selected,
        });
        actions
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
            self.record_history(&reading, &committed);
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
        self.selected = 0;
        self.candidate_kind = None;
        self.completion_selected = false;
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
            for surface in self.session_history.completion_surfaces(&self.reading, 9) {
                push_unique(&mut suggestions, surface.to_owned());
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
                selected: self.selected,
            });
        }
        if include_preedit && (!self.reading.is_empty() || !self.romaji.pending().is_empty()) {
            actions.insert(0, SlimeAction::UpdatePreedit(self.preedit()));
        }
        actions
    }

    fn record_history(&mut self, reading: &str, surface: &str) {
        if !self.preferences.history_learning || self.preferences.private_mode {
            self.session_history.reset_context();
            return;
        }
        if !should_record_history(reading, surface) {
            self.session_history.reset_context();
            return;
        }

        self.user_data.record(reading, surface);
        self.session_history.record_commit(reading, surface);
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
        // Every mainstream Japanese IME (Kotoeri, Mozc, ATOK) types the middle
        // dot here; it has no other key on US layouts, while ／ stays
        // reachable through conversion candidates or ABC mode.
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
        ALL_DATE_FORMATS, ALL_DOMAIN_DICTIONARIES, CandidateKind, DictionaryPackWord,
        EnginePreferences, InputEvent, LiveConversionDecision, Phase, SlimeAction, SlimeEngine,
        TECHNOLOGY_DICTIONARY, UserData, bundled_dictionary, date_time_candidates,
        katakana_candidate,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn type_text(engine: &mut SlimeEngine, input: &str) {
        for character in input.chars() {
            engine.handle(InputEvent::Character(character));
        }
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
        assert!(
            engine
                .session_history
                .exact_surfaces("にほんご", 1)
                .is_empty()
        );
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
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "HOGE");
        fs::remove_dir_all(directory).unwrap();
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
# id: sample-pro
# name: サンプル Pro
# version: 2026.07.1
# license: Proprietary
すらいむぷろ\tSlime Pro
こまわり\t専門小回り\t6000
",
        )
        .unwrap();

        let engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        assert_eq!(
            engine.dictionary.candidates("すらいむぷろ")[0].surface,
            "Slime Pro"
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
        assert_eq!(infos[0].id, "sample-pro");
        assert_eq!(
            engine
                .installed_dictionary_pack_words("sample-pro")
                .unwrap(),
            [
                DictionaryPackWord {
                    reading: "すらいむぷろ".to_owned(),
                    surface: "Slime Pro".to_owned(),
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
        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "感じ");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn newly_selected_exact_candidate_beats_an_old_frequent_candidate() {
        let directory = test_directory("exact-history-reselection");
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
        assert_eq!(reloaded.snapshot().preedit, "感じ");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn session_context_beats_global_recency_without_persisting_context() {
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

        convert_and_commit(&mut engine, "bunshou", "文章");
        convert_and_commit(&mut engine, "kanji", "漢字");
        convert_and_commit(&mut engine, "kimochi", "気持ち");
        convert_and_commit(&mut engine, "kanji", "感じ");
        convert_and_commit(&mut engine, "bunshou", "文章");

        type_text(&mut engine, "kanji");
        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "漢字");

        let history = fs::read_to_string(directory.join("history.tsv")).unwrap();
        assert!(!history.contains("文章\t漢字"));
        assert!(!history.contains("文章\t感じ"));

        let mut reloaded = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        reloaded.set_preferences(preferences);
        type_text(&mut reloaded, "kanji");
        reloaded.handle(InputEvent::Space);
        assert_eq!(reloaded.snapshot().preedit, "感じ");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pausing_learning_breaks_session_context_boundary() {
        let directory = test_directory("session-context-pause");
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
    fn session_context_reranks_prefix_completions() {
        let directory = test_directory("session-completion-context");
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

        convert_and_commit(&mut engine, "bunshou", "文章");
        accept_completion(&mut engine, "kanji", "漢字変換");
        convert_and_commit(&mut engine, "kimochi", "気持ち");
        accept_completion(&mut engine, "kanji", "感情表現");
        convert_and_commit(&mut engine, "bunshou", "文章");

        type_text(&mut engine, "kanji");
        assert_eq!(engine.snapshot().candidates[0], "漢字変換");

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

        assert_eq!(actions, vec![SlimeAction::UpdatePreedit(selected.clone())]);
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
