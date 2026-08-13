use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use slime_core::{
    ALL_DATE_FORMATS, ALL_DOMAIN_DICTIONARIES, DictionaryPackTrust, DictionaryPackVerificationKey,
    EnginePreferences, InputEvent, SlimeEngine, UserData,
};

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

    run("engine/typo_correction_neighbor", iterations, || {
        let mut engine = SlimeEngine::bundled();
        for character in black_box("nihpn").chars() {
            black_box(engine.handle(InputEvent::Character(character)));
        }
        black_box(engine.handle(InputEvent::Space));
    });
    run("engine/typo_correction_missing_vowel", iterations, || {
        let mut engine = SlimeEngine::bundled();
        for character in black_box("nihn").chars() {
            black_box(engine.handle(InputEvent::Character(character)));
        }
        black_box(engine.handle(InputEvent::Space));
    });
    run(
        "engine/typo_correction_missing_geminate",
        iterations,
        || {
            let mut engine = SlimeEngine::bundled();
            for character in black_box("keka").chars() {
                black_box(engine.handle(InputEvent::Character(character)));
            }
            black_box(engine.handle(InputEvent::Space));
        },
    );
    run(
        "engine/typo_correction_missing_consonant",
        iterations,
        || {
            let mut engine = SlimeEngine::bundled();
            for character in black_box("paokon").chars() {
                black_box(engine.handle(InputEvent::Character(character)));
            }
            black_box(engine.handle(InputEvent::Space));
        },
    );

    let mut all_packs_engine = SlimeEngine::bundled();
    black_box(all_packs_engine.set_preferences(EnginePreferences {
        live_conversion: false,
        history_completion: false,
        history_learning: false,
        dictionary_packs: ALL_DOMAIN_DICTIONARIES,
        private_mode: false,
        date_format_mask: ALL_DATE_FORMATS,
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
    run_adaptive_context_benchmarks((iterations / 10).clamp(1_000, 5_000));
    run_confirmed_context_commit_benchmark((iterations / 10).clamp(1_000, 5_000));
    run_static_context_pack_benchmarks((iterations / 10).clamp(1_000, 5_000));

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
                    private_mode: false,
                    date_format_mask: ALL_DATE_FORMATS,
                }));
                for character in black_box(input).chars() {
                    black_box(engine.handle(InputEvent::Character(character)));
                }
                black_box(engine.handle(InputEvent::Enter));
            },
        );
    }
}

fn run_confirmed_context_commit_benchmark(iterations: u64) {
    let mut engine = SlimeEngine::bundled();
    black_box(engine.set_preferences(EnginePreferences {
        live_conversion: false,
        history_completion: true,
        history_learning: true,
        dictionary_packs: 0,
        private_mode: false,
        date_format_mask: ALL_DATE_FORMATS,
    }));

    run("engine/confirmed_phrase_commit", iterations, || {
        engine.reset_context();
        commit_reading(&mut engine, "heyashoumei", "部屋照明");
    });
}

fn run_static_context_pack_benchmarks(iterations: u64) {
    let directory = std::env::temp_dir().join(format!(
        "slime-static-context-benchmark-{}",
        std::process::id()
    ));
    let pack_path = write_static_context_benchmark_pack(&directory);

    let baseline = SlimeEngine::bundled();
    let engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
    assert_eq!(
        engine
            .conversion_candidates_with_left_context("これは長い文章", "かんじ")
            .first()
            .map(String::as_str),
        Some("漢字"),
        "static context benchmark rule must affect ranking"
    );
    let matching_context = format!("{}文章", "長".repeat(126));
    let missing_context = "長".repeat(128);

    run("engine/static_context_baseline_no_pack", iterations, || {
        black_box(baseline.conversion_candidates_with_left_context(
            black_box(&matching_context),
            black_box("かんじ"),
        ));
    });
    run("engine/static_context_miss_10001_rules", iterations, || {
        black_box(engine.conversion_candidates_with_left_context(
            black_box(&missing_context),
            black_box("かんじ"),
        ));
    });
    run(
        "engine/static_context_exact_10001_rules",
        iterations,
        || {
            black_box(
                engine.conversion_candidates_with_left_context(
                    black_box("文章"),
                    black_box("かんじ"),
                ),
            );
        },
    );
    run(
        "engine/static_context_suffix_10001_rules",
        iterations,
        || {
            black_box(engine.conversion_candidates_with_left_context(
                black_box(&matching_context),
                black_box("かんじ"),
            ));
        },
    );

    let trust = sign_static_context_benchmark_pack(&pack_path);
    let load_iterations = (iterations / 10).clamp(100, 500);
    run(
        "engine/static_context_pack_load_unsigned_10001_rules",
        load_iterations,
        || {
            black_box(SlimeEngine::bundled_with_user_data(UserData::load(
                black_box(&directory),
            )));
        },
    );
    run(
        "engine/static_context_pack_load_signed_10001_rules",
        load_iterations,
        || {
            black_box(SlimeEngine::bundled_with_user_data_and_pack_trust(
                UserData::load(black_box(&directory)),
                black_box(trust.clone()),
            ));
        },
    );

    fs::remove_dir_all(directory).expect("remove static context benchmark directory");
}

fn write_static_context_benchmark_pack(directory: &Path) -> PathBuf {
    const RULE_COUNT: usize = 10_000;

    let pack_directory = directory.join("dictionary-packs");
    fs::create_dir_all(&pack_directory).expect("create static context benchmark directory");
    let mut payload = String::from("てすとようご\t試験用語\n# context-rules\n");
    for index in 0..RULE_COUNT {
        writeln!(payload, "前提{index}\tかんじ\t漢字\t100")
            .expect("write static context benchmark rule");
    }
    payload.push_str("文章\tかんじ\t漢字\t0\n");
    let digest = lower_hex(&Sha256::digest(payload.as_bytes()));
    let source = format!(
        "# slime-dictionary-pack-v3\n\
         # id: static-context-benchmark\n\
         # name: 文脈性能試験\n\
         # version: 2026.08.1\n\
         # license: Example-Test-Only\n\
         # minimum-slime-version: 0.1.0\n\
         # published-at: 2026-08-08\n\
         # provenance: fixture/generated/static-context-benchmark\n\
         # payload-sha256: {digest}\n\
         # entries\n\
         {payload}"
    );
    let pack_path = pack_directory.join("static-context-benchmark.slime-dict");
    fs::write(&pack_path, source).expect("write static context benchmark pack");
    pack_path
}

fn sign_static_context_benchmark_pack(pack_path: &Path) -> DictionaryPackTrust {
    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    let pack_bytes = fs::read(pack_path).expect("read static context benchmark pack");
    let signature = signing_key.sign(&pack_bytes).to_bytes();
    fs::write(
        pack_path.with_extension("slime-dict.sig"),
        format!(
            "# slime-dictionary-signature-v1\n\
             # key-id: fixture-benchmark\n\
             # signature-ed25519: {}\n",
            lower_hex(&signature)
        ),
    )
    .expect("write static context benchmark signature");
    DictionaryPackTrust::signed_only(vec![
        DictionaryPackVerificationKey::new(
            "fixture-benchmark",
            signing_key.verifying_key().to_bytes(),
        )
        .expect("valid static context benchmark key"),
    ])
    .expect("valid static context benchmark trust")
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn run_adaptive_context_benchmarks(iterations: u64) {
    let entries = domain_entries();
    assert!(entries.len() >= 128);
    let (previous_reading, previous_surface) = entries[0];
    let (target_reading, target_surface) = entries[1];
    let preferences = EnginePreferences {
        live_conversion: false,
        history_completion: true,
        history_learning: true,
        dictionary_packs: ALL_DOMAIN_DICTIONARIES,
        private_mode: false,
        date_format_mask: ALL_DATE_FORMATS,
    };

    let mut empty_context = SlimeEngine::bundled();
    black_box(empty_context.set_preferences(preferences));
    run("engine/adaptive_context_empty", iterations, || {
        query_and_clear(&mut empty_context, target_reading);
    });

    let mut full_context = SlimeEngine::bundled();
    black_box(full_context.set_preferences(preferences));
    commit_reading(&mut full_context, previous_reading, previous_surface);
    commit_reading(&mut full_context, target_reading, target_surface);
    commit_reading(&mut full_context, previous_reading, previous_surface);
    commit_reading(&mut full_context, target_reading, target_surface);
    for &(reading, surface) in &entries[2..128] {
        commit_reading(&mut full_context, reading, surface);
    }
    commit_reading(&mut full_context, previous_reading, previous_surface);

    run("engine/adaptive_context_128", iterations, || {
        query_and_clear(&mut full_context, target_reading);
    });

    let directory = std::env::temp_dir().join(format!(
        "slime-context-history-benchmark-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create context history benchmark directory");
    let mut fixture = String::from("# slime-context-history-v1\n");
    for index in 0..499 {
        writeln!(
            fixture,
            "ぶんみゃく{index}\t文脈{index}\tこうほ{index}\t候補{index}\t2\t{index}"
        )
        .expect("write context history benchmark row");
    }
    writeln!(
        fixture,
        "{previous_reading}\t{previous_surface}\t{target_reading}\t{target_surface}\t2\t1000"
    )
    .expect("write matching context history benchmark row");
    fs::write(directory.join("context_history.tsv"), fixture)
        .expect("write context history benchmark fixture");

    let mut persistent_context = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
    black_box(persistent_context.set_preferences(preferences));
    commit_reading(&mut persistent_context, previous_reading, previous_surface);
    assert_eq!(
        persistent_context
            .conversion_candidates(target_reading)
            .first()
            .map(String::as_str),
        Some(target_surface),
        "persistent context fixture must affect ranking"
    );
    run("engine/persistent_context_500", iterations, || {
        query_and_clear(&mut persistent_context, target_reading);
    });

    let mut external_context = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
    black_box(external_context.set_preferences(preferences));
    external_context.set_external_left_context(&format!("既存文書{previous_surface}"));
    assert_eq!(
        external_context
            .conversion_candidates(target_reading)
            .first()
            .map(String::as_str),
        Some(target_surface),
        "external context fixture must affect ranking"
    );
    run("engine/external_context_500", iterations, || {
        query_and_clear(&mut external_context, target_reading);
    });

    fs::remove_dir_all(directory).expect("remove context history benchmark directory");
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
    for index in 0..498 {
        writeln!(
            fixture,
            "れきし{index}\t履歴{index}\t{}\t{index}",
            index % 10 + 1
        )
        .expect("write history benchmark row");
    }
    fixture.push_str("ぱふぉーまんす\tパフォーマンス\t8\t1000\n");
    fixture.push_str("わたし\tワタシ\t1\t1001\n");
    fs::write(directory.join("history.tsv"), fixture).expect("write history benchmark fixture");

    for history_completion in [false, true] {
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        black_box(engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
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

    for history_completion in [false, true] {
        let mut engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
        black_box(engine.set_preferences(EnginePreferences {
            live_conversion: false,
            history_completion,
            history_learning: false,
            dictionary_packs: 0,
            private_mode: false,
            date_format_mask: ALL_DATE_FORMATS,
        }));
        engine.set_external_context("彼らは更に自らの救命胴衣を他の兵士に", "た。");
        assert_eq!(
            engine
                .conversion_candidates("わたし")
                .first()
                .map(String::as_str),
            Some("渡し")
        );
        let state = if history_completion { "on" } else { "off" };
        run(
            &format!("engine/contextual_conversion_history_{state}_500_entries"),
            iterations,
            || query_and_clear(&mut engine, "watashi"),
        );
    }

    let mut preferences = String::from("# slime-history-preferences-v1\n");
    for index in 0..498 {
        writeln!(preferences, "れきし{index}\t履歴{index}\t{index}")
            .expect("write history preference benchmark row");
    }
    preferences.push_str("ほげ0\t補助0\t1000\nほげ1\t補助1\t1001\n");
    fs::write(directory.join("history_preferences.tsv"), preferences)
        .expect("write history preferences benchmark fixture");
    let mut preferences_engine = SlimeEngine::bundled_with_user_data(UserData::load(&directory));
    black_box(preferences_engine.set_preferences(EnginePreferences {
        live_conversion: false,
        history_completion: true,
        history_learning: false,
        dictionary_packs: 0,
        private_mode: false,
        date_format_mask: ALL_DATE_FORMATS,
    }));
    preferences_engine.set_external_context("彼らは更に自らの救命胴衣を他の兵士に", "た。");
    assert_eq!(
        preferences_engine
            .conversion_candidates("わたし")
            .first()
            .map(String::as_str),
        Some("渡し")
    );
    run(
        "engine/contextual_conversion_history_on_500_entries_500_preferences",
        iterations,
        || query_and_clear(&mut preferences_engine, "watashi"),
    );

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
