//! Platform-independent IME state machine.

use ime_converter::Dictionary;
use ime_romaji::RomajiComposer;

mod domain_dictionaries;
mod session_history;
mod user_data;

use session_history::SessionHistory;

pub use domain_dictionaries::{
    ALL_DOMAIN_DICTIONARIES, BUSINESS_DICTIONARY, CREATIVE_DICTIONARY, TECHNOLOGY_DICTIONARY,
    words as domain_dictionary_words,
};
pub use user_data::{HistoryEntry, UserData, UserDictionaryEntry};

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
}

const _: () = assert!(std::mem::size_of::<InputEvent>() <= 8);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImeAction {
    UpdatePreedit(String),
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EnginePreferences {
    pub live_conversion: bool,
    pub history_completion: bool,
    pub history_learning: bool,
    pub dictionary_packs: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateKind {
    Conversion,
    Completion,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    pub preedit: String,
    pub candidates: Vec<String>,
    pub selected: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ImeEngine {
    dictionary: Dictionary,
    romaji: RomajiComposer,
    reading: String,
    candidates: Vec<String>,
    selected: usize,
    candidate_kind: Option<CandidateKind>,
    completion_selected: bool,
    preferences: EnginePreferences,
    live_preview: Option<String>,
    live_preview_suppressed: bool,
    user_data: UserData,
    session_history: SessionHistory,
    uses_bundled_dictionary: bool,
}

impl ImeEngine {
    #[must_use]
    pub fn new(dictionary: Dictionary) -> Self {
        Self {
            dictionary,
            romaji: RomajiComposer::new(),
            reading: String::new(),
            candidates: Vec::new(),
            selected: 0,
            candidate_kind: None,
            completion_selected: false,
            preferences: EnginePreferences::default(),
            live_preview: None,
            live_preview_suppressed: false,
            user_data: UserData::default(),
            session_history: SessionHistory::default(),
            uses_bundled_dictionary: false,
        }
    }

    #[must_use]
    pub fn with_user_data(dictionary: Dictionary, user_data: UserData) -> Self {
        Self {
            user_data,
            ..Self::new(dictionary)
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
        let dictionary = bundled_dictionary(0, &user_data);
        let mut engine = Self::with_user_data(dictionary, user_data);
        engine.uses_bundled_dictionary = true;
        engine
    }

    pub fn set_preferences(&mut self, preferences: EnginePreferences) -> Vec<ImeAction> {
        if self.preferences.history_learning && !preferences.history_learning {
            self.session_history.reset_context();
        }
        if self.uses_bundled_dictionary
            && self.preferences.dictionary_packs != preferences.dictionary_packs
        {
            self.dictionary = bundled_dictionary(preferences.dictionary_packs, &self.user_data);
        }
        self.preferences = preferences;
        self.live_preview_suppressed = false;
        self.refresh_live_preview();
        self.refresh_completion_actions(true)
    }

    pub fn reload_user_data(&mut self) -> Vec<ImeAction> {
        self.user_data.reload();
        if self.uses_bundled_dictionary {
            self.dictionary =
                bundled_dictionary(self.preferences.dictionary_packs, &self.user_data);
        }
        self.refresh_live_preview();
        self.refresh_completion_actions(true)
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
        if self.candidate_kind == Some(CandidateKind::Conversion) {
            Phase::Converting
        } else {
            Phase::Composing
        }
    }

    pub fn handle(&mut self, event: InputEvent) -> Vec<ImeAction> {
        match event {
            InputEvent::Character(character) => self.handle_character(character),
            InputEvent::Space => self.start_or_cycle_conversion(),
            InputEvent::NextCandidate => self.next_candidate(),
            InputEvent::PreviousCandidate => self.previous_candidate(),
            InputEvent::SelectCandidate(index) => self.select_candidate(index),
            InputEvent::AcceptCandidate => self.accept_candidate(),
            InputEvent::Enter => self.commit(),
            InputEvent::Escape => self.cancel(),
            InputEvent::Backspace => self.backspace(),
        }
    }

    fn handle_character(&mut self, character: char) -> Vec<ImeAction> {
        let mut actions = Vec::with_capacity(4);
        let had_completions = self.candidate_kind == Some(CandidateKind::Completion);
        if self.phase() == Phase::Converting {
            let committed = self.selected_candidate().to_owned();
            let reading = self.reading.clone();
            self.record_history(&reading, &committed);
            actions.push(ImeAction::Commit(committed));
            self.clear_composition();
            actions.push(ImeAction::HideCandidates);
        } else if had_completions {
            self.clear_candidates();
        }

        if character.is_ascii_alphabetic()
            || (character == '\'' && matches!(self.romaji.pending(), "n" | "t" | "d"))
        {
            let kana = self
                .romaji
                .push(character)
                .expect("ASCII romaji was validated");
            self.reading.push_str(&kana);
        } else {
            self.reading.push_str(&self.romaji.flush());
            self.reading.push(normalize_ascii_character(character));
        }

        self.live_preview_suppressed = false;
        actions.extend(self.refresh_composition_actions());
        if had_completions
            && !actions.contains(&ImeAction::HideCandidates)
            && self.candidates.is_empty()
        {
            actions.push(ImeAction::HideCandidates);
        }
        actions
    }

    fn start_or_cycle_conversion(&mut self) -> Vec<ImeAction> {
        let mut actions = Vec::with_capacity(3);
        if self.candidate_kind == Some(CandidateKind::Conversion) {
            self.selected = (self.selected + 1) % self.candidates.len();
            return self.candidate_actions();
        }
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.clear_candidates();
            actions.push(ImeAction::HideCandidates);
        }

        self.reading.push_str(&self.romaji.flush());
        if self.reading.is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        let mut candidates = Vec::new();
        for surface in self.user_data.exact_dictionary_surfaces(&self.reading) {
            push_unique(&mut candidates, surface.to_owned());
        }
        if self.preferences.history_completion {
            for surface in self.session_history.exact_surfaces(&self.reading, 9) {
                push_unique(&mut candidates, surface.to_owned());
            }
            for surface in self.user_data.exact_history_surfaces(&self.reading) {
                push_unique(&mut candidates, surface.to_owned());
            }
        }
        for surface in self
            .dictionary
            .candidates(&self.reading)
            .into_iter()
            .map(|candidate| candidate.surface)
        {
            if surface == self.reading && !candidates.is_empty() {
                continue;
            }
            push_unique(&mut candidates, surface);
        }
        insert_visible_katakana_candidate(&mut candidates, &self.reading);
        self.candidates = candidates;
        self.selected = 0;
        self.candidate_kind = Some(CandidateKind::Conversion);
        self.completion_selected = false;
        actions.extend(self.candidate_actions());
        actions
    }

    fn next_candidate(&mut self) -> Vec<ImeAction> {
        if self.candidates.is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        self.selected = (self.selected + 1) % self.candidates.len();
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        self.candidate_actions()
    }

    fn previous_candidate(&mut self) -> Vec<ImeAction> {
        if self.candidates.is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.candidates.len() - 1);
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        self.candidate_actions()
    }

    fn select_candidate(&mut self, index: u32) -> Vec<ImeAction> {
        let index = index as usize;
        if index >= self.candidates.len() {
            return Vec::new();
        }

        self.selected = index;
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        vec![ImeAction::UpdatePreedit(
            self.selected_candidate().to_owned(),
        )]
    }

    fn accept_candidate(&mut self) -> Vec<ImeAction> {
        if self.candidates.is_empty() {
            return vec![ImeAction::ForwardKey];
        }
        if self.candidate_kind == Some(CandidateKind::Completion) {
            self.completion_selected = true;
        }
        self.commit()
    }

    fn candidate_actions(&self) -> Vec<ImeAction> {
        let mut actions = Vec::with_capacity(2);
        if self.candidate_kind == Some(CandidateKind::Conversion) || self.completion_selected {
            actions.push(ImeAction::UpdatePreedit(
                self.selected_candidate().to_owned(),
            ));
        }
        actions.push(ImeAction::ShowCandidates {
            candidates: self.candidates.clone(),
            selected: self.selected,
        });
        actions
    }

    fn commit(&mut self) -> Vec<ImeAction> {
        self.reading.push_str(&self.romaji.flush());
        self.refresh_live_preview();
        let committed = if self.candidate_kind == Some(CandidateKind::Conversion)
            || (self.candidate_kind == Some(CandidateKind::Completion) && self.completion_selected)
        {
            self.selected_candidate().to_owned()
        } else if let Some(preview) = &self.live_preview
            && !self.live_preview_suppressed
        {
            preview.clone()
        } else {
            self.reading.clone()
        };

        if committed.is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        let reading = self.reading.clone();
        let used_completion =
            self.candidate_kind == Some(CandidateKind::Completion) && self.completion_selected;
        if used_completion {
            self.record_completion_history(&reading, &committed);
        } else {
            self.record_history(&reading, &committed);
        }
        let had_candidates = self.candidate_kind.is_some();
        self.clear_composition();
        let mut actions = vec![ImeAction::Commit(committed), ImeAction::Clear];
        if had_candidates {
            actions.push(ImeAction::HideCandidates);
        }
        actions
    }

    fn cancel(&mut self) -> Vec<ImeAction> {
        if self.candidate_kind.is_some() {
            self.clear_candidates();
            return vec![
                ImeAction::HideCandidates,
                ImeAction::UpdatePreedit(self.preedit()),
            ];
        }

        if self.live_preview.is_some() && !self.live_preview_suppressed {
            self.live_preview_suppressed = true;
            return vec![ImeAction::UpdatePreedit(self.preedit())];
        }

        if self.reading.is_empty() && self.romaji.pending().is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        self.clear_composition();
        vec![ImeAction::Clear]
    }

    fn backspace(&mut self) -> Vec<ImeAction> {
        if self.candidate_kind == Some(CandidateKind::Conversion) {
            self.clear_candidates();
            return vec![
                ImeAction::HideCandidates,
                ImeAction::UpdatePreedit(self.preedit()),
            ];
        }
        let had_completions = self.candidate_kind == Some(CandidateKind::Completion);
        if had_completions {
            self.clear_candidates();
        }

        if !self.romaji.backspace() {
            self.reading.pop();
        }

        self.live_preview_suppressed = false;
        let mut actions = self.refresh_composition_actions();
        if had_completions
            && !actions.contains(&ImeAction::HideCandidates)
            && self.candidates.is_empty()
        {
            actions.push(ImeAction::HideCandidates);
        }
        actions
    }

    fn preedit(&self) -> String {
        if self.candidate_kind == Some(CandidateKind::Conversion)
            || (self.candidate_kind == Some(CandidateKind::Completion) && self.completion_selected)
        {
            return self.selected_candidate().to_owned();
        }

        let mut preedit = self
            .live_preview
            .as_ref()
            .filter(|_| !self.live_preview_suppressed)
            .cloned()
            .unwrap_or_else(|| self.reading.clone());
        preedit.push_str(self.romaji.preview());
        preedit
    }

    fn selected_candidate(&self) -> &str {
        &self.candidates[self.selected]
    }

    fn clear_composition(&mut self) {
        self.romaji.clear();
        self.reading.clear();
        self.live_preview = None;
        self.live_preview_suppressed = false;
        self.clear_candidates();
    }

    fn clear_candidates(&mut self) {
        self.candidates.clear();
        self.selected = 0;
        self.candidate_kind = None;
        self.completion_selected = false;
    }

    fn refresh_composition_actions(&mut self) -> Vec<ImeAction> {
        self.refresh_live_preview();
        let preedit = self.preedit();
        let mut actions = if preedit.is_empty() {
            vec![ImeAction::Clear]
        } else {
            vec![ImeAction::UpdatePreedit(preedit)]
        };
        actions.extend(self.refresh_completion_actions(false));
        actions
    }

    fn refresh_live_preview(&mut self) {
        self.live_preview = if self.preferences.live_conversion && !self.reading.is_empty() {
            self.best_surface(&self.reading)
        } else {
            None
        };
    }

    fn best_surface(&self, reading: &str) -> Option<String> {
        if let Some(surface) = self.user_data.exact_dictionary_surfaces(reading).next() {
            return Some(surface.to_owned());
        }
        if self.preferences.history_completion
            && let Some(surface) = self.session_history.exact_surfaces(reading, 1).first()
        {
            return Some((*surface).to_owned());
        }
        if self.preferences.history_completion
            && let Some(surface) = self.user_data.exact_history_surfaces(reading).first()
        {
            return Some((*surface).to_owned());
        }
        self.dictionary
            .convert_best(reading)
            .map(|conversion| conversion.surface)
    }

    fn refresh_completion_actions(&mut self, include_preedit: bool) -> Vec<ImeAction> {
        let had_completions = self.candidate_kind == Some(CandidateKind::Completion);
        if self.phase() == Phase::Converting {
            return Vec::new();
        }

        let suggestions =
            if self.preferences.history_completion && self.reading.chars().count() >= 2 {
                let mut suggestions = Vec::with_capacity(9);
                for surface in self.session_history.completion_surfaces(&self.reading, 9) {
                    push_unique(&mut suggestions, surface.to_owned());
                }
                for surface in self.user_data.completion_surfaces(&self.reading, 9) {
                    push_unique(&mut suggestions, surface);
                    if suggestions.len() == 9 {
                        break;
                    }
                }
                suggestions
            } else {
                Vec::new()
            };

        let mut actions = Vec::with_capacity(2);
        if suggestions.is_empty() {
            if had_completions {
                self.clear_candidates();
                actions.push(ImeAction::HideCandidates);
            }
        } else {
            self.candidates = suggestions;
            self.selected = 0;
            self.candidate_kind = Some(CandidateKind::Completion);
            self.completion_selected = false;
            actions.push(ImeAction::ShowCandidates {
                candidates: self.candidates.clone(),
                selected: self.selected,
            });
        }
        if include_preedit && (!self.reading.is_empty() || !self.romaji.pending().is_empty()) {
            actions.insert(0, ImeAction::UpdatePreedit(self.preedit()));
        }
        actions
    }

    fn record_history(&mut self, reading: &str, surface: &str) {
        if !self.preferences.history_learning {
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
        if !self.preferences.history_learning {
            self.session_history.reset_context();
            return;
        }
        let Some(reading) = self.user_data.promote_completion(prefix, surface) else {
            self.session_history.reset_context();
            return;
        };
        self.session_history.record_commit(&reading, surface);
    }
}

fn bundled_dictionary(dictionary_packs: u32, user_data: &UserData) -> Dictionary {
    let mut layers = domain_dictionaries::layers(dictionary_packs);
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

fn should_record_history(reading: &str, surface: &str) -> bool {
    user_data::is_useful_history(reading, surface)
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

impl Default for ImeEngine {
    fn default() -> Self {
        Self::bundled()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_DOMAIN_DICTIONARIES, EnginePreferences, ImeAction, ImeEngine, InputEvent, Phase,
        TECHNOLOGY_DICTIONARY, UserData, bundled_dictionary, katakana_candidate,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn type_text(engine: &mut ImeEngine, input: &str) {
        for character in input.chars() {
            engine.handle(InputEvent::Character(character));
        }
    }

    fn convert_and_commit(engine: &mut ImeEngine, input: &str, surface: &str) {
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
        assert!(actions.contains(&ImeAction::Commit(surface.to_owned())));
    }

    fn accept_completion(engine: &mut ImeEngine, input: &str, surface: &str) {
        type_text(engine, input);
        let index = engine
            .snapshot()
            .candidates
            .iter()
            .position(|candidate| candidate == surface)
            .unwrap_or_else(|| panic!("missing completion {surface} for {input}"));
        engine.handle(InputEvent::SelectCandidate(u32::try_from(index).unwrap()));
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&ImeAction::Commit(surface.to_owned())));
    }

    fn test_directory(name: &str) -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "unvalley-ime-core-{name}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn ambiguous_trailing_n_remains_literal_in_preedit() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "nihon");

        assert_eq!(engine.snapshot().preedit, "にほn");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
    }

    #[test]
    fn ambiguous_n_stays_editable_and_double_n_is_one_syllabic_n() {
        let mut engine = ImeEngine::bundled();

        type_text(&mut engine, "n");
        assert_eq!(engine.snapshot().preedit, "n");

        type_text(&mut engine, "n");
        assert_eq!(engine.snapshot().preedit, "ん");

        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&ImeAction::Commit("ん".to_owned())));
    }

    #[test]
    fn double_n_spends_both_keys_on_one_syllabic_n() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "sennyou");
        assert_eq!(engine.snapshot().preedit, "せんよう");

        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "annnai");
        assert_eq!(engine.snapshot().preedit, "あんない");
    }

    #[test]
    fn ascii_numbers_and_symbols_are_normalized_for_japanese_input() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "123,.!?()[]+-~/@#'");

        assert_eq!(
            engine.snapshot().preedit,
            "１２３、。！？（）「」＋ー〜・＠＃＇"
        );
    }

    #[test]
    fn arrow_shortcuts_are_composed_in_preedit() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "zhzm");

        assert_eq!(engine.snapshot().preedit, "←→");
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&ImeAction::Commit("←→".to_owned())));
    }

    #[test]
    fn foreign_word_with_long_vowel_converts_to_dictionary_candidate() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "pafo-mansu");

        assert_eq!(engine.snapshot().preedit, "ぱふぉーまんす");

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "パフォーマンス");
    }

    #[test]
    fn live_conversion_updates_preedit_and_enter_commits_preview() {
        let mut engine = ImeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
        });

        type_text(&mut engine, "nihongo");
        assert_eq!(engine.snapshot().preedit, "日本語");
        assert_eq!(engine.snapshot().phase, Phase::Composing);

        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&ImeAction::Commit("日本語".to_owned())));
    }

    #[test]
    fn escape_restores_reading_before_clearing_live_conversion() {
        let mut engine = ImeEngine::bundled();
        engine.set_preferences(EnginePreferences {
            live_conversion: true,
            history_completion: false,
            history_learning: false,
            dictionary_packs: 0,
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
            "# unvalley-ime-user-dictionary-v1\nほげ\tHOGE\n",
        )
        .unwrap();
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));

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
            "# unvalley-ime-history-v1\nぱふぉーまんす\tパフォーマンス\t5\t10\n",
        )
        .unwrap();
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
        });

        type_text(&mut engine, "pafo");
        assert_eq!(engine.snapshot().preedit, "ぱふぉ");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
        assert_eq!(engine.snapshot().candidates, ["パフォーマンス"]);

        let actions = engine.handle(InputEvent::AcceptCandidate);
        assert!(actions.contains(&ImeAction::Commit("パフォーマンス".to_owned())));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completions_hide_when_reading_stops_matching_history() {
        let directory = test_directory("completion-stale-hide");
        fs::write(
            directory.join("history.tsv"),
            "# unvalley-ime-history-v1\nどうじしんこう\t同時進行\t5\t10\n",
        )
        .unwrap();
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
        });

        type_text(&mut engine, "dou");
        assert_eq!(engine.snapshot().candidates, ["同時進行"]);

        engine.handle(InputEvent::Character('g'));
        assert_eq!(engine.snapshot().candidates, ["同時進行"]);

        let actions = engine.handle(InputEvent::Character('u'));
        assert!(actions.contains(&ImeAction::HideCandidates));
        assert!(engine.snapshot().candidates.is_empty());
        assert_eq!(engine.snapshot().preedit, "どうぐ");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn accepted_history_completion_is_ranked_first_after_reload() {
        let directory = test_directory("completion-ranking");
        fs::write(
            directory.join("history.tsv"),
            "# unvalley-ime-history-v1\nぱふぉーまんす\tパフォーマンス\t6\t20\nぱふぇづくり\tパフェ作り\t5\t10\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
        };
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(preferences);

        type_text(&mut engine, "pafu");
        assert_eq!(
            engine.snapshot().candidates,
            ["パフォーマンス", "パフェ作り"]
        );
        engine.handle(InputEvent::SelectCandidate(1));
        let actions = engine.handle(InputEvent::Enter);
        assert!(actions.contains(&ImeAction::Commit("パフェ作り".to_owned())));

        let mut reloaded = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
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
            "# unvalley-ime-history-v1\nかんじ\t感じ\t1\t10\n",
        )
        .unwrap();
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
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
            "# unvalley-ime-history-v1\nかんじ\t漢字\t100\t20\nかんじ\t感じ\t1\t10\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
        };
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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

        let mut reloaded = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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
        };
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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

        let mut reloaded = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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
        };
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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
            "# unvalley-ime-history-v1\nかんじへんかん\t漢字変換\t5\t10\nかんじょうひょうげん\t感情表現\t5\t20\n",
        )
        .unwrap();
        let preferences = EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: true,
            dictionary_packs: 0,
        };
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
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
        let original = "# unvalley-ime-history-v1\nかんじ\t感じ\t2\t10\n";
        fs::write(&path, original).unwrap();
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: true,
            history_learning: false,
            dictionary_packs: 0,
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
        let mut engine = ImeEngine::bundled_with_user_data(UserData::load(&directory));
        engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion: false,
            history_learning: true,
            dictionary_packs: 0,
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
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "t'id'yu");

        assert_eq!(engine.snapshot().preedit, "てぃでゅ");
    }

    #[test]
    fn punctuation_resolves_a_trailing_n_before_it_is_inserted() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "hon,");

        assert_eq!(engine.snapshot().preedit, "ほん、");
    }

    #[test]
    fn space_starts_conversion_and_cycles_candidates() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "nihon");

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "日本");
        assert_eq!(engine.snapshot().phase, Phase::Converting);

        engine.handle(InputEvent::Space);
        assert_eq!(engine.snapshot().preedit, "ニホン");
    }

    #[test]
    fn conversion_always_includes_a_unique_full_width_katakana_candidate() {
        let mut engine = ImeEngine::bundled();
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
        let mut engine = ImeEngine::bundled();
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
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "kikan");
        engine.handle(InputEvent::Space);

        let candidates = engine.snapshot().candidates;
        assert!(candidates.len() > 9);
        assert_eq!(candidates[1], "キカン");
    }

    #[test]
    fn selecting_candidate_by_index_updates_preedit_and_commit() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let candidates = engine.snapshot().candidates;
        let selected = candidates[1].clone();
        let actions = engine.handle(InputEvent::SelectCandidate(1));

        assert_eq!(actions, vec![ImeAction::UpdatePreedit(selected.clone())]);
        assert_eq!(engine.snapshot().selected, Some(1));
        assert!(
            engine
                .handle(InputEvent::Enter)
                .contains(&ImeAction::Commit(selected))
        );
    }

    #[test]
    fn selecting_out_of_range_candidate_does_nothing() {
        let mut engine = ImeEngine::bundled();
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
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        let actions = engine.handle(InputEvent::Enter);

        assert!(actions.contains(&ImeAction::Commit("日本".to_owned())));
        assert_eq!(engine.snapshot().preedit, "");
    }

    #[test]
    fn escape_restores_reading_after_conversion() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "nihon");
        engine.handle(InputEvent::Space);

        engine.handle(InputEvent::Escape);

        assert_eq!(engine.snapshot().preedit, "にほん");
        assert_eq!(engine.snapshot().phase, Phase::Composing);
    }

    #[test]
    fn phrase_uses_segmented_conversion() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "watashihanihon");

        engine.handle(InputEvent::Space);

        assert_eq!(engine.snapshot().preedit, "私は日本");
    }

    #[test]
    fn backspace_removes_pending_then_committed_kana() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "kak");
        assert_eq!(engine.snapshot().preedit, "かk");

        engine.handle(InputEvent::Backspace);
        assert_eq!(engine.snapshot().preedit, "か");
        engine.handle(InputEvent::Backspace);
        assert_eq!(engine.snapshot().preedit, "");
    }

    #[test]
    fn empty_control_keys_are_forwarded() {
        let mut engine = ImeEngine::bundled();

        assert_eq!(
            engine.handle(InputEvent::Enter),
            vec![ImeAction::ForwardKey]
        );
        assert_eq!(
            engine.handle(InputEvent::Space),
            vec![ImeAction::ForwardKey]
        );
    }
}
