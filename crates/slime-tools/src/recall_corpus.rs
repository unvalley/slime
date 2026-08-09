//! Builds a bounded compound-recall fixture from neutral annotated text.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;
use slime_converter::Dictionary;

const MINIMUM_TOKENS: usize = 2;
const MAXIMUM_TOKENS: usize = 6;
const MINIMUM_READING_CHARACTERS: usize = 4;
const MAXIMUM_READING_CHARACTERS: usize = 16;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let options = Options::parse(arguments)?;
    let mut phrases = Vec::new();
    let mut seen = HashSet::new();
    let mut stats = Stats::default();
    let dictionary = Dictionary::bundled();

    for input in &options.inputs {
        let file = fs::File::open(input)
            .map_err(|error| format!("failed to open {}: {error}", input.display()))?;
        for line in BufReader::new(file).lines() {
            stats.lines += 1;
            let line = line.map_err(|error| {
                format!(
                    "failed to read line {} from {}: {error}",
                    stats.lines,
                    input.display()
                )
            })?;
            let Some(tokens) = parse_tokens(&line) else {
                stats.invalid_lines += 1;
                continue;
            };
            for run in phonetic_runs(&tokens) {
                collect_phrases(run, &dictionary, &mut seen, &mut phrases, &mut stats);
            }
        }
    }

    if phrases.is_empty() {
        return Err("annotated inputs produced no compound phrases".to_owned());
    }
    stats.unique_phrases = phrases.len();
    let selected = sample_evenly(&phrases, options.limit);
    stats.selected_phrases = selected.len();
    let mut output = String::new();
    for phrase in selected {
        output.push_str(&phrase.reading);
        output.push('\t');
        output.push_str(&phrase.surface);
        output.push('\n');
    }
    write_atomically(&options.output, output.as_bytes())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&stats)
            .map_err(|error| format!("failed to serialize statistics: {error}"))?
    );
    Ok(())
}

struct Options {
    inputs: Vec<PathBuf>,
    output: PathBuf,
    limit: usize,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let usage = "usage: slime-recall-corpus --input PATH [--input PATH ...] \
                     --output PATH [--limit N]";
        let mut inputs = Vec::new();
        let mut output = None;
        let mut limit = 2_000;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" => inputs.push(PathBuf::from(arguments.next().ok_or(usage)?)),
                "--output" => output = Some(PathBuf::from(arguments.next().ok_or(usage)?)),
                "--limit" => {
                    limit = arguments
                        .next()
                        .ok_or_else(|| "--limit requires a value".to_owned())?
                        .parse::<usize>()
                        .ok()
                        .filter(|value| *value > 0)
                        .ok_or_else(|| "--limit must be a positive integer".to_owned())?;
                }
                _ => return Err(usage.to_owned()),
            }
        }
        if inputs.is_empty() {
            return Err(usage.to_owned());
        }
        Ok(Self {
            inputs,
            output: output.ok_or(usage)?,
            limit,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Token {
    surface: String,
    reading: String,
}

fn parse_tokens(line: &str) -> Option<Vec<Token>> {
    let tokens: Vec<_> = line
        .split_whitespace()
        .map(|token| {
            let (surface, reading) = token.rsplit_once('/')?;
            if surface.is_empty() || reading.is_empty() {
                return None;
            }
            Some(Token {
                surface: surface.to_owned(),
                reading: reading.to_owned(),
            })
        })
        .collect::<Option<_>>()?;
    (!tokens.is_empty()).then_some(tokens)
}

fn phonetic_runs(tokens: &[Token]) -> impl Iterator<Item = &[Token]> {
    tokens
        .split(|token| !token.reading.chars().all(is_kana))
        .filter(|run| !run.is_empty())
}

fn collect_phrases(
    tokens: &[Token],
    dictionary: &Dictionary,
    seen: &mut HashSet<String>,
    phrases: &mut Vec<Phrase>,
    stats: &mut Stats,
) {
    let suspicious_administrative_tokens = suspicious_administrative_tokens(tokens, dictionary);
    for start in 0..tokens.len() {
        let maximum_end = (start + MAXIMUM_TOKENS).min(tokens.len());
        for end in start + MINIMUM_TOKENS..=maximum_end {
            stats.windows += 1;
            let window = &tokens[start..end];
            if !window.iter().all(is_compound_element) {
                stats.non_compound_elements += 1;
                continue;
            }
            if suspicious_administrative_tokens[start..end]
                .iter()
                .any(|suspicious| *suspicious)
            {
                stats.suspicious_administrative_windows += 1;
                continue;
            }
            if window
                .iter()
                .filter(|token| token.surface.chars().any(is_kanji))
                .count()
                < 2
            {
                stats.insufficient_kanji_elements += 1;
                continue;
            }
            let reading: String = window.iter().map(|token| token.reading.as_str()).collect();
            let reading_characters = reading.chars().count();
            if !(MINIMUM_READING_CHARACTERS..=MAXIMUM_READING_CHARACTERS)
                .contains(&reading_characters)
            {
                stats.outside_reading_length += 1;
                continue;
            }
            let surface: String = window.iter().map(|token| token.surface.as_str()).collect();
            let mut key = String::with_capacity(reading.len() + surface.len() + 1);
            key.push_str(&reading);
            key.push('\0');
            key.push_str(&surface);
            if !seen.insert(key) {
                stats.duplicates += 1;
                continue;
            }
            phrases.push(Phrase { reading, surface });
        }
    }
}

fn suspicious_administrative_tokens(tokens: &[Token], dictionary: &Dictionary) -> Vec<bool> {
    let mut suspicious = vec![false; tokens.len()];
    let mut previous_region = None;
    for (index, token) in tokens.iter().enumerate() {
        if dictionary.has_exact_region_surface(&token.reading, &token.surface) {
            previous_region = Some(index);
        } else if has_administrative_suffix(&token.surface)
            && let Some(region_index) = previous_region
        {
            suspicious[region_index..=index].fill(true);
        }
    }
    suspicious
}

fn has_administrative_suffix(surface: &str) -> bool {
    surface.chars().last().is_some_and(|suffix| {
        matches!(
            suffix,
            '都' | '道' | '府' | '県' | '市' | '区' | '町' | '村' | '郡'
        )
    })
}

fn sample_evenly(phrases: &[Phrase], limit: usize) -> Vec<&Phrase> {
    if phrases.len() <= limit {
        return phrases.iter().collect();
    }
    (0..limit)
        .map(|index| &phrases[index * phrases.len() / limit])
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Phrase {
    reading: String,
    surface: String,
}

fn is_kana(character: char) -> bool {
    matches!(character, 'ぁ'..='ゖ' | 'ゝ' | 'ゞ' | 'ァ'..='ヶ' | 'ー' | 'ヽ' | 'ヾ')
}

fn is_kanji(character: char) -> bool {
    matches!(character, '\u{4e00}'..='\u{9fff}' | '々' | '〆')
}

fn is_compound_element(token: &Token) -> bool {
    let mut characters = token.surface.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    is_lexical_character(first)
        && characters.next().is_some_and(is_lexical_character)
        && characters.all(is_lexical_character)
}

fn is_lexical_character(character: char) -> bool {
    is_kanji(character) || matches!(character, 'ァ'..='ヶ' | 'ー' | 'ヽ' | 'ヾ')
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("output path has no file name: {}", path.display()))?;
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)
            .map_err(|error| format!("failed to create {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("failed to sync {}: {error}", temporary.display()))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("failed to replace {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Default, Serialize)]
struct Stats {
    lines: usize,
    invalid_lines: usize,
    windows: usize,
    non_compound_elements: usize,
    suspicious_administrative_windows: usize,
    insufficient_kanji_elements: usize,
    outside_reading_length: usize,
    duplicates: usize,
    unique_phrases: usize,
    selected_phrases: usize,
}

#[cfg(test)]
mod tests {
    use slime_converter::{Dictionary, DictionaryEntry};

    use super::{Phrase, Stats, collect_phrases, parse_tokens, phonetic_runs, sample_evenly};
    use std::collections::HashSet;

    #[test]
    fn creates_only_bounded_multi_kanji_phrases() {
        let tokens = parse_tokens(
            "新規/しんき 商品/しょうひん を/を 共同/きょうどう 開発/かいはつ する/する 。/。",
        )
        .unwrap();
        let mut phrases = Vec::new();
        let mut seen = HashSet::new();
        let mut stats = Stats::default();
        let dictionary = Dictionary::new(Vec::new());
        for run in phonetic_runs(&tokens) {
            collect_phrases(run, &dictionary, &mut seen, &mut phrases, &mut stats);
        }
        assert!(phrases.contains(&Phrase {
            reading: "しんきしょうひん".to_owned(),
            surface: "新規商品".to_owned(),
        }));
        assert!(phrases.contains(&Phrase {
            reading: "きょうどうかいはつ".to_owned(),
            surface: "共同開発".to_owned(),
        }));
        assert!(phrases.iter().all(|phrase| !phrase.surface.contains('。')));
        assert!(phrases.iter().all(|phrase| !phrase.surface.contains('を')));
        assert!(
            phrases
                .iter()
                .all(|phrase| !phrase.surface.contains("する"))
        );
    }

    #[test]
    fn rejects_inflection_fragments_even_when_each_contains_kanji() {
        let tokens = parse_tokens("微/び 妙に/みょうに 違/ちがい う/う").unwrap();
        let mut phrases = Vec::new();
        let mut seen = HashSet::new();
        let mut stats = Stats::default();
        collect_phrases(
            &tokens,
            &Dictionary::new(Vec::new()),
            &mut seen,
            &mut phrases,
            &mut stats,
        );
        assert!(phrases.is_empty());
        assert!(stats.non_compound_elements > 0);
    }

    #[test]
    fn rejects_misread_administrative_segments_after_a_region_anchor() {
        const REGION_POS_ID: u16 = 1924;
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::with_pos("おきなわけん", "沖縄県", REGION_POS_ID, REGION_POS_ID, 100),
            DictionaryEntry::with_pos("ぎのざむら", "宜野座村", REGION_POS_ID, REGION_POS_ID, 100),
        ]);
        let tokens = parse_tokens("沖縄県/おきなわけん 宜野/よしの 座村/ざむら").unwrap();
        let mut phrases = Vec::new();
        let mut seen = HashSet::new();
        let mut stats = Stats::default();

        collect_phrases(&tokens, &dictionary, &mut seen, &mut phrases, &mut stats);

        assert!(!phrases.iter().any(|phrase| {
            phrase.reading == "おきなわけんよしのざむら" && phrase.surface == "沖縄県宜野座村"
        }));
        assert!(stats.suspicious_administrative_windows > 0);

        let correct = parse_tokens("沖縄県/おきなわけん 宜野座村/ぎのざむら").unwrap();
        let mut correct_phrases = Vec::new();
        collect_phrases(
            &correct,
            &dictionary,
            &mut HashSet::new(),
            &mut correct_phrases,
            &mut Stats::default(),
        );
        assert!(correct_phrases.contains(&Phrase {
            reading: "おきなわけんぎのざむら".to_owned(),
            surface: "沖縄県宜野座村".to_owned(),
        }));
    }

    #[test]
    fn sampling_is_deterministic_and_spans_the_input() {
        let phrases: Vec<_> = (0..10)
            .map(|index| Phrase {
                reading: index.to_string(),
                surface: index.to_string(),
            })
            .collect();
        assert_eq!(
            sample_evenly(&phrases, 3)
                .into_iter()
                .map(|phrase| phrase.reading.as_str())
                .collect::<Vec<_>>(),
            ["0", "3", "6"]
        );
    }
}
