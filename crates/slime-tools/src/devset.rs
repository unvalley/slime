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

use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use slime_tools::surface_annotation::{SurfaceReadingIndex, contains_kanji, hiragana_to_katakana};

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
    source_split: String,
    index: String,
    context_text: String,
    input: String,
    expected_output: Vec<String>,
    original_text: String,
    #[serde(skip)]
    annotated_tokens: Vec<(String, String)>,
}

struct Options {
    train_path: PathBuf,
    dictionary_path: PathBuf,
    output_path: PathBuf,
    count: usize,
    partition_count: usize,
    partition_index: Option<usize>,
    exclude_partition_index: Option<usize>,
    annotated_output: Option<PathBuf>,
}

fn run() -> Result<(), String> {
    let options = parse_options(env::args().skip(1))?;
    let readings = SurfaceReadingIndex::load(&options.dictionary_path)?;
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

    let partitioned = partition_items(
        accepted,
        options.partition_count,
        options.partition_index,
        options.exclude_partition_index,
    );
    eprintln!("partition contains {} items", partitioned.len());
    if let Some(path) = options.annotated_output {
        write_annotated_corpus(&path, &partitioned)?;
    }
    let selected = sample_evenly(partitioned, options.count);
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
    let usage = "usage: ime-devset <train.jsonl> <mozc-basic.tsv> <output.json> \
                 [--count N] [--partition-count N \
                 (--partition-index N | --exclude-partition-index N)] \
                 [--annotated-output PATH]";
    let train_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let dictionary_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let output_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let mut count = 400;
    let mut partition_count = 1;
    let mut partition_index = None;
    let mut exclude_partition_index = None;
    let mut annotated_output = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--count" => {
                count = arguments
                    .next()
                    .ok_or("--count requires a value")?
                    .parse()
                    .map_err(|_| "--count requires a positive integer")?;
            }
            "--partition-count" => {
                partition_count = arguments
                    .next()
                    .ok_or("--partition-count requires a value")?
                    .parse()
                    .map_err(|_| "--partition-count requires a positive integer")?;
            }
            "--partition-index" => {
                partition_index = Some(
                    arguments
                        .next()
                        .ok_or("--partition-index requires a value")?
                        .parse()
                        .map_err(|_| "--partition-index requires a non-negative integer")?,
                );
            }
            "--exclude-partition-index" => {
                exclude_partition_index = Some(
                    arguments
                        .next()
                        .ok_or("--exclude-partition-index requires a value")?
                        .parse()
                        .map_err(|_| "--exclude-partition-index requires a non-negative integer")?,
                );
            }
            "--annotated-output" => {
                annotated_output = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or("--annotated-output requires a path")?,
                ));
            }
            _ => return Err(format!("unknown argument {argument:?}\n{usage}")),
        }
    }
    if count == 0 {
        return Err("--count requires a positive integer".to_owned());
    }
    if partition_count == 0 {
        return Err("--partition-count requires a positive integer".to_owned());
    }
    if partition_index.is_some() && exclude_partition_index.is_some() {
        return Err(
            "--partition-index and --exclude-partition-index are mutually exclusive".to_owned(),
        );
    }
    let selected_partition = partition_index.or(exclude_partition_index);
    if selected_partition.is_some_and(|index| index >= partition_count) {
        return Err("partition index must be less than --partition-count".to_owned());
    }
    if partition_count > 1 && selected_partition.is_none() {
        return Err(
            "--partition-count requires --partition-index or --exclude-partition-index".to_owned(),
        );
    }
    Ok(Options {
        train_path,
        dictionary_path,
        output_path,
        count,
        partition_count,
        partition_index,
        exclude_partition_index,
        annotated_output,
    })
}

fn partition_items(
    items: Vec<DevItem>,
    partition_count: usize,
    partition_index: Option<usize>,
    exclude_partition_index: Option<usize>,
) -> Vec<DevItem> {
    let Some(selected_index) = partition_index.or(exclude_partition_index) else {
        return items;
    };
    let exclude = exclude_partition_index.is_some();
    items
        .into_iter()
        .filter(|item| {
            item.index.parse::<usize>().is_ok_and(|line_number| {
                let matches = line_number % partition_count == selected_index;
                if exclude { !matches } else { matches }
            })
        })
        .collect()
}

fn write_annotated_corpus(path: &PathBuf, items: &[DevItem]) -> Result<(), String> {
    let mut output = String::new();
    for item in items {
        for (index, (surface, reading)) in item.annotated_tokens.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            output.push_str(surface);
            output.push('/');
            output.push_str(reading);
        }
        output.push('\n');
    }
    fs::write(path, output)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    eprintln!("wrote annotated corpus to {}", path.display());
    Ok(())
}

fn build_item(
    pair: &TrainPair,
    line_number: usize,
    accepted_count: usize,
    readings: &SurfaceReadingIndex,
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
    if !(6..=60).contains(&span_characters) || !contains_kanji(&span) {
        return None;
    }

    let annotated_tokens = readings.annotate(&span)?;
    let reading = annotated_tokens
        .iter()
        .map(|(_, reading)| reading.as_str())
        .collect::<String>();

    // A kana-kanji misconversion types the same reading for both surfaces;
    // if the readings of the two variants are derivable and differ, the
    // reading estimate for this span is not trustworthy.
    if let (Some(pre_reading), Some(post_reading)) = (
        readings.reading(&diff.pre_str),
        readings.reading(&diff.post_str),
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
        source_split: "jwtd-v2-train".to_owned(),
        index: line_number.to_string(),
        context_text,
        input: hiragana_to_katakana(&reading),
        expected_output: vec![span],
        original_text: pair.post_text.clone(),
        annotated_tokens,
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
                source_split: item.source_split.clone(),
                index: item.index.clone(),
                context_text: item.context_text.clone(),
                input: item.input.clone(),
                expected_output: item.expected_output.clone(),
                original_text: item.original_text.clone(),
                annotated_tokens: item.annotated_tokens.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{DevItem, SurfaceReadingIndex, diff_span, hiragana_to_katakana, partition_items};

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
    fn partitions_items_by_stable_source_line_number() {
        let items: Vec<_> = (0..10)
            .map(|index| DevItem {
                source_split: "test".to_owned(),
                index: index.to_string(),
                context_text: String::new(),
                input: String::new(),
                expected_output: vec![String::new()],
                original_text: String::new(),
                annotated_tokens: Vec::new(),
            })
            .collect();
        let selected = partition_items(items, 3, Some(1), None);
        assert_eq!(
            selected
                .iter()
                .map(|item| item.index.as_str())
                .collect::<Vec<_>>(),
            ["1", "4", "7"]
        );
    }

    #[test]
    fn excludes_a_stable_partition_for_training() {
        let items: Vec<_> = (0..10)
            .map(|index| DevItem {
                source_split: "test".to_owned(),
                index: index.to_string(),
                context_text: String::new(),
                input: String::new(),
                expected_output: vec![String::new()],
                original_text: String::new(),
                annotated_tokens: Vec::new(),
            })
            .collect();
        let selected = partition_items(items, 3, None, Some(1));
        assert_eq!(
            selected
                .iter()
                .map(|item| item.index.as_str())
                .collect::<Vec<_>>(),
            ["0", "2", "3", "5", "6", "8", "9"]
        );
    }

    #[test]
    fn annotated_tokens_preserve_the_derived_reading() {
        let readings = SurfaceReadingIndex::from_pairs([
            ("漢字".to_owned(), "かんじ".to_owned()),
            ("変換".to_owned(), "へんかん".to_owned()),
        ]);
        let tokens = readings.annotate("漢字への変換").unwrap();
        assert_eq!(
            tokens,
            [
                ("漢字".to_owned(), "かんじ".to_owned()),
                ("へ".to_owned(), "へ".to_owned()),
                ("の".to_owned(), "の".to_owned()),
                ("変換".to_owned(), "へんかん".to_owned()),
            ]
        );
    }

    #[test]
    fn readings_are_emitted_as_katakana() {
        assert_eq!(hiragana_to_katakana("かけい、ゔ"), "カケイ、ヴ");
    }
}
