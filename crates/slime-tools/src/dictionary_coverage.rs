//! Measures dictionary coverage against source-independent annotated corpora.
//!
//! Corpus lines contain whitespace-separated `surface/reading` tokens. A
//! dictionary source is either a tab-separated `reading<TAB>surface` file or
//! an SKK dictionary. Repeating a label combines several source files without
//! rewriting them into a temporary merged dictionary.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;

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
    let corpus = Corpus::load(&options.corpus_paths)?;
    let mut reports = HashMap::<String, DictionaryReport>::new();

    for source in &options.sources {
        let report = reports
            .entry(source.label.clone())
            .or_insert_with(|| DictionaryReport::new(&source.label));
        report.add_source(source, &corpus)?;
    }

    let mut reports: Vec<_> = reports
        .into_values()
        .map(|report| report.finish(&corpus))
        .collect();
    reports.sort_unstable_by(|left, right| left.label.cmp(&right.label));
    let output = Output {
        corpus: corpus.summary(),
        dictionaries: reports,
    };

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .map_err(|error| format!("failed to serialize report: {error}"))?
        );
    } else {
        print_report(&output);
    }
    Ok(())
}

const fn usage() -> &'static str {
    "usage: slime-dictionary-coverage --corpus PATH [--corpus PATH ...] \
     --dictionary LABEL FORMAT PATH [--dictionary LABEL FORMAT PATH ...] [--json]\n\
     FORMAT is tsv (reading<TAB>surface) or skk"
}

#[derive(Debug)]
struct Options {
    corpus_paths: Vec<PathBuf>,
    sources: Vec<DictionarySource>,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut corpus_paths = Vec::new();
        let mut sources = Vec::new();
        let mut json = false;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--corpus" => corpus_paths.push(PathBuf::from(
                    arguments.next().ok_or("--corpus requires PATH")?,
                )),
                "--dictionary" => {
                    let label = arguments.next().ok_or("--dictionary requires LABEL")?;
                    let format = DictionaryFormat::parse(
                        &arguments.next().ok_or("--dictionary requires FORMAT")?,
                    )?;
                    let path = PathBuf::from(arguments.next().ok_or("--dictionary requires PATH")?);
                    sources.push(DictionarySource {
                        label,
                        format,
                        path,
                    });
                }
                "--json" => json = true,
                "--help" | "-h" => return Err(usage().to_owned()),
                _ => return Err(format!("unknown argument {argument:?}\n{}", usage())),
            }
        }
        if corpus_paths.is_empty() || sources.is_empty() {
            return Err(usage().to_owned());
        }
        Ok(Self {
            corpus_paths,
            sources,
            json,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum DictionaryFormat {
    Tsv,
    Skk,
}

impl DictionaryFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "tsv" => Ok(Self::Tsv),
            "skk" => Ok(Self::Skk),
            _ => Err(format!(
                "unsupported dictionary format {value:?}; expected tsv or skk"
            )),
        }
    }
}

#[derive(Debug)]
struct DictionarySource {
    label: String,
    format: DictionaryFormat,
    path: PathBuf,
}

#[derive(Debug)]
struct CorpusToken {
    reading: String,
    pair_key: String,
}

#[derive(Debug)]
struct Corpus {
    files: usize,
    tokens: Vec<CorpusToken>,
    pairs_by_reading: HashMap<String, HashSet<String>>,
}

impl Corpus {
    fn load(paths: &[PathBuf]) -> Result<Self, String> {
        let mut tokens = Vec::new();
        let mut pairs_by_reading = HashMap::<String, HashSet<String>>::new();
        for path in paths {
            let file = fs::File::open(path)
                .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
            for line in BufReader::new(file).lines() {
                let line =
                    line.map_err(|error| format!("failed to read {}: {error}", path.display()))?;
                for token in line.split_whitespace() {
                    let Some((surface, reading)) = token.rsplit_once('/') else {
                        continue;
                    };
                    if surface.is_empty() || reading.is_empty() {
                        continue;
                    }
                    let reading = normalize_reading(reading).into_owned();
                    if surface == reading {
                        continue;
                    }
                    pairs_by_reading
                        .entry(reading.clone())
                        .or_default()
                        .insert(surface.to_owned());
                    tokens.push(CorpusToken {
                        pair_key: pair_key(&reading, surface),
                        reading,
                    });
                }
            }
        }
        if tokens.is_empty() {
            return Err("corpus contains no non-literal surface/reading tokens".to_owned());
        }
        Ok(Self {
            files: paths.len(),
            tokens,
            pairs_by_reading,
        })
    }

    fn summary(&self) -> CorpusSummary {
        CorpusSummary {
            files: self.files,
            token_occurrences: self.tokens.len(),
            unique_readings: self.pairs_by_reading.len(),
            unique_pairs: self.pairs_by_reading.values().map(HashSet::len).sum(),
        }
    }
}

#[derive(Debug)]
struct DictionaryReport {
    label: String,
    source_files: usize,
    source_bytes: u64,
    entries: usize,
    covered_readings: HashSet<String>,
    covered_pairs: HashSet<String>,
}

impl DictionaryReport {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            source_files: 0,
            source_bytes: 0,
            entries: 0,
            covered_readings: HashSet::new(),
            covered_pairs: HashSet::new(),
        }
    }

    fn add_source(&mut self, source: &DictionarySource, corpus: &Corpus) -> Result<(), String> {
        self.source_files += 1;
        self.source_bytes += fs::metadata(&source.path)
            .map_err(|error| format!("failed to inspect {}: {error}", source.path.display()))?
            .len();
        for_each_entry(source, |reading, surface| {
            self.entries += 1;
            let reading = normalize_reading(reading);
            let Some(surfaces) = corpus.pairs_by_reading.get(reading.as_ref()) else {
                return;
            };
            if surfaces.contains(surface) {
                self.covered_pairs
                    .insert(pair_key(reading.as_ref(), surface));
            }
            self.covered_readings.insert(reading.into_owned());
        })
    }

    fn finish(self, corpus: &Corpus) -> CoverageSummary {
        let covered_token_occurrences = corpus
            .tokens
            .iter()
            .filter(|token| self.covered_pairs.contains(&token.pair_key))
            .count();
        let reading_covered_token_occurrences = corpus
            .tokens
            .iter()
            .filter(|token| self.covered_readings.contains(&token.reading))
            .count();
        CoverageSummary {
            label: self.label,
            source_files: self.source_files,
            source_bytes: self.source_bytes,
            entries: self.entries,
            exact_pair: CoverageCounts {
                covered_occurrences: covered_token_occurrences,
                covered_unique: self.covered_pairs.len(),
                total_occurrences: corpus.tokens.len(),
                total_unique: corpus.pairs_by_reading.values().map(HashSet::len).sum(),
            },
            reading: CoverageCounts {
                covered_occurrences: reading_covered_token_occurrences,
                covered_unique: self.covered_readings.len(),
                total_occurrences: corpus.tokens.len(),
                total_unique: corpus.pairs_by_reading.len(),
            },
        }
    }
}

fn for_each_entry(
    source: &DictionarySource,
    mut visit: impl FnMut(&str, &str),
) -> Result<(), String> {
    let file = fs::File::open(&source.path)
        .map_err(|error| format!("failed to open {}: {error}", source.path.display()))?;
    for line in BufReader::new(file).lines() {
        let line =
            line.map_err(|error| format!("failed to read {}: {error}", source.path.display()))?;
        match source.format {
            DictionaryFormat::Tsv => visit_tsv_entries(&line, &mut visit),
            DictionaryFormat::Skk => visit_skk_entries(&line, &mut visit),
        }
    }
    Ok(())
}

fn visit_tsv_entries(line: &str, visit: &mut impl FnMut(&str, &str)) {
    let mut columns = line.split('\t');
    if let (Some(reading), Some(surface)) = (columns.next(), columns.next())
        && !reading.is_empty()
        && !surface.is_empty()
    {
        visit(reading, surface);
    }
}

fn visit_skk_entries(line: &str, visit: &mut impl FnMut(&str, &str)) {
    if line.starts_with(';') {
        return;
    }
    let Some((reading, candidates)) = line.split_once(char::is_whitespace) else {
        return;
    };
    if reading.is_empty()
        || reading
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_alphabetic())
    {
        return;
    }
    for candidate in candidates.trim().trim_matches('/').split('/') {
        let surface = candidate
            .split_once(';')
            .map_or(candidate, |(value, _)| value);
        if !surface.is_empty() && !surface.starts_with('[') {
            visit(reading, surface);
        }
    }
}

fn normalize_reading(reading: &str) -> Cow<'_, str> {
    if reading
        .chars()
        .any(|character| ('ァ'..='ヶ').contains(&character))
    {
        Cow::Owned(
            reading
                .chars()
                .map(|character| {
                    if ('ァ'..='ヶ').contains(&character) {
                        char::from_u32(u32::from(character) - 0x60).unwrap_or(character)
                    } else {
                        character
                    }
                })
                .collect(),
        )
    } else {
        Cow::Borrowed(reading)
    }
}

fn pair_key(reading: &str, surface: &str) -> String {
    let mut key = String::with_capacity(reading.len() + surface.len() + 1);
    key.push_str(reading);
    key.push('\0');
    key.push_str(surface);
    key
}

#[derive(Debug, Serialize)]
struct Output {
    corpus: CorpusSummary,
    dictionaries: Vec<CoverageSummary>,
}

#[derive(Debug, Serialize)]
struct CorpusSummary {
    files: usize,
    token_occurrences: usize,
    unique_readings: usize,
    unique_pairs: usize,
}

#[derive(Debug, Serialize)]
struct CoverageSummary {
    label: String,
    source_files: usize,
    source_bytes: u64,
    entries: usize,
    exact_pair: CoverageCounts,
    reading: CoverageCounts,
}

#[derive(Debug, Serialize)]
struct CoverageCounts {
    covered_occurrences: usize,
    covered_unique: usize,
    total_occurrences: usize,
    total_unique: usize,
}

fn print_report(output: &Output) {
    println!(
        "corpus: {} files, {} non-literal tokens, {} unique readings, {} unique pairs",
        output.corpus.files,
        output.corpus.token_occurrences,
        output.corpus.unique_readings,
        output.corpus.unique_pairs
    );
    for dictionary in &output.dictionaries {
        println!("{}:", dictionary.label);
        println!(
            "  sources={} bytes={} entries={}",
            dictionary.source_files, dictionary.source_bytes, dictionary.entries
        );
        println!(
            "  exact-pair occurrences={}/{} ({:.2}%) unique={}/{} ({:.2}%)",
            dictionary.exact_pair.covered_occurrences,
            dictionary.exact_pair.total_occurrences,
            percentage(
                dictionary.exact_pair.covered_occurrences,
                dictionary.exact_pair.total_occurrences
            ),
            dictionary.exact_pair.covered_unique,
            dictionary.exact_pair.total_unique,
            percentage(
                dictionary.exact_pair.covered_unique,
                dictionary.exact_pair.total_unique
            )
        );
        println!(
            "  reading occurrences={}/{} ({:.2}%) unique={}/{} ({:.2}%)",
            dictionary.reading.covered_occurrences,
            dictionary.reading.total_occurrences,
            percentage(
                dictionary.reading.covered_occurrences,
                dictionary.reading.total_occurrences
            ),
            dictionary.reading.covered_unique,
            dictionary.reading.total_unique,
            percentage(
                dictionary.reading.covered_unique,
                dictionary.reading.total_unique
            )
        );
    }
}

#[allow(clippy::cast_precision_loss)]
fn percentage(covered: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * covered as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_reader_uses_first_two_columns() {
        let mut entries = Vec::new();
        visit_tsv_entries("かんじ\t漢字\t1\t2\t300", &mut |reading, surface| {
            entries.push((reading.to_owned(), surface.to_owned()));
        });
        assert_eq!(entries, [("かんじ".to_owned(), "漢字".to_owned())]);
    }

    #[test]
    fn skk_reader_removes_annotations_and_skips_okuri_entries() {
        let mut entries = Vec::new();
        visit_skk_entries(
            "かんじ /漢字;annotation/感じ/",
            &mut |reading, surface| {
                entries.push((reading.to_owned(), surface.to_owned()));
            },
        );
        visit_skk_entries("おくr /送/", &mut |reading, surface| {
            entries.push((reading.to_owned(), surface.to_owned()));
        });
        assert_eq!(
            entries,
            [
                ("かんじ".to_owned(), "漢字".to_owned()),
                ("かんじ".to_owned(), "感じ".to_owned())
            ]
        );
    }

    #[test]
    fn katakana_readings_are_normalized() {
        assert_eq!(normalize_reading("カンジ"), "かんじ");
        assert!(matches!(normalize_reading("かんじ"), Cow::Borrowed(_)));
    }

    #[test]
    fn hiragana_surfaces_are_literal_even_when_pronunciation_is_katakana() {
        let corpus_path = env::temp_dir().join(format!(
            "slime-dictionary-coverage-{}-{}.txt",
            std::process::id(),
            line!()
        ));
        fs::write(&corpus_path, "する/スル 漢字/カンジ\n").expect("write corpus fixture");
        let corpus = Corpus::load(std::slice::from_ref(&corpus_path)).expect("load corpus");
        fs::remove_file(corpus_path).expect("remove corpus fixture");

        assert_eq!(corpus.tokens.len(), 1);
        assert_eq!(corpus.tokens[0].reading, "かんじ");
    }
}
