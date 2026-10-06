//! Probes an exact conversion through the same user-data and dictionary-pack
//! loading boundary used by platform adapters.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use slime_core::{EnginePreferences, InputEvent, SlimeAction, SlimeEngine, UserData};

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let Some(data_directory) = arguments.next() else {
        eprintln!("usage: data_dir_probe DATA_DIR ROMAJI EXPECTED [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    };
    let Some(romaji) = arguments.next() else {
        eprintln!("usage: data_dir_probe DATA_DIR ROMAJI EXPECTED [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    };
    let Some(expected) = arguments.next() else {
        eprintln!("usage: data_dir_probe DATA_DIR ROMAJI EXPECTED [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    };
    let left_context = arguments.next();
    if arguments.next().is_some() {
        eprintln!("usage: data_dir_probe DATA_DIR ROMAJI EXPECTED [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    }

    let user_data = UserData::load(PathBuf::from(data_directory));
    let mut engine = SlimeEngine::bundled_with_user_data(user_data);
    engine.set_preferences(EnginePreferences {
        live_conversion: true,
        ..EnginePreferences::default()
    });
    if let Some(left_context) = left_context {
        engine.set_external_left_context(&left_context);
    }
    let mut live_preedit = None;
    for character in romaji.chars() {
        let actions = engine.handle(InputEvent::Character(character));
        live_preedit = actions.iter().rev().find_map(|action| match action {
            SlimeAction::UpdatePreedit(preedit)
            | SlimeAction::UpdateSegmentedPreedit { text: preedit, .. } => Some(preedit.clone()),
            _ => None,
        });
    }
    let actions = engine.handle(InputEvent::Space);
    let top = actions.iter().find_map(|action| match action {
        SlimeAction::ShowCandidates { candidates, .. } => candidates.first(),
        _ => None,
    });
    match top {
        Some(top) if top == &expected && live_preedit.as_deref() == Some(expected.as_str()) => {
            println!("live={expected} top={top}");
            ExitCode::SUCCESS
        }
        Some(top) => {
            eprintln!(
                "expected={expected} live={} top={top}",
                live_preedit.as_deref().unwrap_or("<none>")
            );
            ExitCode::FAILURE
        }
        None => {
            eprintln!("conversion produced no candidates");
            ExitCode::FAILURE
        }
    }
}
