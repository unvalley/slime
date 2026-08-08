use std::hint::black_box;
use std::time::Instant;

use slime_converter::{Dictionary, DictionaryEntry, DictionaryLayer};

const LONG_CANDIDATE_READING: &str = "きょうはあさからあめがふっていたのでえきまであるいていきひるすぎにしごとをおえていえにもどった";

fn main() {
    let dictionary = Dictionary::bundled();
    let iterations = iterations(100_000);

    run("converter/candidate_window_single_word", iterations, || {
        black_box(dictionary.candidates(black_box("にほん")));
    });
    run_document_context_benchmark(&dictionary, iterations);
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
    run(
        "converter/candidate_window_long_sentence",
        iterations,
        || {
            black_box(dictionary.candidates(black_box(LONG_CANDIDATE_READING)));
        },
    );
    run_fixed_segment_benchmark(&dictionary, iterations);
    run("converter/short_candidates_initial", iterations, || {
        black_box(dictionary.candidates(black_box("あさいり")));
    });
    run("converter/short_candidates_expanded", iterations, || {
        black_box(dictionary.candidates_with_limit(black_box("あさいり"), black_box(32)));
    });
    run("converter/short_compound_recall", iterations, || {
        black_box(dictionary.compound_candidates(black_box("あさいり"), 8, 32));
    });
    let three_part_dictionary = three_part_dictionary();
    run("converter/three_part_compound_recall", iterations, || {
        black_box(three_part_dictionary.compound_candidates(
            black_box("あいうえおかきくけ"),
            8,
            32,
        ));
    });
    let one_character_segment_dictionary = one_character_segment_dictionary();
    run(
        "converter/one_character_segment_compound_recall",
        iterations,
        || {
            black_box(one_character_segment_dictionary.compound_candidates(
                black_box("あいうえお"),
                8,
                32,
            ));
        },
    );
    let kana_only_segment_dictionary = kana_only_segment_dictionary();
    run(
        "converter/kana_only_segment_compound_recall",
        iterations,
        || {
            black_box(kana_only_segment_dictionary.compound_candidates(
                black_box("やまだのけんきゅうしつ"),
                8,
                32,
            ));
        },
    );
    let four_part_dictionary = four_part_dictionary();
    run("converter/four_part_compound_recall", iterations, || {
        black_box(four_part_dictionary.compound_candidates(
            black_box("あいうえおかきくけこさし"),
            8,
            32,
        ));
    });
    let five_part_dictionary = five_part_dictionary();
    run("converter/five_part_compound_recall", iterations, || {
        black_box(five_part_dictionary.compound_candidates(
            black_box("あいうえおかきくけこさしすせそ"),
            8,
            32,
        ));
    });
    let six_part_dictionary = six_part_dictionary();
    run("converter/six_part_compound_recall", iterations, || {
        black_box(six_part_dictionary.compound_candidates(
            black_box("あいうえおかきくけこさし"),
            8,
            32,
        ));
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

fn run_document_context_benchmark(dictionary: &Dictionary, iterations: u64) {
    run("converter/document_context_candidates", iterations, || {
        black_box(dictionary.candidates_with_context(
            black_box("あさの"),
            black_box("同社の不燃木材は浅野木材工業の"),
        ));
    });
}

fn run_fixed_segment_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/fixed_segment_variants_long_sentence",
        iterations,
        || {
            black_box(dictionary.fixed_segment_variants(black_box(LONG_CANDIDATE_READING), 8, 22));
        },
    );
}

fn three_part_dictionary() -> Dictionary {
    let mut entries = Vec::new();
    for (reading, prefix) in [("あいう", "左"), ("えおか", "中"), ("きくけ", "右")] {
        for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
            entries.push(DictionaryEntry::new(
                reading,
                format!("{prefix}{index}"),
                cost,
            ));
        }
    }
    Dictionary::new(entries)
}

fn four_part_dictionary() -> Dictionary {
    let mut entries = Vec::new();
    for (reading, prefix) in [
        ("あいう", "一"),
        ("えおか", "二"),
        ("きくけ", "三"),
        ("こさし", "四"),
    ] {
        for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
            entries.push(DictionaryEntry::new(
                reading,
                format!("{prefix}{index}"),
                cost,
            ));
        }
    }
    Dictionary::new(entries)
}

fn five_part_dictionary() -> Dictionary {
    let mut entries = Vec::new();
    for (reading, prefix) in [
        ("あいう", "一"),
        ("えおか", "二"),
        ("きくけ", "三"),
        ("こさし", "四"),
        ("すせそ", "五"),
    ] {
        for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
            entries.push(DictionaryEntry::new(
                reading,
                format!("{prefix}{index}"),
                cost,
            ));
        }
    }
    Dictionary::new(entries)
}

fn six_part_dictionary() -> Dictionary {
    let mut entries = Vec::new();
    for (reading, prefix) in [
        ("あい", "一"),
        ("うえ", "二"),
        ("おか", "三"),
        ("きく", "四"),
        ("けこ", "五"),
        ("さし", "六"),
    ] {
        for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
            entries.push(DictionaryEntry::new(
                reading,
                format!("{prefix}{index}"),
                cost,
            ));
        }
    }
    Dictionary::new(entries)
}

fn one_character_segment_dictionary() -> Dictionary {
    let mut entries = Vec::new();
    for (reading, prefix) in [("あい", "左"), ("う", "中"), ("えお", "右")] {
        for (index, cost) in [0, 10, 20, 30].into_iter().enumerate() {
            entries.push(DictionaryEntry::new(
                reading,
                format!("{prefix}{index}"),
                cost,
            ));
        }
    }
    Dictionary::new(entries)
}

fn kana_only_segment_dictionary() -> Dictionary {
    Dictionary::new(vec![
        DictionaryEntry::new("やまだ", "山田", 10),
        DictionaryEntry::new("の", "の", 20),
        DictionaryEntry::new("けんきゅう", "研究", 30),
        DictionaryEntry::new("しつ", "室", 40),
    ])
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
    if std::env::var("SLIME_BENCH_FILTER").is_ok_and(|filter| !name.contains(&filter)) {
        return;
    }
    let warmup_iterations = std::env::var("SLIME_BENCH_WARMUP_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000);
    for _ in 0..warmup_iterations {
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
