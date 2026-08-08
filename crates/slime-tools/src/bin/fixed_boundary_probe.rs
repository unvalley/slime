//! Evaluation-only probe for bounded alternatives on the best segmentation.

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::Deserialize;
use slime_converter::Dictionary;

const INITIAL_CANDIDATES: usize = 10;
const ALTERNATIVES_PER_SEGMENT: usize = 8;
const REPORT_LIMITS: [usize; 4] = [16, 32, 64, 128];

#[derive(Debug, Deserialize)]
struct Item {
    input: String,
    expected_output: Vec<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let path = env::args_os()
        .nth(1)
        .ok_or("usage: fixed_boundary_probe INPUT.json")?;
    let items: Vec<Item> = serde_json::from_str(
        &fs::read_to_string(Path::new(&path))
            .map_err(|error| format!("failed to read input: {error}"))?,
    )
    .map_err(|error| format!("failed to parse input: {error}"))?;
    let dictionary = Dictionary::bundled();
    let mut initial_hits = 0usize;
    let mut expanded_hits = [0usize; REPORT_LIMITS.len()];
    let mut durations = Vec::with_capacity(items.len());

    for item in &items {
        let reading = katakana_to_hiragana(&item.input);
        let initial = dictionary
            .candidates_with_limit(&reading, INITIAL_CANDIDATES)
            .into_iter()
            .take(INITIAL_CANDIDATES)
            .map(|candidate| candidate.surface)
            .collect::<Vec<_>>();
        let initial_hit = contains_expected(&initial, &item.expected_output);
        initial_hits += usize::from(initial_hit);

        for (index, limit) in REPORT_LIMITS.iter().copied().enumerate() {
            let variant_limit = limit.saturating_sub(initial.len());
            let started = Instant::now();
            let variants = dictionary.fixed_segment_variants(
                &reading,
                ALTERNATIVES_PER_SEGMENT,
                variant_limit,
            );
            if limit == 32 {
                durations.push(started.elapsed());
            }
            let mut merged = initial.clone();
            for variant in variants {
                push_unique(&mut merged, variant);
            }
            expanded_hits[index] += usize::from(contains_expected(&merged, &item.expected_output));
        }
    }

    durations.sort_unstable();
    println!("items={}", items.len());
    println!("initial@10={initial_hits}");
    for (limit, hits) in REPORT_LIMITS.into_iter().zip(expanded_hits) {
        println!("fixed-boundary@{limit}={hits}");
    }
    println!("p50_ms={:.4}", percentile(&durations, 50));
    println!("p95_ms={:.4}", percentile(&durations, 95));
    Ok(())
}

fn contains_expected(candidates: &[String], expected: &[String]) -> bool {
    candidates
        .iter()
        .any(|candidate| expected.iter().any(|expected| expected == candidate))
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn percentile(samples: &[Duration], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = (samples.len() - 1)
        .saturating_mul(percentile)
        .saturating_add(99)
        / 100;
    samples[index].as_secs_f64() * 1_000.0
}

fn katakana_to_hiragana(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            '\u{30A1}'..='\u{30F6}' => {
                char::from_u32(u32::from(character) - 0x60).expect("valid hiragana scalar")
            }
            _ => character,
        })
        .collect()
}
