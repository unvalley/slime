use std::hint::black_box;
use std::time::Instant;

use slime_romaji::RomajiComposer;

fn main() {
    let iterations = iterations(200_000);
    run("romaji/nihongo", iterations, || {
        let mut composer = RomajiComposer::new();
        let mut output = String::new();
        for character in black_box("nihongo").chars() {
            output.push_str(&composer.push(character).unwrap());
        }
        output.push_str(&composer.flush());
        black_box(output);
    });
}

fn iterations(default: u64) -> u64 {
    std::env::var("SLIME_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn run(mut name: &str, iterations: u64, mut operation: impl FnMut()) {
    for _ in 0..1_000 {
        operation();
    }

    name = black_box(name);
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let elapsed = started.elapsed();
    let nanos = elapsed.as_nanos() / u128::from(iterations);
    println!("{name}\t{nanos}\tns/op\t{iterations}\titerations");
}
