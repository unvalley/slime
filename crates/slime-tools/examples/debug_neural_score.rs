//! Prints raw and interpolated neural scores for one candidate set.

use std::env;
use std::path::Path;
use std::process::ExitCode;

use slime_tools::neural::{Rescorer, ScoreRequest};

const COST_LOG_SCALE: f64 = 500.0;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let model = arguments.next().ok_or_else(usage)?;
    let reading = arguments.next().ok_or_else(usage)?;
    let context = arguments.next().ok_or_else(usage)?;
    let lambda = arguments
        .next()
        .ok_or_else(usage)?
        .parse::<f64>()
        .map_err(|_| "LAMBDA must be a finite number between 0 and 1".to_owned())?;
    if !lambda.is_finite() || !(0.0..=1.0).contains(&lambda) {
        return Err("LAMBDA must be a finite number between 0 and 1".to_owned());
    }

    let candidates = arguments
        .map(|argument| parse_candidate(&argument))
        .collect::<Result<Vec<_>, _>>()?;
    if candidates.len() < 2 {
        return Err(usage());
    }
    let request = ScoreRequest {
        context,
        input_katakana: hiragana_to_katakana(&reading),
        candidates: candidates
            .iter()
            .map(|(surface, _)| surface.clone())
            .collect(),
    };
    let rescorer = Rescorer::load(Path::new(&model))?;
    let scored = rescorer.score_interactive(&request)?;
    let mut rows = candidates
        .into_iter()
        .zip(scored.logliks)
        .map(|((surface, cost), loglik)| {
            let base = -f64::from(cost) / COST_LOG_SCALE;
            let combined = (1.0 - lambda) * base + lambda * loglik;
            (surface, cost, base, loglik, combined)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| right.4.total_cmp(&left.4));
    let best = rows.first().map(|row| row.4).unwrap_or_default();
    for (rank, (surface, cost, base, loglik, combined)) in rows.into_iter().enumerate() {
        println!(
            "{}\t{}\tbase={base:.6}\tloglik={loglik:.6}\tcombined={combined:.6}\tgap={:.6}\t{surface}",
            rank + 1,
            cost,
            best - combined,
        );
    }
    Ok(())
}

fn parse_candidate(argument: &str) -> Result<(String, i32), String> {
    let (surface, cost) = argument
        .rsplit_once(':')
        .ok_or_else(|| format!("candidate must be SURFACE:COST: {argument}"))?;
    if surface.is_empty() {
        return Err("candidate surface must not be empty".to_owned());
    }
    let cost = cost
        .parse::<i32>()
        .map_err(|_| format!("candidate cost must be an integer: {argument}"))?;
    Ok((surface.to_owned(), cost))
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

fn usage() -> String {
    "usage: debug_neural_score MODEL READING CONTEXT LAMBDA SURFACE:COST SURFACE:COST [...]"
        .to_owned()
}
