//! Replays one resolved kana reading and prints the delayed LIVE ranking input.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use slime_core::{EnginePreferences, InputEvent, SlimeEngine, UserData};

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let Some(reading) = arguments.next() else {
        eprintln!("usage: debug_live_request READING [DATA_DIR]");
        return ExitCode::FAILURE;
    };
    let data_directory = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        eprintln!("usage: debug_live_request READING [DATA_DIR]");
        return ExitCode::FAILURE;
    }

    let mut engine = data_directory.map_or_else(SlimeEngine::bundled, |directory| {
        SlimeEngine::bundled_with_user_data(UserData::load(&directory))
    });
    engine.set_preferences(EnginePreferences {
        live_conversion: true,
        ..EnginePreferences::default()
    });
    for character in reading.chars() {
        engine.handle(InputEvent::Character(character));
    }

    println!("preedit\t{}", engine.snapshot().preedit);
    for (rank, candidate) in engine
        .conversion_candidates(&reading)
        .into_iter()
        .enumerate()
    {
        println!("explicit-{}\t{}", rank + 1, candidate);
    }
    let Some(snapshot) = engine.live_candidate_ranking_snapshot() else {
        println!("task\tunavailable");
        return ExitCode::SUCCESS;
    };
    println!("resolved-reading\t{}", snapshot.resolved_reading());
    println!("target-literal\t{}", snapshot.target_is_literal());
    let Some(request) = snapshot.candidate_ranking_request() else {
        println!("request\tunavailable");
        return ExitCode::SUCCESS;
    };
    println!("target-reading\t{}", request.reading);
    println!("left-context\t{}", request.left_context);
    for (rank, candidate) in request.candidates.into_iter().enumerate() {
        println!("{}\t{}\t{}", rank + 1, candidate.cost, candidate.surface);
    }
    ExitCode::SUCCESS
}
