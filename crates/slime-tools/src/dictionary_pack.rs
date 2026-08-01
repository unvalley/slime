//! Validates external Slime dictionary packs without installing them.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

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
    let usage = "usage: slime-dictionary-pack validate <pack.slime-dict> [...]";
    if arguments.next().as_deref() != Some("validate") {
        return Err(usage.to_owned());
    }
    let paths: Vec<PathBuf> = arguments.map(PathBuf::from).collect();
    if paths.is_empty() {
        return Err(usage.to_owned());
    }

    for path in paths {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let info = slime_core::validate_dictionary_pack(&source)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        println!(
            "v{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            info.format_version,
            info.id,
            info.name,
            info.version,
            info.license,
            info.minimum_slime_version.as_deref().unwrap_or("-"),
            info.published_at.as_deref().unwrap_or("-"),
            info.entry_count
        );
    }
    Ok(())
}
