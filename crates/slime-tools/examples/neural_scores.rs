//! Inspect lattice costs and neural likelihoods for one interactive conversion.

use std::env;
use std::path::Path;
use std::process::ExitCode;

use slime_converter::Dictionary;
use slime_core::{
    CandidateRankingItem, CandidateRankingRequest, EnginePreferences, InputEvent, SlimeEngine,
    UserData,
};
use slime_tools::neural::{Rescorer, ScoreRequest};

const COST_LOG_SCALE: f64 = 500.0;

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let (Some(model), Some(romaji)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: neural_scores MODEL.gguf ROMAJI [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    };
    let left_context = arguments.next().unwrap_or_default();
    let lambda = env::var("SLIME_NEURAL_LAMBDA")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.2);
    if !(0.0..=1.0).contains(&lambda) {
        eprintln!("SLIME_NEURAL_LAMBDA must be between 0 and 1");
        return ExitCode::FAILURE;
    }

    match inspect(Path::new(&model), &romaji, &left_context, lambda) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn inspect(model: &Path, romaji: &str, left_context: &str, lambda: f64) -> Result<(), String> {
    let live = env::var_os("SLIME_NEURAL_LIVE").is_some();
    let mut engine = env::var_os("SLIME_DATA_DIR").map_or_else(SlimeEngine::bundled, |directory| {
        SlimeEngine::bundled_with_user_data(UserData::load(directory))
    });
    engine.set_preferences(EnginePreferences {
        live_conversion: live,
        history_learning: false,
        ..EnginePreferences::default()
    });
    if !left_context.is_empty() {
        engine.set_external_left_context(left_context);
    }
    for character in romaji.chars() {
        engine.handle(InputEvent::Character(character));
    }
    let mut request = if live {
        engine
            .live_candidate_ranking_snapshot()
            .and_then(|snapshot| snapshot.candidate_ranking_request())
            .ok_or_else(|| "LIVE conversion did not produce a rankable snapshot".to_owned())?
    } else {
        engine.handle(InputEvent::Space);
        engine
            .candidate_ranking_request()
            .ok_or_else(|| "conversion did not produce two dictionary candidates".to_owned())?
    };
    extend_diagnostic_candidates(&mut request);
    let score_request = ScoreRequest {
        context: request.left_context.clone(),
        input_katakana: hiragana_to_katakana(&request.reading),
        candidates: request
            .candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect(),
    };
    let scored = Rescorer::load(model)?.score_interactive(&score_request)?;
    let combined: Vec<_> = request
        .candidates
        .iter()
        .zip(&scored.logliks)
        .map(|(candidate, loglik)| {
            (1.0 - lambda) * (-f64::from(candidate.cost) / COST_LOG_SCALE) + lambda * loglik
        })
        .collect();
    let base = combined[0];
    println!(
        "mode={} reading={} context={:?} lambda={lambda:.3} latency_ms={:.3}",
        if live { "live" } else { "space" },
        request.reading,
        request.left_context,
        scored.latency.as_secs_f64() * 1_000.0
    );
    println!("rank\trequest_index\tsurface\tcost\tloglik\tcombined\tdelta_from_base");
    let mut order: Vec<_> = (0..request.candidates.len()).collect();
    order.sort_by(|&left, &right| combined[right].total_cmp(&combined[left]));
    for (rank, index) in order.into_iter().enumerate() {
        let candidate = &request.candidates[index];
        println!(
            "{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:+.6}",
            rank + 1,
            index,
            candidate.surface,
            candidate.cost,
            scored.logliks[index],
            combined[index],
            combined[index] - base
        );
    }
    Ok(())
}

fn extend_diagnostic_candidates(request: &mut CandidateRankingRequest) {
    if let Some(limit) = positive_env_usize("SLIME_NEURAL_FIXED_SEGMENT_LIMIT") {
        let additional: Vec<_> = Dictionary::bundled()
            .fixed_segment_candidates(&request.reading, 8, limit)
            .into_iter()
            .filter(|candidate| {
                !request
                    .candidates
                    .iter()
                    .any(|existing| existing.surface == candidate.surface)
            })
            .take(limit)
            .collect();
        for candidate in additional {
            request.candidates.push(CandidateRankingItem {
                surface: candidate.surface,
                cost: candidate.cost,
            });
        }
    }
    if let Some(limit) = positive_env_usize("SLIME_NEURAL_RECOMBINED_LIMIT") {
        let additional: Vec<_> = Dictionary::bundled()
            .recombined_n_best_variants(&request.reading, 32, limit)
            .into_iter()
            .filter(|candidate| {
                !request
                    .candidates
                    .iter()
                    .any(|existing| existing.surface == candidate.surface)
            })
            .take(limit)
            .collect();
        for candidate in additional {
            request.candidates.push(CandidateRankingItem {
                surface: candidate.surface,
                cost: candidate.cost,
            });
        }
    }
}

fn positive_env_usize(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn hiragana_to_katakana(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            let code = u32::from(character);
            if (0x3041..=0x3096).contains(&code) {
                char::from_u32(code + 0x60).unwrap_or(character)
            } else {
                character
            }
        })
        .collect()
}
