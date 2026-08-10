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
    run_ordinal_generation_benchmark(&dictionary, iterations);
    run_percentage_change_benchmark(&dictionary, iterations);
    run_superlative_prefix_context_benchmark(&dictionary, iterations);
    run_noun_prefix_right_phrase_benchmark(&dictionary, iterations);
    run("converter/segmented_phrase", iterations, || {
        black_box(dictionary.convert_best(black_box("わたしはにほん")));
    });
    run_n_best_benchmarks(&dictionary, iterations);
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

fn run_n_best_benchmarks(dictionary: &Dictionary, iterations: u64) {
    run("converter/n_best_search", iterations, || {
        black_box(dictionary.convert_n_best(black_box("わたしはにほん"), black_box(10)));
    });
    run("converter/n_best_search_20", iterations, || {
        black_box(dictionary.convert_n_best(black_box("わたしはにほん"), black_box(20)));
    });
    run("converter/n_best_search_32", iterations, || {
        black_box(dictionary.convert_n_best(black_box("わたしはにほん"), black_box(32)));
    });
    run("converter/n_best_long_10", iterations, || {
        black_box(dictionary.convert_n_best(black_box(LONG_CANDIDATE_READING), black_box(10)));
    });
    run("converter/n_best_long_32", iterations, || {
        black_box(dictionary.convert_n_best(black_box(LONG_CANDIDATE_READING), black_box(32)));
    });
}

fn run_document_context_benchmark(dictionary: &Dictionary, iterations: u64) {
    run("converter/document_context_candidates", iterations, || {
        black_box(dictionary.candidates_with_context(
            black_box("あさの"),
            black_box("同社の不燃木材は浅野木材工業の"),
        ));
    });
    run("converter/boundary_context_candidates", iterations, || {
        black_box(dictionary.candidates_with_context(black_box("いせき"), black_box("オランダへ")));
    });
    run(
        "converter/katakana_compound_context_candidates",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("たい"), black_box("ナスタアリーク")),
            );
        },
    );
    run("converter/numeric_context_candidates", iterations, || {
        black_box(dictionary.candidates_with_context(black_box("だん"), black_box("3")));
    });
    run_general_noun_context_benchmark(dictionary, iterations);
    run_ideographic_suffix_context_benchmark(dictionary, iterations);
    run_genitive_context_benchmark(dictionary, iterations);
    run_numeric_right_compound_benchmark(dictionary, iterations);
    run_numeric_score_notation_benchmark(dictionary, iterations);
    run(
        "converter/polite_right_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("のめ"),
                black_box("うまいコーヒーが"),
                black_box("ました。"),
            ));
        },
    );
    run("converter/polite_left_only_candidates", iterations, || {
        black_box(
            dictionary.candidates_with_context(black_box("のめ"), black_box("うまいコーヒーが")),
        );
    });
    run(
        "converter/desiderative_right_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("かい"),
                black_box("丁寧に案内してもらい、"),
                black_box("たい物が買えました。"),
            ));
        },
    );
    run(
        "converter/desiderative_left_only_candidates",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(
                    black_box("かい"),
                    black_box("丁寧に案内してもらい、"),
                ),
            );
        },
    );
    run_right_grammar_benchmarks(dictionary, iterations);
    run("converter/right_compound_candidates", iterations, || {
        black_box(dictionary.candidates_with_surrounding_context(
            black_box("まち"),
            black_box("患者と患者の"),
            black_box("時間は少ない"),
        ));
    });
    run("converter/right_compound_left_only", iterations, || {
        black_box(dictionary.candidates_with_context(black_box("まち"), black_box("患者と患者の")));
    });
    run_right_coordination_phrase_benchmark(dictionary, iterations);
    run_right_genitive_phrase_benchmark(dictionary, iterations);
    run_measured_reach_benchmark(dictionary, iterations);
    run(
        "converter/right_inflectional_phrase_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("き"),
                black_box("カラフルで色合いがいいデザインがあったので"),
                black_box("に入りました"),
            ));
        },
    );
    run(
        "converter/right_inflectional_phrase_left_only",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(
                black_box("き"),
                black_box("カラフルで色合いがいいデザインがあったので"),
            ));
        },
    );
}

fn run_ordinal_generation_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/ordinal_dai_generation_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(black_box("だい"), black_box("第33")));
        },
    );
    run(
        "converter/ordinal_dai_plain_device_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(black_box("だい"), black_box("33")));
        },
    );
}

fn run_percentage_change_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/percentage_change_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(black_box("げん"), black_box("0.5%")));
        },
    );
    run("converter/percentage_change_nonmatch", iterations, || {
        black_box(dictionary.candidates_with_context(black_box("げん"), black_box("-3%")));
    });
}

fn run_ideographic_suffix_context_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/ideographic_suffix_context_candidates",
        iterations,
        || {
            black_box(
                dictionary
                    .candidates_with_context(black_box("けん"), black_box("大勢の信者が傍聴")),
            );
        },
    );
    run(
        "converter/ideographic_suffix_context_without_match",
        iterations,
        || {
            black_box(
                dictionary
                    .candidates_with_context(black_box("けん"), black_box("大勢の信者が集合")),
            );
        },
    );
}

fn run_superlative_prefix_context_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/superlative_prefix_context_candidates",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("かい"), black_box("もうひとつは最")),
            );
        },
    );
    run(
        "converter/ordinary_single_character_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(black_box("き"), black_box("大正")));
        },
    );
}

fn run_noun_prefix_right_phrase_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/noun_prefix_right_phrase_conflict",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("み"),
                black_box("犯人は検挙されておらず、2012年8月現在"),
                black_box("解決。"),
            ));
        },
    );
    run(
        "converter/left_phrase_with_unmatched_right_context",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("かい"),
                black_box("もうひとつは最"),
                black_box("でした"),
            ));
        },
    );
}

fn run_genitive_context_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/verbal_noun_genitive_context_candidates",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("しかい"), black_box("多数の番組の")),
            );
        },
    );
    run(
        "converter/verbal_noun_genitive_context_without_match",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("しかい"), black_box("多数の番組")),
            );
        },
    );
}

fn run_general_noun_context_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/general_noun_context_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(black_box("そ"), black_box("線形作用")));
        },
    );
    run(
        "converter/general_noun_context_without_match",
        iterations,
        || {
            black_box(dictionary.candidates_with_context(black_box("そ"), black_box("線形")));
        },
    );
    run(
        "converter/lower_frequency_general_noun_left_candidates",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("おき"), black_box("太平洋の三陸")),
            );
        },
    );
    run(
        "converter/lower_frequency_general_noun_right_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("くん"),
                black_box("叙正三位"),
                black_box("一等授瑞宝章。"),
            ));
        },
    );
}

fn run_numeric_right_compound_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/numeric_right_compound_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("かんせん"),
                black_box("デドフスクにはM9"),
                black_box("道路が通る"),
            ));
        },
    );
    run(
        "converter/numeric_right_compound_left_only",
        iterations,
        || {
            black_box(
                dictionary
                    .candidates_with_context(black_box("かんせん"), black_box("デドフスクにはM9")),
            );
        },
    );
}

fn run_numeric_score_notation_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/numeric_score_notation_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("たい"),
                black_box("相手GKと1"),
                black_box("1になった"),
            ));
        },
    );
    run(
        "converter/numeric_score_notation_left_only",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("たい"), black_box("相手GKと1")),
            );
        },
    );
}

fn run_right_coordination_phrase_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/right_coordination_phrase_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("かた"),
                black_box("デスクワークで固まった"),
                black_box("や背中をほぐす"),
            ));
        },
    );
    run(
        "converter/right_coordination_phrase_left_only",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(
                    black_box("かた"),
                    black_box("デスクワークで固まった"),
                ),
            );
        },
    );
}

fn run_right_genitive_phrase_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/right_genitive_phrase_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("みち"),
                black_box("人間の心、"),
                black_box("の世界を探究する"),
            ));
        },
    );
    run(
        "converter/right_genitive_phrase_left_only",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(black_box("みち"), black_box("人間の心、")),
            );
        },
    );
}

fn run_measured_reach_benchmark(dictionary: &Dictionary, iterations: u64) {
    run("converter/measured_reach_candidates", iterations, || {
        black_box(dictionary.candidates_with_surrounding_context(
            black_box("けん"),
            black_box("大阪駅から徒歩10分"),
            black_box("内のホテル"),
        ));
    });
    run("converter/measured_reach_left_only", iterations, || {
        black_box(
            dictionary.candidates_with_context(black_box("けん"), black_box("大阪駅から徒歩10分")),
        );
    });
}

fn run_unique_right_grammar_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/unique_right_grammar_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("こ"),
                black_box("有名な先生方が講師として"),
                black_box("られています。"),
            ));
        },
    );
    run(
        "converter/unique_right_grammar_left_only",
        iterations,
        || {
            black_box(
                dictionary.candidates_with_context(
                    black_box("こ"),
                    black_box("有名な先生方が講師として"),
                ),
            );
        },
    );
}

fn run_right_grammar_benchmarks(dictionary: &Dictionary, iterations: u64) {
    run_unique_right_grammar_benchmark(dictionary, iterations);
    run_right_function_word_benchmark(dictionary, iterations);
    run_unique_right_suru_benchmark(dictionary, iterations);
}

fn run_right_function_word_benchmark(dictionary: &Dictionary, iterations: u64) {
    run(
        "converter/right_function_word_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("とく"),
                black_box("メルダーザへかけられた呪は、マリシーユにも"),
                black_box("ことができなかった。"),
            ));
        },
    );
    run("converter/right_function_word_nonmatch", iterations, || {
        black_box(dictionary.candidates_with_surrounding_context(
            black_box("とく"),
            black_box("メルダーザへかけられた呪は、マリシーユにも"),
            black_box("しかありません。"),
        ));
    });
}

fn run_unique_right_suru_benchmark(dictionary: &Dictionary, iterations: u64) {
    run("converter/unique_right_suru_candidates", iterations, || {
        black_box(dictionary.candidates_with_surrounding_context(
            black_box("かんしん"),
            black_box("自分でも趣味で料理をするもので一層"),
            black_box("することが多いのです。"),
        ));
    });
    run("converter/unique_right_suru_left_only", iterations, || {
        black_box(dictionary.candidates_with_context(
            black_box("かんしん"),
            black_box("自分でも趣味で料理をするもので一層"),
        ));
    });
    run(
        "converter/unique_right_suru_past_candidates",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("たいい"),
                black_box("これにより、ギャネンドラ国王は"),
                black_box("した。"),
            ));
        },
    );
    run(
        "converter/unique_right_suru_nonmatch_shika",
        iterations,
        || {
            black_box(dictionary.candidates_with_surrounding_context(
                black_box("とまる"),
                black_box("学校のまわりにはホテルも無いので、成田か香取で"),
                black_box("しかありません。"),
            ));
        },
    );
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
