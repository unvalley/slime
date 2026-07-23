use std::hint::black_box;
use std::time::Instant;

use slime_converter::Dictionary;

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
    run("converter/n_best_phrase", iterations, || {
        black_box(dictionary.candidates(black_box("わたしはにほん")));
    });
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
