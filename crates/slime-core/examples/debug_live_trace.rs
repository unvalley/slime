//! Replays one resolved kana reading and prints every visible LIVE transition.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use slime_core::{EnginePreferences, InputEvent, SlimeEngine, UserData};

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let Some(reading) = arguments.next() else {
        eprintln!("usage: debug_live_trace READING [DATA_DIR]");
        return ExitCode::FAILURE;
    };
    let data_directory = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        eprintln!("usage: debug_live_trace READING [DATA_DIR]");
        return ExitCode::FAILURE;
    }

    let mut engine = data_directory.map_or_else(SlimeEngine::bundled, |directory| {
        SlimeEngine::bundled_with_user_data(UserData::load(&directory))
    });
    engine.set_preferences(EnginePreferences {
        live_conversion: true,
        ..EnginePreferences::default()
    });
    engine.set_delayed_live_ranking_available(env::var_os("SLIME_DEBUG_DELAYED_RANKING").is_some());

    let mut resolved = String::new();
    let mut previous = String::new();
    for character in reading.chars() {
        resolved.push(character);
        engine.handle(InputEvent::Character(character));
        let preedit = engine.snapshot().preedit;
        if preedit != previous {
            let (stable, pending) = engine.evaluation_live_prefixes();
            let word = engine.evaluation_live_word_checkpoint();
            let sealable = engine.evaluation_live_sealable_bunsetsu();
            let literal_extension = engine.evaluation_live_literal_extension_checkpoint();
            let rankable = engine.live_candidate_ranking_snapshot().is_some();
            println!(
                "{resolved}\t{preedit}\tstable={stable:?}\tpending={pending:?}\tword={word:?}\tsealable={sealable:?}\tliteral-extension={literal_extension}\trankable={rankable}"
            );
            previous = preedit;
        }
    }
    if let Some(snapshot) = engine.live_candidate_ranking_snapshot()
        && let Some(request) = snapshot.candidate_ranking_request()
    {
        println!(
            "request\treading={}\tcontext={}\tliteral={}\tcandidates={:?}",
            request.reading,
            request.left_context,
            snapshot.request_target_is_literal(&request),
            request.candidates,
        );
    }
    ExitCode::SUCCESS
}
