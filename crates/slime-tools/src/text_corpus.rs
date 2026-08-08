//! Converts plain Japanese text into the neutral surface/reading corpus format
//! used by offline candidate-ranking experiments.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use slime_tools::surface_annotation::{SurfaceReadingIndex, contains_kanji};

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
    let index = SurfaceReadingIndex::load(&options.dictionary)?;
    let exclusions = load_exclusions(&options.exclude_json)?;
    let mut seen = HashSet::new();
    let mut output = String::new();
    let mut stats = Stats {
        dictionary_surfaces: index.len(),
        ..Stats::default()
    };

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
            let text = line.trim();
            if text.is_empty() {
                stats.empty += 1;
                continue;
            }
            let characters = text.chars().count();
            if !(options.minimum_characters..=options.maximum_characters).contains(&characters) {
                stats.outside_length += 1;
                continue;
            }
            if !contains_kanji(text) {
                stats.without_kanji += 1;
                continue;
            }
            if exclusions.contains(text) {
                stats.excluded += 1;
                continue;
            }
            if !seen.insert(text.to_owned()) {
                stats.duplicates += 1;
                continue;
            }
            let Some(tokens) = index.annotate_plain_text(text) else {
                stats.unannotated += 1;
                continue;
            };
            for (position, (surface, reading)) in tokens.iter().enumerate() {
                if position > 0 {
                    output.push(' ');
                }
                output.push_str(surface);
                output.push('/');
                output.push_str(reading);
            }
            output.push('\n');
            stats.accepted += 1;
        }
    }

    if stats.accepted == 0 {
        return Err("no input lines could be annotated".to_owned());
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
    dictionary: PathBuf,
    output: PathBuf,
    inputs: Vec<PathBuf>,
    exclude_json: Vec<PathBuf>,
    minimum_characters: usize,
    maximum_characters: usize,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let usage = "usage: slime-text-corpus --dictionary PATH --output PATH \
                     --input PATH [--input PATH ...] [--exclude-json PATH ...] \
                     [--min-chars N] [--max-chars N]";
        let mut dictionary = None;
        let mut output = None;
        let mut inputs = Vec::new();
        let mut exclude_json = Vec::new();
        let mut minimum_characters = 4;
        let mut maximum_characters = 120;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--dictionary" => {
                    dictionary = Some(PathBuf::from(arguments.next().ok_or(usage)?));
                }
                "--output" => output = Some(PathBuf::from(arguments.next().ok_or(usage)?)),
                "--input" => inputs.push(PathBuf::from(arguments.next().ok_or(usage)?)),
                "--exclude-json" => {
                    exclude_json.push(PathBuf::from(arguments.next().ok_or(usage)?));
                }
                "--min-chars" => {
                    minimum_characters = parse_positive(arguments.next(), "--min-chars")?;
                }
                "--max-chars" => {
                    maximum_characters = parse_positive(arguments.next(), "--max-chars")?;
                }
                _ => return Err(usage.to_owned()),
            }
        }
        if inputs.is_empty() || minimum_characters > maximum_characters {
            return Err(usage.to_owned());
        }
        Ok(Self {
            dictionary: dictionary.ok_or(usage)?,
            output: output.ok_or(usage)?,
            inputs,
            exclude_json,
            minimum_characters,
            maximum_characters,
        })
    }
}

fn parse_positive(value: Option<String>, option: &str) -> Result<usize, String> {
    value
        .ok_or_else(|| format!("{option} requires a value"))?
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{option} must be a positive integer"))
}

#[derive(Deserialize)]
struct EvaluationItem {
    original_text: String,
}

fn load_exclusions(paths: &[PathBuf]) -> Result<HashSet<String>, String> {
    let mut exclusions = HashSet::new();
    for path in paths {
        let bytes = fs::read(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let items: Vec<EvaluationItem> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
        exclusions.extend(
            items
                .into_iter()
                .map(|item| item.original_text.trim().to_owned())
                .filter(|text| !text.is_empty()),
        );
    }
    Ok(exclusions)
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("output path has no file name: {}", path.display()))?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
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
    dictionary_surfaces: usize,
    lines: usize,
    accepted: usize,
    empty: usize,
    outside_length: usize,
    without_kanji: usize,
    excluded: usize,
    duplicates: usize,
    unannotated: usize,
}

#[cfg(test)]
mod tests {
    use super::Options;

    #[test]
    fn parses_repeated_inputs_and_exclusions() {
        let options = Options::parse(
            [
                "--dictionary",
                "dictionary.tsv",
                "--output",
                "corpus.txt",
                "--input",
                "one.txt",
                "--input",
                "two.txt",
                "--exclude-json",
                "held-out.json",
                "--min-chars",
                "6",
                "--max-chars",
                "80",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(options.inputs.len(), 2);
        assert_eq!(options.exclude_json.len(), 1);
        assert_eq!(options.minimum_characters, 6);
        assert_eq!(options.maximum_characters, 80);
    }
}
