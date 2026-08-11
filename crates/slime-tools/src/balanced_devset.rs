//! Builds an external-domain kana-kanji ranking benchmark from a UD Japanese
//! treebank carrying `UniDic` pronunciation metadata.
//!
//! The source corpus contains news/blog sentences, manual token boundaries,
//! `UniDic` part-of-speech tags, and surface pronunciations. For each sentence,
//! this tool selects at most one content word whose reading maps to multiple
//! bundled-dictionary surfaces. Both bounded sides of the target are retained
//! so an editing-time scorer can be evaluated separately from end-of-document
//! conversion.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use slime_tools::surface_annotation::{SurfaceReadingIndex, hiragana_to_katakana};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Options {
    conllu_path: PathBuf,
    dictionary_path: PathBuf,
    output_path: PathBuf,
    source_split: String,
    count: Option<usize>,
    annotated_output: Option<PathBuf>,
    phrase_window: Option<PhraseWindowLength>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhraseWindowLength {
    minimum: usize,
    maximum: usize,
}

impl Default for PhraseWindowLength {
    fn default() -> Self {
        Self {
            minimum: 6,
            maximum: 20,
        }
    }
}

#[derive(Debug, Default)]
struct Sentence {
    id: String,
    text: String,
    tokens: Vec<Token>,
}

#[derive(Debug)]
struct Token {
    id: String,
    surface: String,
    upos: String,
    pronunciation: String,
    space_after: bool,
}

#[derive(Debug, Serialize)]
struct EvaluationItem {
    source_split: String,
    index: String,
    target_upos: String,
    context_text: String,
    right_context_text: String,
    input: String,
    expected_output: Vec<String>,
    original_text: String,
}

fn run() -> Result<(), String> {
    let options = parse_options(env::args().skip(1))?;
    let dictionary = load_reading_surfaces(&options.dictionary_path)?;
    let surface_readings = SurfaceReadingIndex::load(&options.dictionary_path)?;
    let source = fs::read_to_string(&options.conllu_path)
        .map_err(|error| format!("failed to read {}: {error}", options.conllu_path.display()))?;
    let sentences = parse_conllu(&source);
    if let Some(path) = &options.annotated_output {
        write_annotated_corpus(path, &sentences)?;
    }
    let mut items = build_items(
        &sentences,
        &dictionary,
        &options.source_split,
        options.phrase_window,
        &surface_readings,
    );
    let accepted = items.len();
    if let Some(count) = options.count {
        items = sample_evenly(items, count);
    }
    let json = serde_json::to_string_pretty(&items)
        .map_err(|error| format!("failed to serialize benchmark: {error}"))?;
    fs::write(&options.output_path, json)
        .map_err(|error| format!("failed to write {}: {error}", options.output_path.display()))?;
    eprintln!(
        "accepted {accepted} ambiguous items from {} sentences; wrote {} to {}",
        sentences.len(),
        items.len(),
        options.output_path.display()
    );
    Ok(())
}

fn parse_options(mut arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let usage = "usage: slime-balanced-devset <input.conllu> <mozc-basic.tsv> <output.json> \
                 --source-split NAME [--count N] [--annotated-output PATH] \
                 [--phrase-windows [--phrase-min-reading N] [--phrase-max-reading N]]";
    let conllu_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let dictionary_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let output_path = PathBuf::from(arguments.next().ok_or(usage)?);
    let mut source_split = None;
    let mut count = None;
    let mut annotated_output = None;
    let mut phrase_windows = false;
    let mut phrase_minimum = None;
    let mut phrase_maximum = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--source-split" => {
                source_split = Some(arguments.next().ok_or("--source-split requires a value")?);
            }
            "--count" => {
                let parsed = arguments
                    .next()
                    .ok_or("--count requires a value")?
                    .parse()
                    .map_err(|_| "--count requires a positive integer")?;
                if parsed == 0 {
                    return Err("--count requires a positive integer".to_owned());
                }
                count = Some(parsed);
            }
            "--annotated-output" => {
                annotated_output = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or("--annotated-output requires a value")?,
                ));
            }
            "--phrase-windows" => phrase_windows = true,
            "--phrase-min-reading" => {
                phrase_minimum = Some(parse_positive_usize(
                    arguments.next(),
                    "--phrase-min-reading",
                )?);
            }
            "--phrase-max-reading" => {
                phrase_maximum = Some(parse_positive_usize(
                    arguments.next(),
                    "--phrase-max-reading",
                )?);
            }
            _ => return Err(format!("unknown argument {argument:?}\n{usage}")),
        }
    }
    if !phrase_windows && (phrase_minimum.is_some() || phrase_maximum.is_some()) {
        return Err("phrase reading bounds require --phrase-windows".to_owned());
    }
    let phrase_window = if phrase_windows {
        let defaults = PhraseWindowLength::default();
        let length = PhraseWindowLength {
            minimum: phrase_minimum.unwrap_or(defaults.minimum),
            maximum: phrase_maximum.unwrap_or(defaults.maximum),
        };
        if length.minimum > length.maximum {
            return Err("--phrase-min-reading cannot exceed --phrase-max-reading".to_owned());
        }
        Some(length)
    } else {
        None
    };
    Ok(Options {
        conllu_path,
        dictionary_path,
        output_path,
        source_split: source_split.ok_or("--source-split is required")?,
        count,
        annotated_output,
        phrase_window,
    })
}

fn parse_positive_usize(value: Option<String>, option: &str) -> Result<usize, String> {
    let parsed = value
        .ok_or_else(|| format!("{option} requires a value"))?
        .parse()
        .map_err(|_| format!("{option} requires a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{option} requires a positive integer"));
    }
    Ok(parsed)
}

fn write_annotated_corpus(path: &PathBuf, sentences: &[Sentence]) -> Result<(), String> {
    let mut output = String::new();
    for sentence in sentences {
        let tokens = sentence
            .tokens
            .iter()
            .filter(|token| !token.surface.contains(['/', ' ', '\t']));
        for (index, token) in tokens.enumerate() {
            if index > 0 {
                output.push(' ');
            }
            output.push_str(&token.surface);
            output.push('/');
            output.push_str(&katakana_to_hiragana(&token.pronunciation));
        }
        output.push('\n');
    }
    fs::write(path, output)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    eprintln!("wrote annotated corpus to {}", path.display());
    Ok(())
}

fn load_reading_surfaces(path: &PathBuf) -> Result<HashMap<String, HashSet<String>>, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut dictionary: HashMap<String, HashSet<String>> = HashMap::new();
    for line in content.lines() {
        let mut columns = line.split('\t');
        let (Some(reading), Some(surface)) = (columns.next(), columns.next()) else {
            continue;
        };
        dictionary
            .entry(katakana_to_hiragana(reading))
            .or_default()
            .insert(surface.to_owned());
    }
    Ok(dictionary)
}

fn parse_conllu(source: &str) -> Vec<Sentence> {
    let mut sentences = Vec::new();
    let mut sentence = Sentence::default();
    for line in source.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !sentence.tokens.is_empty() {
                sentences.push(sentence);
                sentence = Sentence::default();
            }
            continue;
        }
        if let Some(id) = line.strip_prefix("# sent_id = ") {
            id.clone_into(&mut sentence.id);
            continue;
        }
        if let Some(text) = line.strip_prefix("# text = ") {
            text.clone_into(&mut sentence.text);
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let columns: Vec<_> = line.split('\t').collect();
        if columns.len() != 10 || columns[0].contains(['-', '.']) {
            continue;
        }
        let misc = columns[9];
        let pronunciation = unidic_pronunciation(misc)
            .or_else(|| matches!(columns[3], "PUNCT" | "SYM").then_some(columns[1]));
        let Some(pronunciation) = pronunciation else {
            continue;
        };
        sentence.tokens.push(Token {
            id: columns[0].to_owned(),
            surface: columns[1].to_owned(),
            upos: columns[3].to_owned(),
            pronunciation: pronunciation.to_owned(),
            space_after: !misc.split('|').any(|field| field == "SpaceAfter=No"),
        });
    }
    sentences
}

fn unidic_pronunciation(misc: &str) -> Option<&str> {
    let values = misc
        .split('|')
        .find_map(|field| field.strip_prefix("UnidicInfo="))?;
    let pronunciation = values.split(',').nth(4)?;
    (!pronunciation.is_empty()).then_some(pronunciation)
}

fn build_items(
    sentences: &[Sentence],
    dictionary: &HashMap<String, HashSet<String>>,
    source_split: &str,
    phrase_window: Option<PhraseWindowLength>,
    surface_readings: &SurfaceReadingIndex,
) -> Vec<EvaluationItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for sentence in sentences {
        let mut context = String::new();
        let mut candidates = Vec::new();
        for (token_index, token) in sentence.tokens.iter().enumerate() {
            let reading = katakana_to_hiragana(&token.pronunciation);
            if is_content_word(&token.upos)
                && context.chars().count() >= 2
                && token.surface.chars().any(is_kanji)
                && reading.chars().count() <= 20
                && let Some(surfaces) = dictionary.get(&reading)
                && surfaces.contains(&token.surface)
            {
                let ambiguous_kanji = surfaces
                    .iter()
                    .filter(|surface| surface.chars().any(is_kanji))
                    .count();
                if ambiguous_kanji >= 2 {
                    candidates.push((ambiguous_kanji, token_index, token, context.clone()));
                }
            }
            context.push_str(&token.surface);
            if token.space_after {
                context.push(' ');
            }
        }
        let Some((_, token_index, token, _)) = candidates
            .into_iter()
            .max_by_key(|(ambiguity, _, _, context)| (*ambiguity, context.chars().count()))
        else {
            continue;
        };
        let identity = (token.pronunciation.as_str(), token.surface.as_str());
        if !seen.insert(identity) {
            continue;
        }
        let phrase_readings = phrase_window.map(|_| {
            sentence
                .tokens
                .iter()
                .map(|token| phrase_token_reading(token, surface_readings))
                .collect::<Vec<_>>()
        });
        let (window_start, window_end) = if let Some(length) = phrase_window {
            let Some(bounds) = phrase_window_bounds(
                &sentence.tokens,
                phrase_readings.as_deref().expect("phrase readings exist"),
                token_index,
                length,
            ) else {
                continue;
            };
            bounds
        } else {
            (token_index, token_index + 1)
        };
        let window = &sentence.tokens[window_start..window_end];
        let context_text = render_surface(&sentence.tokens[..window_start]);
        let right_context_text = render_surface(&sentence.tokens[window_end..]);
        let input = if let Some(readings) = &phrase_readings {
            phrase_window_reading(&readings[window_start..window_end])
        } else {
            token.pronunciation.clone()
        };
        let expected = render_surface(window);
        items.push(EvaluationItem {
            source_split: source_split.to_owned(),
            index: format!("{}:{}", sentence.id, token.id),
            target_upos: token.upos.clone(),
            context_text: context_text
                .chars()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            right_context_text: right_context_text.chars().take(40).collect(),
            input,
            expected_output: vec![expected],
            original_text: sentence.text.clone(),
        });
    }
    items
}

fn phrase_window_bounds(
    tokens: &[Token],
    readings: &[String],
    target: usize,
    length: PhraseWindowLength,
) -> Option<(usize, usize)> {
    let mut start = target;
    let mut end = target + 1;
    let mut reading_characters = readings[target].chars().count();
    while reading_characters < length.minimum {
        let left = start.checked_sub(1).filter(|index| {
            !is_phrase_boundary(&tokens[*index])
                && reading_characters + readings[*index].chars().count() <= length.maximum
        });
        let right = (end < tokens.len()).then_some(end).filter(|index| {
            !is_phrase_boundary(&tokens[*index])
                && reading_characters + readings[*index].chars().count() <= length.maximum
        });
        let Some(index) = left.or(right) else {
            break;
        };
        reading_characters += readings[index].chars().count();
        if index < start {
            start = index;
        } else {
            end = index + 1;
        }
    }
    (length.minimum..=length.maximum)
        .contains(&reading_characters)
        .then_some((start, end))
}

fn is_phrase_boundary(token: &Token) -> bool {
    matches!(token.upos.as_str(), "PUNCT" | "SYM")
}

fn render_surface(tokens: &[Token]) -> String {
    let mut surface = String::new();
    for token in tokens {
        surface.push_str(&token.surface);
        if token.space_after {
            surface.push(' ');
        }
    }
    surface
}

fn phrase_token_reading(token: &Token, surface_readings: &SurfaceReadingIndex) -> String {
    surface_readings.reading(&token.surface).map_or_else(
        || token.pronunciation.clone(),
        |reading| hiragana_to_katakana(&reading),
    )
}

fn phrase_window_reading(readings: &[String]) -> String {
    readings.concat()
}

fn is_content_word(upos: &str) -> bool {
    matches!(upos, "ADJ" | "ADV" | "NOUN" | "PROPN" | "VERB")
}

fn is_kanji(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

fn katakana_to_hiragana(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if ('ァ'..='ヶ').contains(&character) {
                char::from_u32(character as u32 - 0x60).unwrap_or(character)
            } else {
                character
            }
        })
        .collect()
}

fn sample_evenly(items: Vec<EvaluationItem>, count: usize) -> Vec<EvaluationItem> {
    if items.len() <= count {
        return items;
    }
    let length = items.len();
    let selected: HashSet<_> = (0..count).map(|index| index * length / count).collect();
    items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| selected.contains(&index).then_some(item))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_surface_pronunciation_from_unidic_info() {
        let misc = "SpaceAfter=No|UnidicInfo=ツカウ,使う,使わ,使う,ツカワ,,,ツカウ,ツカウ,使う";
        assert_eq!(unidic_pronunciation(misc), Some("ツカワ"));
    }

    #[test]
    fn creates_one_contextual_ambiguous_item_per_sentence() {
        let source = "# sent_id = test-1\n# text = 私は魚を食べる。\n1\t私\t私\tPRON\t代名詞\t_\t3\tnsubj\t_\tSpaceAfter=No|UnidicInfo=ワタシ,私,私,私,ワタシ\n2\tは\tは\tADP\t助詞\t_\t1\tcase\t_\tSpaceAfter=No|UnidicInfo=ハ,は,は,は,ワ\n3\t魚\t魚\tNOUN\t名詞\t_\t5\tobj\t_\tSpaceAfter=No|UnidicInfo=サカナ,魚,魚,魚,サカナ\n4\tを\tを\tADP\t助詞\t_\t3\tcase\t_\tSpaceAfter=No|UnidicInfo=ヲ,を,を,を,オ\n5\t食べる\t食べる\tVERB\t動詞\t_\t0\troot\t_\tSpaceAfter=No|UnidicInfo=タベル,食べる,食べ,食べる,タベル\n6\t。\t。\tPUNCT\t補助記号\t_\t5\tpunct\t_\tSpaceAfter=No\n\n";
        let sentences = parse_conllu(source);
        let dictionary = HashMap::from([(
            "さかな".to_owned(),
            HashSet::from(["魚".to_owned(), "肴".to_owned()]),
        )]);
        let surface_readings = SurfaceReadingIndex::from_pairs([]);
        let items = build_items(&sentences, &dictionary, "test", None, &surface_readings);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].target_upos, "NOUN");
        assert_eq!(items[0].context_text, "私は");
        assert_eq!(items[0].right_context_text, "を食べる。");
        assert_eq!(items[0].input, "サカナ");
        assert_eq!(items[0].expected_output, ["魚"]);
    }

    #[test]
    fn creates_a_long_phrase_window_without_crossing_punctuation() {
        let source = "# sent_id = test-1\n# text = 私は魚を食べる。\n1\t私\t私\tPRON\t代名詞\t_\t3\tnsubj\t_\tSpaceAfter=No|UnidicInfo=ワタシ,私,私,私,ワタシ\n2\tは\tは\tADP\t助詞\t_\t1\tcase\t_\tSpaceAfter=No|UnidicInfo=ハ,は,は,は,ワ\n3\t魚\t魚\tNOUN\t名詞\t_\t5\tobj\t_\tSpaceAfter=No|UnidicInfo=サカナ,魚,魚,魚,サカナ\n4\tを\tを\tADP\t助詞\t_\t3\tcase\t_\tSpaceAfter=No|UnidicInfo=ヲ,を,を,を,オ\n5\t食べる\t食べる\tVERB\t動詞\t_\t0\troot\t_\tSpaceAfter=No|UnidicInfo=タベル,食べる,食べ,食べる,タベル\n6\t。\t。\tPUNCT\t補助記号\t_\t5\tpunct\t_\tSpaceAfter=No\n\n";
        let sentences = parse_conllu(source);
        let dictionary = HashMap::from([(
            "さかな".to_owned(),
            HashSet::from(["魚".to_owned(), "肴".to_owned()]),
        )]);
        let surface_readings = SurfaceReadingIndex::from_pairs([
            ("私".to_owned(), "わたし".to_owned()),
            ("魚".to_owned(), "さかな".to_owned()),
        ]);
        let items = build_items(
            &sentences,
            &dictionary,
            "test",
            Some(PhraseWindowLength::default()),
            &surface_readings,
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].context_text, "");
        assert_eq!(items[0].right_context_text, "を食べる。");
        assert_eq!(items[0].input, "ワタシハサカナ");
        assert_eq!(items[0].expected_output, ["私は魚"]);
    }

    #[test]
    fn supports_a_custom_long_phrase_window() {
        let tokens = (0..5)
            .map(|index| Token {
                id: (index + 1).to_string(),
                surface: format!("語{index}"),
                upos: "NOUN".to_owned(),
                pronunciation: "アアアアアアアア".to_owned(),
                space_after: false,
            })
            .collect::<Vec<_>>();
        let readings = tokens
            .iter()
            .map(|token| token.pronunciation.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            phrase_window_bounds(
                &tokens,
                &readings,
                2,
                PhraseWindowLength {
                    minimum: 33,
                    maximum: 40,
                },
            ),
            Some((0, 5))
        );
    }

    #[test]
    fn measures_phrase_bounds_from_the_emitted_readings() {
        let tokens = (0..5)
            .map(|index| Token {
                id: (index + 1).to_string(),
                surface: format!("語{index}"),
                upos: "NOUN".to_owned(),
                pronunciation: "アアアアアアアア".to_owned(),
                space_after: false,
            })
            .collect::<Vec<_>>();
        let readings = vec!["アアアアアア".to_owned(); 5];

        assert_eq!(
            phrase_window_bounds(
                &tokens,
                &readings,
                2,
                PhraseWindowLength {
                    minimum: 33,
                    maximum: 40,
                },
            ),
            None
        );
    }

    #[test]
    fn rejects_phrase_bounds_without_phrase_windows() {
        let arguments = [
            "input.conllu",
            "dictionary.tsv",
            "output.json",
            "--source-split",
            "test",
            "--phrase-min-reading",
            "33",
        ]
        .into_iter()
        .map(str::to_owned);

        assert_eq!(
            parse_options(arguments).unwrap_err(),
            "phrase reading bounds require --phrase-windows"
        );
    }
}
