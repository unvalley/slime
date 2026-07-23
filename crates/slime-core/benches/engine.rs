use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::time::Instant;

use slime_core::{ALL_DOMAIN_DICTIONARIES, EnginePreferences, InputEvent, SlimeEngine, UserData};

fn main() {
    let iterations = iterations(50_000);

    run("engine/nihon_conversion", iterations, || {
        let mut engine = SlimeEngine::bundled();
        for character in black_box("nihon").chars() {
            black_box(engine.handle(InputEvent::Character(character)));
        }
        black_box(engine.handle(InputEvent::Space));
        black_box(engine.handle(InputEvent::Enter));
    });

    let mut all_packs_engine = SlimeEngine::bundled();
    black_box(all_packs_engine.set_preferences(EnginePreferences {
        live_conversion: false,
        history_completion: false,
        history_learning: false,
        dictionary_packs: ALL_DOMAIN_DICTIONARIES,
    }));
    run("engine/nihon_conversion_all_packs", iterations, || {
        let mut engine = all_packs_engine.clone();
        for character in black_box("nihon").chars() {
            black_box(engine.handle(InputEvent::Character(character)));
        }
        black_box(engine.handle(InputEvent::Space));
        black_box(engine.handle(InputEvent::Enter));
    });

    run_history_benchmarks((iterations / 10).clamp(1_000, 5_000));
    run_session_context_benchmarks((iterations / 10).clamp(1_000, 5_000));

    let live_iterations = (iterations / 100).clamp(100, 500);
    let source = "seidowotakamerukufuuwoshiteikimashou".repeat(3);
    for length in live_lengths() {
        let input = &source[..length];
        run(
            &format!("engine/live_conversion_{length}"),
            live_iterations,
            || {
                let mut engine = SlimeEngine::bundled();
                black_box(engine.set_preferences(EnginePreferences {
                    live_conversion: true,
                    history_completion: false,
                    history_learning: false,
                    dictionary_packs: 0,
                }));
                for character in black_box(input).chars() {
                    black_box(engine.handle(InputEvent::Character(character)));
                }
                black_box(engine.handle(InputEvent::Enter));
            },
        );
    }
}

fn run_session_context_benchmarks(iterations: u64) {
    let entries = domain_entries();
    assert!(entries.len() >= 128);
    let (previous_reading, previous_surface) = entries[0];
    let (target_reading, target_surface) = entries[1];
    let preferences = EnginePreferences {
        live_conversion: false,
        history_completion: true,
        history_learning: true,
        dictionary_packs: ALL_DOMAIN_DICTIONARIES,
    };

    let mut empty_context = SlimeEngine::bundled();
    black_box(empty_context.set_preferences(preferences));
    run("engine/session_context_empty", iterations, || {
        query_and_clear(&mut empty_context, target_reading);
    });

    let mut full_context = SlimeEngine::bundled();
    black_box(full_context.set_preferences(preferences));
    commit_reading(&mut full_context, previous_reading, previous_surface);
    commit_reading(&mut full_context, target_reading, target_surface);
    for &(reading, surface) in &entries[2..128] {
        commit_reading(&mut full_context, reading, surface);
    }
    commit_reading(&mut full_context, previous_reading, previous_surface);

    run("engine/session_context_128", iterations, || {
        query_and_clear(&mut full_context, target_reading);
    });
}

fn domain_entries() -> Vec<(&'static str, &'static str)> {
    [
        include_str!("../data/technology.tsv"),
        include_str!("../data/business.tsv"),
        include_str!("../data/creative.tsv"),
    ]
    .into_iter()
    .flat_map(str::lines)
    .filter(|line| !line.is_empty() && !line.starts_with('#'))
    .map(|line| {
        let mut columns = line.split('\t');
        (
            columns.next().expect("domain reading"),
            columns.next().expect("domain surface"),
        )
    })
    .collect()
}

fn commit_reading(engine: &mut SlimeEngine, reading: &str, surface: &str) {
    for character in reading.chars() {
        black_box(engine.handle(InputEvent::Character(character)));
    }
    black_box(engine.handle(InputEvent::Space));
    let index = engine
        .snapshot()
        .candidates
        .iter()
        .position(|candidate| candidate == surface)
        .unwrap_or_else(|| panic!("missing {surface} for {reading}"));
    black_box(engine.handle(InputEvent::SelectCandidate(
        u32::try_from(index).expect("candidate index"),
    )));
    black_box(engine.handle(InputEvent::Enter));
}

fn query_and_clear(engine: &mut SlimeEngine, reading: &str) {
    for character in reading.chars() {
        black_box(engine.handle(InputEvent::Character(character)));
    }
    black_box(engine.handle(InputEvent::Space));
    black_box(engine.handle(InputEvent::Escape));
    black_box(engine.handle(InputEvent::Escape));
}

fn run_history_benchmarks(iterations: u64) {
    let directory =
        std::env::temp_dir().join(format!("slime-history-benchmark-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("create history benchmark directory");
    let mut fixture = String::from("# slime-history-v1\n");
    for index in 0..499 {
        writeln!(
            fixture,
            "れきし{index}\t履歴{index}\t{}\t{index}",
            index % 10 + 1
        )
        .expect("write history benchmark row");
    }
    fixture.push_str("ぱふぉーまんす\tパフォーマンス\t8\t1000\n");
    fs::write(directory.join("history.tsv"), fixture).expect("write history benchmark fixture");

    for history_completion in [false, true] {
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        black_box(engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion,
            history_learning: false,
            dictionary_packs: 0,
        }));
        let state = if history_completion { "on" } else { "off" };
        run(
            &format!("engine/history_completion_{state}_500_entries"),
            iterations,
            || {
                for character in black_box("pafu").chars() {
                    black_box(engine.handle(InputEvent::Character(character)));
                }
                black_box(engine.handle(InputEvent::Enter));
            },
        );
    }

    fs::remove_dir_all(directory).expect("remove history benchmark directory");
}

fn live_lengths() -> Vec<usize> {
    std::env::var("SLIME_BENCH_LIVE_LENGTHS").map_or_else(
        |_| vec![10, 50, 100],
        |value| {
            value
                .split(',')
                .filter_map(|length| length.parse().ok())
                .collect()
        },
    )
}

fn iterations(default: u64) -> u64 {
    std::env::var("SLIME_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn run(name: &str, iterations: u64, mut operation: impl FnMut()) {
    for _ in 0..1_000 {
        operation();
    }

    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let elapsed = started.elapsed();
    let nanos = elapsed.as_nanos() / u128::from(iterations);
    println!("{name}\t{nanos}\tns/op\t{iterations}\titerations");
}
