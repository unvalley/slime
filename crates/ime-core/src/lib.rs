//! Platform-independent IME state machine.

use ime_converter::Dictionary;
use ime_romaji::RomajiComposer;

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
        }
    }

    #[must_use]
    pub fn bundled() -> Self {
        Self::new(Dictionary::bundled())
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
        if self.candidates.is_empty() {
            Phase::Composing
        } else {
            Phase::Converting
        }
    }

    pub fn handle(&mut self, event: InputEvent) -> Vec<ImeAction> {
        match event {
            InputEvent::Character(character) => self.handle_character(character),
            InputEvent::Space | InputEvent::NextCandidate => self.next_or_convert(),
            InputEvent::PreviousCandidate => self.previous_candidate(),
            InputEvent::SelectCandidate(index) => self.select_candidate(index),
            InputEvent::Enter => self.commit(),
            InputEvent::Escape => self.cancel(),
            InputEvent::Backspace => self.backspace(),
        }
    }

    fn handle_character(&mut self, character: char) -> Vec<ImeAction> {
        let mut actions = Vec::with_capacity(3);
        if self.phase() == Phase::Converting {
            actions.push(ImeAction::Commit(self.selected_candidate().to_owned()));
            self.clear_composition();
            actions.push(ImeAction::HideCandidates);
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

        actions.push(ImeAction::UpdatePreedit(self.preedit()));
        actions
    }

    fn next_or_convert(&mut self) -> Vec<ImeAction> {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1) % self.candidates.len();
            return self.candidate_actions();
        }

        self.reading.push_str(&self.romaji.flush());
        if self.reading.is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        self.candidates = self
            .dictionary
            .candidates(&self.reading)
            .into_iter()
            .map(|candidate| candidate.surface)
            .collect();
        self.selected = 0;
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
        self.candidate_actions()
    }

    fn select_candidate(&mut self, index: u32) -> Vec<ImeAction> {
        let index = index as usize;
        if index >= self.candidates.len() {
            return Vec::new();
        }

        self.selected = index;
        vec![ImeAction::UpdatePreedit(
            self.selected_candidate().to_owned(),
        )]
    }

    fn candidate_actions(&self) -> Vec<ImeAction> {
        vec![
            ImeAction::UpdatePreedit(self.selected_candidate().to_owned()),
            ImeAction::ShowCandidates {
                candidates: self.candidates.clone(),
                selected: self.selected,
            },
        ]
    }

    fn commit(&mut self) -> Vec<ImeAction> {
        let committed = if self.candidates.is_empty() {
            self.reading.push_str(&self.romaji.flush());
            self.reading.clone()
        } else {
            self.selected_candidate().to_owned()
        };

        if committed.is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        let was_converting = !self.candidates.is_empty();
        self.clear_composition();
        let mut actions = vec![ImeAction::Commit(committed), ImeAction::Clear];
        if was_converting {
            actions.push(ImeAction::HideCandidates);
        }
        actions
    }

    fn cancel(&mut self) -> Vec<ImeAction> {
        if !self.candidates.is_empty() {
            self.candidates.clear();
            self.selected = 0;
            return vec![
                ImeAction::HideCandidates,
                ImeAction::UpdatePreedit(self.preedit()),
            ];
        }

        if self.reading.is_empty() && self.romaji.pending().is_empty() {
            return vec![ImeAction::ForwardKey];
        }

        self.clear_composition();
        vec![ImeAction::Clear]
    }

    fn backspace(&mut self) -> Vec<ImeAction> {
        if !self.candidates.is_empty() {
            self.candidates.clear();
            self.selected = 0;
            return vec![
                ImeAction::HideCandidates,
                ImeAction::UpdatePreedit(self.preedit()),
            ];
        }

        if !self.romaji.backspace() {
            self.reading.pop();
        }

        let preedit = self.preedit();
        if preedit.is_empty() {
            vec![ImeAction::Clear]
        } else {
            vec![ImeAction::UpdatePreedit(preedit)]
        }
    }

    fn preedit(&self) -> String {
        if !self.candidates.is_empty() {
            return self.selected_candidate().to_owned();
        }

        let mut preedit = self.reading.clone();
        preedit.push_str(self.romaji.preview());
        preedit
    }

    fn selected_candidate(&self) -> &str {
        &self.candidates[self.selected]
    }

    fn clear_composition(&mut self) {
        self.romaji.clear();
        self.reading.clear();
        self.candidates.clear();
        self.selected = 0;
    }
}

fn normalize_ascii_character(character: char) -> char {
    match character {
        '-' => 'ー',
        '~' => '〜',
        ',' => '、',
        '.' => '。',
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
    use super::{ImeAction, ImeEngine, InputEvent, Phase};

    fn type_text(engine: &mut ImeEngine, input: &str) {
        for character in input.chars() {
            engine.handle(InputEvent::Character(character));
        }
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
    fn double_n_can_still_begin_the_following_na_row_syllable() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "annai");

        assert_eq!(engine.snapshot().preedit, "あんない");
    }

    #[test]
    fn ascii_numbers_and_symbols_are_normalized_for_japanese_input() {
        let mut engine = ImeEngine::bundled();
        type_text(&mut engine, "123,.!?()[]+-~/@#'");

        assert_eq!(
            engine.snapshot().preedit,
            "１２３、。！？（）「」＋ー〜／＠＃＇"
        );
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
