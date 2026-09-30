//! Prints bounded lattice paths and their segment boundaries for one reading.

use std::env;
use std::process::ExitCode;

use slime_converter::Dictionary;

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let Some(reading) = arguments.next() else {
        eprintln!("usage: debug_core_reading READING [LIMIT] [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    };
    let limit = match arguments.next() {
        Some(value) => {
            if let Ok(value @ 1..=128) = value.parse::<usize>() {
                value
            } else {
                eprintln!("LIMIT must be between 1 and 128");
                return ExitCode::FAILURE;
            }
        }
        None => 10,
    };
    let left_context = arguments.next();
    if arguments.next().is_some() {
        eprintln!("usage: debug_core_reading READING [LIMIT] [LEFT_CONTEXT]");
        return ExitCode::FAILURE;
    }

    let dictionary = Dictionary::bundled();
    if let Some(left_context) = left_context.as_deref() {
        println!("context-ranked candidates:");
        for (rank, candidate) in dictionary
            .candidates_with_context_limit(&reading, left_context, limit)
            .into_iter()
            .enumerate()
        {
            println!("{}	{}	{}", rank + 1, candidate.cost, candidate.surface);
        }
        println!("lattice paths:");
    }
    for (rank, conversion) in dictionary
        .convert_n_best(&reading, limit)
        .into_iter()
        .enumerate()
    {
        println!("{}\t{}\t{}", rank + 1, conversion.cost, conversion.surface);
        for segment in conversion.segments {
            println!(
                "  {}\t{}\t{}",
                segment.cost, segment.reading, segment.surface
            );
        }
    }
    ExitCode::SUCCESS
}
