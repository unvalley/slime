use std::hint::black_box;
use std::time::Instant;

use slime_converter::{Dictionary, DictionaryEntry, DictionaryLayer};

fn main() {
    let dictionary = Dictionary::bundled();
    let iterations = iterations(100_000);

    run("converter/candidate_window_single_word", iterations, || {
        black_box(dictionary.candidates(black_box("にほん")));
    });
    run("converter/segmented_phrase", iterations, || {
        black_box(dictionary.convert_best(black_box("わたしはにほん")));
    });
    run("converter/n_best_search", iterations, || {
        black_box(dictionary.convert_n_best(black_box("わたしはにほん"), black_box(10)));
    });
    run("converter/n_best_search_20", iterations, || {
        black_box(dictionary.convert_n_best(black_box("わたしはにほん"), black_box(20)));
    });
    run("converter/n_best_phrase", iterations, || {
        black_box(dictionary.candidates(black_box("わたしはにほん")));
    });
    run("converter/short_candidates_initial", iterations, || {
        black_box(dictionary.candidates(black_box("あさいり")));
    });
    run("converter/short_candidates_expanded", iterations, || {
        black_box(dictionary.candidates_with_limit(black_box("あさいり"), black_box(32)));
    });
    run("converter/digit_counter_phrase", iterations, || {
        black_box(dictionary.candidates(black_box("２０２６ねん８がつ１にち")));
    });
    run("converter/reconversion_lookup", iterations, || {
        black_box(dictionary.readings_for_surface(black_box("日本")));
    });
    run("converter/short_dictionary_layer", iterations, || {
        black_box(short_dictionary_layer());
    });
}

fn short_dictionary_layer() -> DictionaryLayer {
    let entries = (0..256)
        .map(|_| DictionaryEntry::with_pos("かんじ", "漢字", 1_851, 1_851, 500))
        .collect();
    DictionaryLayer::new("user", "ユーザー辞書", entries)
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
