//! Builds a kana-kanji conversion development set from the JWTD v2 train
//! split.
//!
//! AJIMEE-Bench is derived from the JWTD test split, so it must stay a
//! held-out reporting set. This tool produces AJIMEE-compatible items from the
//! train split for cost and model tuning.
//!
//! For every train pair whose single diff is a `kanji-conversion_a` error, the
//! corrected sentence is cut to a conversion window around the error, and the
//! window's reading is estimated by greedy longest-match reverse lookup over
//! the bundled dictionary (surface to reading). Only unambiguous readings are
//! accepted: a surface with several distinct dictionary readings rejects the
//! item, and when both the wrong and the corrected surface have derivable
//! readings they must match (a kana-kanji misconversion preserves the typed
//! reading).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Deserialize)]
struct TrainPair {
    pre_text: String,
    post_text: String,
    diffs: Vec<TrainDiff>,
}

#[derive(Debug, Deserialize)]
struct TrainDiff {
    pre_str: String,
    post_str: String,
    category: String,
}

#[derive(Debug, Serialize)]
struct DevItem {
    index: String,
    context_text: String,
    input: String,
    expected_output: Vec<String>,
    original_text: String,
}

struct Options {
    train_path: PathBuf,
    dictionary_path: PathBuf,
    output_path: PathBuf,
    count: usize,
}

fn run() -> Result<(), String> {
    let options = parse_options(env::args().skip(1))?;
    let readings = load_surface_readings(&options.dictionary_path)?;
    eprintln!("loaded {} unambiguous surface readings", readings.len());

    let file = fs::File::open(&options.train_path)
        .map_err(|error| format!("failed to open {}: {error}", options.train_path.display()))?;
    let mut accepted = Vec::new();
    let mut seen_spans = std::collections::HashSet::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| format!("failed to read train data: {error}"))?;
        let Ok(pair) = serde_json::from_str::<TrainPair>(&line) else {
            continue;
        };
        let Some(item) = build_item(&pair, line_number, accepted.len(), &readings) else {
            continue;
        };
        if seen_spans.insert(item.expected_output[0].clone()) {
            accepted.push(item);
        }
    }
    eprintln!("accepted {} candidate items", accepted.len());

    let selected = sample_evenly(accepted, options.count);
    let json = serde_json::to_string_pretty(&selected)
        .map_err(|error| format!("failed to serialize items: {error}"))?;
    fs::write(&options.output_path, json)
        .map_err(|error| format!("failed to write {}: {error}", options.output_path.display()))?;
    eprintln!(
        "wrote {} items to {}",
        selected.len(),
        options.output_path.display()
    );
    Ok(())
}

fn parse_options(mut arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let usage = "usage: ime-devset <train.jsonl> <mozc-basic.tsv> <output.json> [--count N]";
    let train_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let dictionary_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let output_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let mut count = 400;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--count" => {
                count = arguments
                    .next()
                    .ok_or("--count requires a value")?
                    .parse()
                    .map_err(|_| "--count requires a positive integer")?;
            }
            _ => return Err(format!("unknown argument {argument:?}\n{usage}")),
        }
    }
    Ok(Options {
        train_path,
        dictionary_path,
        output_path,
        count,
    })
}

/// Maps each dictionary surface to its reading when every dictionary entry
/// for that surface agrees on one reading; ambiguous surfaces map to `None`
/// so lookups can distinguish "unknown" from "ambiguous".
fn load_surface_readings(path: &PathBuf) -> Result<HashMap<String, Option<String>>, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut readings: HashMap<String, Option<String>> = HashMap::new();
    for line in content.lines() {
        let mut columns = line.split('\t');
        let (Some(reading), Some(surface)) = (columns.next(), columns.next()) else {
            continue;
        };
        readings
            .entry(surface.to_owned())
            .and_modify(|existing| {
                if existing.as_deref() != Some(reading) {
                    *existing = None;
                }
            })
            .or_insert_with(|| Some(reading.to_owned()));
    }
    readings.retain(|_, reading| reading.is_some());
    Ok(readings)
}

fn build_item(
    pair: &TrainPair,
    line_number: usize,
    accepted_count: usize,
    readings: &HashMap<String, Option<String>>,
) -> Option<DevItem> {
    let [diff] = pair.diffs.as_slice() else {
        return None;
    };
    if diff.category != "kanji-conversion_a" || diff.pre_str.is_empty() || diff.post_str.is_empty()
    {
        return None;
    }

    let pre: Vec<char> = pair.pre_text.chars().collect();
    let post: Vec<char> = pair.post_text.chars().collect();
    let (diff_start, diff_end) = diff_span(&pre, &post)?;

    let sentence_start = (0..diff_start)
        .rev()
        .find(|&index| matches!(post[index], '。' | '！' | '？'))
        .map_or(0, |index| index + 1);
    let window_start = window_start(&post, diff_start, sentence_start);
    let window_end = window_end(&post, diff_end);
    let span: String = post[window_start..window_end].iter().collect();

    let span_characters = window_end - window_start;
    if !(6..=60).contains(&span_characters) || !span.chars().any(is_kanji) {
        return None;
    }

    let reading = derive_reading(&post[window_start..window_end], readings)?;

    // A kana-kanji misconversion types the same reading for both surfaces;
    // if the readings of the two variants are derivable and differ, the
    // reading estimate for this span is not trustworthy.
    let pre_chars: Vec<char> = diff.pre_str.chars().collect();
    let post_chars: Vec<char> = diff.post_str.chars().collect();
    if let (Some(pre_reading), Some(post_reading)) = (
        derive_reading(&pre_chars, readings),
        derive_reading(&post_chars, readings),
    ) && pre_reading != post_reading
    {
        return None;
    }

    let context_text = if accepted_count.is_multiple_of(2) {
        String::new()
    } else {
        post[sentence_start..window_start].iter().collect()
    };

    Some(DevItem {
        index: line_number.to_string(),
        context_text,
        input: hiragana_to_katakana(&reading),
        expected_output: vec![span],
        original_text: pair.post_text.clone(),
    })
}

/// Returns the changed region of `post` as char indices, assuming a single
/// contiguous edit between the two texts.
fn diff_span(pre: &[char], post: &[char]) -> Option<(usize, usize)> {
    if pre == post {
        return None;
    }
    let common_prefix = pre
        .iter()
        .zip(post.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let maximum_suffix = pre.len().min(post.len()) - common_prefix;
    let common_suffix = pre
        .iter()
        .rev()
        .zip(post.iter().rev())
        .take_while(|(left, right)| left == right)
        .count()
        .min(maximum_suffix);
    let start = common_prefix;
    let end = post.len() - common_suffix;
    (start < end).then_some((start, end))
}

fn window_start(post: &[char], diff_start: usize, sentence_start: usize) -> usize {
    let mut start = diff_start;
    let mut boundaries = 0;
    for _ in 0..25 {
        if start == sentence_start {
            break;
        }
        if is_clause_boundary(post[start - 1]) {
            boundaries += 1;
            if boundaries == 2 {
                break;
            }
        }
        start -= 1;
    }
    start
}

fn window_end(post: &[char], diff_end: usize) -> usize {
    let mut end = diff_end;
    for _ in 0..12 {
        if end == post.len() || is_clause_boundary(post[end]) {
            break;
        }
        end += 1;
    }
    end
}

fn is_clause_boundary(character: char) -> bool {
    matches!(
        character,
        '、' | '。' | '！' | '？' | '「' | '」' | '『' | '』' | '（' | '）' | '：' | '；'
    )
}

fn is_kanji(character: char) -> bool {
    matches!(character, '\u{4e00}'..='\u{9fff}' | '々' | '〆')
}

fn is_kana(character: char) -> bool {
    matches!(character, 'ぁ'..='ゖ' | 'ゝ' | 'ゞ' | 'ァ'..='ヶ' | 'ー' | 'ヽ' | 'ヾ')
}

const MAXIMUM_SURFACE_CHARACTERS: usize = 12;

/// Estimates the reading of `span` by greedy longest-match over unambiguous
/// dictionary surfaces, falling back to kana and punctuation passthrough.
/// Returns `None` when any part of the span cannot be read confidently.
fn derive_reading(span: &[char], readings: &HashMap<String, Option<String>>) -> Option<String> {
    let mut reading = String::new();
    let mut position = 0;
    while position < span.len() {
        let mut matched = false;
        let longest = MAXIMUM_SURFACE_CHARACTERS.min(span.len() - position);
        for length in (2..=longest).rev() {
            let surface: String = span[position..position + length].iter().collect();
            if let Some(Some(surface_reading)) = readings.get(&surface) {
                reading.push_str(surface_reading);
                position += length;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }

        let character = span[position];
        if is_kana(character) {
            reading.push(katakana_to_hiragana(character));
            position += 1;
        } else if is_clause_boundary(character) || character == '・' || character == '…' {
            reading.push(character);
            position += 1;
        } else if let Some(Some(surface_reading)) = readings.get(&character.to_string()) {
            reading.push_str(surface_reading);
            position += 1;
        } else {
            return None;
        }
    }
    Some(reading)
}

fn katakana_to_hiragana(character: char) -> char {
    match character {
        'ァ'..='ヶ' | 'ヽ' | 'ヾ' => {
            char::from_u32(u32::from(character) - 0x60).expect("valid hiragana scalar")
        }
        _ => character,
    }
}

fn hiragana_to_katakana(reading: &str) -> String {
    reading
        .chars()
        .map(|character| match character {
            'ぁ'..='ゖ' | 'ゝ' | 'ゞ' => {
                char::from_u32(u32::from(character) + 0x60).expect("valid katakana scalar")
            }
            _ => character,
        })
        .collect()
}

/// Deterministically spreads the selection across the corpus so one Wikipedia
/// page cannot dominate the set.
fn sample_evenly(items: Vec<DevItem>, count: usize) -> Vec<DevItem> {
    if items.len() <= count {
        return items;
    }
    (0..count)
        .map(|index| {
            let position = index * items.len() / count;
            let item = &items[position];
            DevItem {
                index: item.index.clone(),
                context_text: item.context_text.clone(),
                input: item.input.clone(),
                expected_output: item.expected_output.clone(),
                original_text: item.original_text.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{diff_span, hiragana_to_katakana};

    #[test]
    fn diff_span_finds_the_changed_region() {
        let pre: Vec<char> = "固体発生の議論".chars().collect();
        let post: Vec<char> = "個体発生の議論".chars().collect();
        assert_eq!(diff_span(&pre, &post), Some((0, 1)));

        let pre: Vec<char> = "ああいう".chars().collect();
        let post: Vec<char> = "ああそういう".chars().collect();
        assert_eq!(diff_span(&pre, &post), Some((2, 4)));
    }

    #[test]
    fn readings_are_emitted_as_katakana() {
        assert_eq!(hiragana_to_katakana("かけい、ゔ"), "カケイ、ヴ");
    }
}
