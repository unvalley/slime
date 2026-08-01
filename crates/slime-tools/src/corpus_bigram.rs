//! Evaluation-only adjacent and skip-bigram ranker.
//!
//! This intentionally stays outside `slime-converter`: a corpus model must
//! prove its quality, coverage, size, and latency before it becomes a bundled
//! runtime dependency.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use slime_converter::{CandidateRanker, Conversion};

const BOS: &str = "<BOS>";
const EOS: &str = "<EOS>";

type TransitionKey = (String, String, String, String);
type Token = (String, String);

#[derive(Debug)]
struct Entry {
    previous_surface: Box<str>,
    previous_reading: Box<str>,
    current_surface: Box<str>,
    current_reading: Box<str>,
    count: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TransitionDiagnostics {
    pub(crate) entries: usize,
    pub(crate) weight: i32,
    pub(crate) transitions_scored: u64,
    pub(crate) matched_transitions: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Diagnostics {
    pub(crate) candidates_scored: u64,
    pub(crate) word: Option<TransitionDiagnostics>,
    pub(crate) skip: Option<TransitionDiagnostics>,
}

#[derive(Debug)]
struct TransitionTable {
    entries: Vec<Entry>,
    weight: i32,
    transitions_scored: Cell<u64>,
    matched_transitions: Cell<u64>,
}

impl TransitionTable {
    fn new(counts: BTreeMap<TransitionKey, u32>, weight: i32) -> Self {
        let entries = counts
            .into_iter()
            .map(
                |(
                    (previous_surface, previous_reading, current_surface, current_reading),
                    count,
                )| {
                    Entry {
                        previous_surface: previous_surface.into_boxed_str(),
                        previous_reading: previous_reading.into_boxed_str(),
                        current_surface: current_surface.into_boxed_str(),
                        current_reading: current_reading.into_boxed_str(),
                        count,
                    }
                },
            )
            .collect();
        Self {
            entries,
            weight,
            transitions_scored: Cell::new(0),
            matched_transitions: Cell::new(0),
        }
    }

    fn is_enabled(&self) -> bool {
        self.weight > 0
    }

    fn diagnostics(&self) -> Option<TransitionDiagnostics> {
        self.is_enabled().then(|| TransitionDiagnostics {
            entries: self.entries.len(),
            weight: self.weight,
            transitions_scored: self.transitions_scored.get(),
            matched_transitions: self.matched_transitions.get(),
        })
    }

    fn count(
        &self,
        previous_surface: &str,
        previous_reading: &str,
        current_surface: &str,
        current_reading: &str,
    ) -> u32 {
        self.entries
            .binary_search_by(|entry| {
                entry
                    .previous_surface
                    .as_ref()
                    .cmp(previous_surface)
                    .then(entry.previous_reading.as_ref().cmp(previous_reading))
                    .then(entry.current_surface.as_ref().cmp(current_surface))
                    .then(entry.current_reading.as_ref().cmp(current_reading))
            })
            .map_or(0, |index| self.entries[index].count)
    }

    fn bonus(
        &self,
        previous_surface: &str,
        previous_reading: &str,
        current_surface: &str,
        current_reading: &str,
    ) -> i32 {
        if !self.is_enabled() {
            return 0;
        }
        self.transitions_scored
            .set(self.transitions_scored.get().saturating_add(1));
        let count = self.count(
            previous_surface,
            previous_reading,
            current_surface,
            current_reading,
        );
        if count == 0 {
            return 0;
        }
        self.matched_transitions
            .set(self.matched_transitions.get().saturating_add(1));
        let logarithmic_count = i32::try_from(count.ilog2() + 1).expect("u32 log fits in i32");
        self.weight.saturating_mul(logarithmic_count)
    }
}

#[derive(Debug)]
pub(crate) struct CorpusBigramRanker {
    word: TransitionTable,
    skip: TransitionTable,
    candidates_scored: Cell<u64>,
}

impl CorpusBigramRanker {
    pub(crate) fn load(
        paths: &[PathBuf],
        word_weight: i32,
        skip_weight: i32,
    ) -> Result<Self, String> {
        debug_assert!(word_weight > 0 || skip_weight > 0);
        let mut word_counts = BTreeMap::new();
        let mut skip_counts = BTreeMap::new();
        for path in paths {
            let source = fs::read_to_string(path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            for (line_index, line) in source.lines().enumerate() {
                let tokens = parse_annotated_corpus_line(line)
                    .map_err(|error| format!("{}:{}: {error}", path.display(), line_index + 1))?;
                count_word_transitions(&tokens, &mut word_counts);
                count_skip_transitions(&tokens, &mut skip_counts);
            }
        }

        Ok(Self {
            word: TransitionTable::new(word_counts, word_weight),
            skip: TransitionTable::new(skip_counts, skip_weight),
            candidates_scored: Cell::new(0),
        })
    }

    pub(crate) fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            candidates_scored: self.candidates_scored.get(),
            word: self.word.diagnostics(),
            skip: self.skip.diagnostics(),
        }
    }
}

impl CandidateRanker for CorpusBigramRanker {
    fn ranking_cost(&self, _reading: &str, conversion: &Conversion) -> i32 {
        self.candidates_scored
            .set(self.candidates_scored.get().saturating_add(1));
        let mut ranking_cost = conversion.cost;

        let mut previous = (BOS, "");
        for segment in &conversion.segments {
            ranking_cost = ranking_cost.saturating_sub(self.word.bonus(
                previous.0,
                previous.1,
                &segment.surface,
                &segment.reading,
            ));
            previous = (&segment.surface, &segment.reading);
        }
        ranking_cost =
            ranking_cost.saturating_sub(self.word.bonus(previous.0, previous.1, EOS, ""));

        for segments in conversion.segments.windows(3) {
            ranking_cost = ranking_cost.saturating_sub(self.skip.bonus(
                &segments[0].surface,
                &segments[0].reading,
                &segments[2].surface,
                &segments[2].reading,
            ));
        }
        ranking_cost
    }
}

fn count_word_transitions(tokens: &[Token], counts: &mut BTreeMap<TransitionKey, u32>) {
    if tokens.is_empty() {
        return;
    }
    let mut previous = (BOS, "");
    for (surface, reading) in tokens {
        increment(counts, previous, (surface, reading));
        previous = (surface, reading);
    }
    increment(counts, previous, (EOS, ""));
}

fn count_skip_transitions(tokens: &[Token], counts: &mut BTreeMap<TransitionKey, u32>) {
    for tokens in tokens.windows(3) {
        increment(
            counts,
            (&tokens[0].0, &tokens[0].1),
            (&tokens[2].0, &tokens[2].1),
        );
    }
}

fn increment(
    counts: &mut BTreeMap<TransitionKey, u32>,
    previous: (&str, &str),
    current: (&str, &str),
) {
    let key = (
        previous.0.to_owned(),
        previous.1.to_owned(),
        current.0.to_owned(),
        current.1.to_owned(),
    );
    let count = counts.entry(key).or_default();
    *count = count.saturating_add(1);
}

fn parse_annotated_corpus_line(line: &str) -> Result<Vec<Token>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(";;") {
        return Ok(Vec::new());
    }
    line.split_whitespace()
        .map(|token| {
            let (surface, reading) = token
                .rsplit_once('/')
                .ok_or_else(|| format!("expected surface/reading token, got {token:?}"))?;
            if surface.is_empty() || reading.is_empty() {
                return Err(format!(
                    "surface and reading must not be empty in {token:?}"
                ));
            }
            Ok((surface.to_owned(), reading.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{BOS, CorpusBigramRanker, parse_annotated_corpus_line};
    use slime_converter::{CandidateRanker, Conversion, Segment};
    use std::fs;

    #[test]
    fn parses_surface_reading_tokens_and_comments() {
        assert!(
            parse_annotated_corpus_line(";; comment")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            parse_annotated_corpus_line("夏/なつ は/は 暑い/あつい").unwrap(),
            [
                ("夏".to_owned(), "なつ".to_owned()),
                ("は".to_owned(), "は".to_owned()),
                ("暑い".to_owned(), "あつい".to_owned()),
            ]
        );
        assert!(parse_annotated_corpus_line("invalid").is_err());
    }

    #[test]
    fn rewards_observed_word_and_skip_transitions() {
        let corpus_path = std::env::temp_dir().join(format!(
            "slime-tools-word-bigram-{}.txt",
            std::process::id()
        ));
        fs::write(&corpus_path, "夏/なつ は/は 暑い/あつい\n").unwrap();
        let ranker =
            CorpusBigramRanker::load(std::slice::from_ref(&corpus_path), 500, 500).unwrap();
        assert_eq!(ranker.word.count(BOS, "", "夏", "なつ"), 1);
        assert_eq!(ranker.skip.count("夏", "なつ", "暑い", "あつい"), 1);

        let observed = conversion("夏", "暑い");
        let unseen = conversion("板", "厚い");
        assert!(
            ranker.ranking_cost("なつはあつい", &observed)
                < ranker.ranking_cost("いたはあつい", &unseen)
        );
        let diagnostics = ranker.diagnostics();
        assert_eq!(diagnostics.candidates_scored, 2);
        assert_eq!(diagnostics.word.unwrap().transitions_scored, 8);
        assert_eq!(diagnostics.skip.unwrap().transitions_scored, 2);
        fs::remove_file(corpus_path).unwrap();
    }

    fn conversion(subject: &str, adjective: &str) -> Conversion {
        Conversion {
            surface: format!("{subject}は{adjective}"),
            segments: vec![
                Segment {
                    reading: if subject == "夏" { "なつ" } else { "いた" }.to_owned(),
                    surface: subject.to_owned(),
                    cost: 1_000,
                },
                Segment {
                    reading: "は".to_owned(),
                    surface: "は".to_owned(),
                    cost: 1_000,
                },
                Segment {
                    reading: "あつい".to_owned(),
                    surface: adjective.to_owned(),
                    cost: 1_000,
                },
            ],
            cost: 3_000,
        }
    }
}
