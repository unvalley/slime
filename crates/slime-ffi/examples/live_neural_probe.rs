//! End-to-end probe for delayed, generation-safe LIVE neural ranking.

use std::env;
use std::ffi::c_void;
use std::process::ExitCode;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use slime_ffi::{
    ACTION_UPDATE_PREEDIT, EVENT_CHARACTER, EVENT_ENTER, EVENT_SPACE, STATUS_OK, SlimeActionViewV2,
    SlimeBuffer, SlimeLiveNeuralTask, slime_buffer_destroy, slime_create, slime_destroy,
    slime_enable_neural_reranker_with_cost_gap, slime_live_neural_task_apply_actions_v2,
    slime_live_neural_task_create_v2, slime_live_neural_task_destroy,
    slime_live_neural_task_reading_character_count, slime_live_neural_task_run, slime_process,
    slime_set_external_left_context, slime_set_live_neural_ranking_enabled, slime_set_options,
};

const LEFT_CONTEXT: &str = "久しぶりにうまいコーヒーが";
const INPUT: &str = "nome";
const EXPECTED: &str = "飲め";
const LIVE_MINIMUM_SWITCH_MARGIN: f64 = 0.2;
const LIVE_LONG_READING_MINIMUM_SWITCH_MARGIN: f64 = 0.3;
const LIVE_NUMERIC_BASE_SWITCH_MARGIN: f64 = 0.1;
const LIVE_LONG_READING_LAMBDA: f64 = 0.6;

#[derive(Debug, Default)]
struct Capture {
    preedit: Option<String>,
}

type ExactProbe = (u32, u32, Option<String>, bool);
type NamedExactProbe = (&'static str, &'static str, Option<ExactProbe>);

fn main() -> ExitCode {
    let Some(model_path) = env::args().nth(1) else {
        eprintln!("usage: live_neural_probe MODEL.gguf");
        return ExitCode::FAILURE;
    };
    let model_path = model_path.into_bytes();
    let handle = match configured_engine(&model_path, LEFT_CONTEXT) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    type_text(handle, INPUT);

    let create_started = Instant::now();
    // SAFETY: Snapshot creation reads the live handle synchronously.
    let task = unsafe {
        slime_live_neural_task_create_v2(
            handle,
            LIVE_MINIMUM_SWITCH_MARGIN,
            LIVE_LONG_READING_MINIMUM_SWITCH_MARGIN,
            LIVE_NUMERIC_BASE_SWITCH_MARGIN,
            LIVE_LONG_READING_LAMBDA,
        )
    };
    let create_elapsed = create_started.elapsed();
    if task.is_null() {
        eprintln!("LIVE snapshot was unexpectedly unavailable");
        // SAFETY: The handle remains live and uniquely owned.
        unsafe { slime_destroy(handle) };
        return ExitCode::FAILURE;
    }
    // SAFETY: The task is live and only read synchronously here.
    let reading_character_count = unsafe { slime_live_neural_task_reading_character_count(task) };

    let run_started = Instant::now();
    let run_status = run_on_worker(task);
    let run_elapsed = run_started.elapsed();
    let mut capture = Capture::default();
    // SAFETY: The completed task and engine are live and exclusively accessed.
    let apply_status = unsafe {
        slime_live_neural_task_apply_actions_v2(
            handle,
            task,
            (&raw mut capture).cast(),
            Some(capture_action),
        )
    };
    // SAFETY: The task is no longer in use and is destroyed exactly once.
    unsafe { slime_live_neural_task_destroy(task) };

    let extension_response = process_character(handle, 'r');
    let extension_response = format!("{extension_response}{}", process_character(handle, 'u'));

    let (stale_context_status, stale_run_status, stale_apply_status, stale_capture) =
        stale_result_probe(handle);

    let sample_count = env::var("SLIME_LIVE_NEURAL_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(1, 1_000);
    let benchmark = benchmark_tasks(handle, sample_count);
    let concurrent_space = concurrent_space_probe(handle);
    let exact_probes_passed = run_exact_probes(&model_path);
    // SAFETY: The handle is no longer in use and is destroyed exactly once.
    unsafe { slime_destroy(handle) };

    println!(
        "snapshot_create_ms={:.3}",
        create_elapsed.as_secs_f64() * 1_000.0
    );
    println!("snapshot_reading_characters={reading_character_count}");
    println!("worker_run_ms={:.3}", run_elapsed.as_secs_f64() * 1_000.0);
    println!("applied_preedit={:?}", capture.preedit);
    println!(
        "extension_preserved={}",
        extension_response.contains("飲める")
    );
    println!("stale_applied={}", stale_capture.preedit.is_some());
    report_benchmark(benchmark.as_ref(), sample_count);
    report_concurrent_space(concurrent_space);

    let passed = run_status == STATUS_OK
        && reading_character_count == 2
        && apply_status == STATUS_OK
        && capture.preedit.as_deref() == Some(EXPECTED)
        && extension_response.contains("飲める")
        && stale_run_status == STATUS_OK
        && stale_context_status == STATUS_OK
        && stale_apply_status == STATUS_OK
        && stale_capture.preedit.is_none()
        && benchmark.is_some()
        && concurrent_space.is_some_and(|(worker_status, concurrent_apply, _, stale_applied)| {
            worker_status == STATUS_OK && concurrent_apply == STATUS_OK && !stale_applied
        })
        && exact_probes_passed;
    if passed {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "probe failed: run={run_status} apply={apply_status} stale_run={stale_run_status} stale_apply={stale_apply_status}"
        );
        ExitCode::FAILURE
    }
}

fn stale_result_probe(handle: *mut slime_ffi::SlimeHandle) -> (u32, u32, u32, Capture) {
    // A completed result from the previous generation must become a no-op.
    // SAFETY: The engine remains live and exclusively accessed.
    let enter = unsafe { slime_process(handle, EVENT_ENTER, 0) };
    // SAFETY: The returned buffer is destroyed exactly once.
    unsafe { slime_buffer_destroy(enter) };
    // SAFETY: Restore the same external context so the stale-task comparison
    // uses the identical candidate pool as the successful generation.
    let context_status = unsafe {
        slime_set_external_left_context(handle, LEFT_CONTEXT.as_ptr(), LEFT_CONTEXT.len())
    };
    type_text(handle, INPUT);
    // SAFETY: Snapshot creation reads the live handle synchronously.
    let task = unsafe {
        slime_live_neural_task_create_v2(
            handle,
            LIVE_MINIMUM_SWITCH_MARGIN,
            LIVE_LONG_READING_MINIMUM_SWITCH_MARGIN,
            LIVE_NUMERIC_BASE_SWITCH_MARGIN,
            LIVE_LONG_READING_LAMBDA,
        )
    };
    let run_status = run_on_worker(task);
    let _ = process_character(handle, 'w');
    let mut capture = Capture::default();
    // SAFETY: The completed task and engine are live and exclusively accessed.
    let apply_status = unsafe {
        slime_live_neural_task_apply_actions_v2(
            handle,
            task,
            (&raw mut capture).cast(),
            Some(capture_action),
        )
    };
    // SAFETY: The stale task is no longer in use and is destroyed exactly once.
    unsafe { slime_live_neural_task_destroy(task) };
    (context_status, run_status, apply_status, capture)
}

fn report_concurrent_space(concurrent: Option<(u32, u32, f64, bool)>) {
    if let Some((worker_status, apply_status, space_ms, stale_applied)) = concurrent {
        println!(
            "concurrent_space_ms={space_ms:.3} worker_status={worker_status} apply_status={apply_status} stale_applied={stale_applied}"
        );
    }
}

fn report_benchmark(benchmark: Option<&(Vec<f64>, Vec<f64>)>, sample_count: usize) {
    let Some((create_samples, run_samples)) = benchmark else {
        return;
    };
    println!(
        "snapshot_create_p50_ms={:.3} snapshot_create_p95_ms={:.3} n={sample_count}",
        percentile(create_samples, 50),
        percentile(create_samples, 95)
    );
    println!(
        "worker_run_p50_ms={:.3} worker_run_p95_ms={:.3} n={sample_count}",
        percentile(run_samples, 50),
        percentile(run_samples, 95)
    );
}

fn concurrent_space_probe(handle: *mut slime_ffi::SlimeHandle) -> Option<(u32, u32, f64, bool)> {
    // SAFETY: Clear the prior composition on the exclusively owned handle.
    let clear = unsafe { slime_process(handle, EVENT_ENTER, 0) };
    // SAFETY: The returned buffer is destroyed exactly once.
    unsafe { slime_buffer_destroy(clear) };
    // SAFETY: Context bytes and handle are live for this synchronous call.
    if unsafe { slime_set_external_left_context(handle, LEFT_CONTEXT.as_ptr(), LEFT_CONTEXT.len()) }
        != STATUS_OK
    {
        return None;
    }
    type_text(handle, INPUT);
    // SAFETY: Snapshot creation reads the live handle synchronously.
    let task = unsafe {
        slime_live_neural_task_create_v2(
            handle,
            LIVE_MINIMUM_SWITCH_MARGIN,
            LIVE_LONG_READING_MINIMUM_SWITCH_MARGIN,
            LIVE_NUMERIC_BASE_SWITCH_MARGIN,
            LIVE_LONG_READING_LAMBDA,
        )
    };
    if task.is_null() {
        return None;
    }
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let address = task as usize;
    let worker = thread::spawn(move || {
        worker_barrier.wait();
        // SAFETY: The worker exclusively owns the independent task.
        unsafe { slime_live_neural_task_run(address as *mut SlimeLiveNeuralTask) }
    });
    barrier.wait();
    let space_started = Instant::now();
    // SAFETY: The worker owns only the immutable task; this thread exclusively
    // owns the engine while explicit Space performs its own model score.
    let space = unsafe { slime_process(handle, EVENT_SPACE, 0) };
    let space_ms = space_started.elapsed().as_secs_f64() * 1_000.0;
    // SAFETY: The returned buffer is destroyed exactly once.
    unsafe { slime_buffer_destroy(space) };
    let worker_status = worker.join().unwrap_or(u32::MAX);
    let mut capture = Capture::default();
    // SAFETY: The worker has joined; task and engine are exclusively accessed.
    let apply_status = unsafe {
        slime_live_neural_task_apply_actions_v2(
            handle,
            task,
            (&raw mut capture).cast(),
            Some(capture_action),
        )
    };
    // SAFETY: The task is no longer used and is destroyed exactly once.
    unsafe { slime_live_neural_task_destroy(task) };
    Some((
        worker_status,
        apply_status,
        space_ms,
        capture.preedit.is_some(),
    ))
}

fn exact_probe(model_path: &[u8], input: &str, expected: &str) -> Option<ExactProbe> {
    exact_probe_with_context(model_path, input, expected, "")
}

fn baseline_exact_probes(model_path: &[u8]) -> [NamedExactProbe; 13] {
    [
        (
            "precision",
            "変換精度",
            exact_probe(model_path, "henkanseido", "変換精度"),
        ),
        (
            "precision_with_particle",
            "変換精度も",
            exact_probe(model_path, "henkanseidomo", "変換精度も"),
        ),
        (
            "numeric_base_repair",
            "位置固定",
            exact_probe(model_path, "ichikotei", "位置固定"),
        ),
        (
            "numeric_homophone_repair",
            "点対称の位置",
            exact_probe(model_path, "tentaishounoichi", "点対称の位置"),
        ),
        (
            "compound_tail_repair",
            "の復讐",
            exact_probe(model_path, "nofukushuu", "の復讐"),
        ),
        (
            "specific_literal_tail_repair",
            "荼毘に付された遺体",
            exact_probe(model_path, "だびにふされたいたい", "荼毘に付された遺体"),
        ),
        (
            "inflected_death_repair",
            "を強打、死亡し",
            exact_probe_with_context(
                model_path,
                "wokyouda,shiboushi",
                "を強打、死亡し",
                "デッドボールが頭",
            ),
        ),
        (
            "inflected_defeat_repair",
            "はギリシアに敗れ",
            exact_probe(model_path, "hagirishianiyabure", "はギリシアに敗れ"),
        ),
        (
            "inflected_attraction_repair",
            "浄土教に強く惹かれる",
            exact_probe(
                model_path,
                "joudokyounitsuyokuhikareru",
                "浄土教に強く惹かれる",
            ),
        ),
        (
            "sokuon_privilege_repair",
            "では、議員の不逮捕特権",
            exact_probe(
                model_path,
                "では、ぎいんのふたいほとっけん",
                "では、議員の不逮捕特権",
            ),
        ),
        (
            "sokuon_chorus_repair",
            "に混声合唱",
            exact_probe_with_context(model_path, "にこんせいがっしょう", "に混声合唱", "代表作"),
        ),
        (
            "sokuon_school_repair",
            "専門学校",
            exact_probe(model_path, "せんもんがっこう", "専門学校"),
        ),
        (
            "literal_dictionary_base_repair",
            "の受賞及びノミネート歴",
            exact_probe_with_context(
                model_path,
                "のじゅしょうおよびのみねーとれき",
                "の受賞及びノミネート歴",
                "の双方で活躍し、それぞれ数多く",
            ),
        ),
    ]
}

fn run_exact_probes(model_path: &[u8]) -> bool {
    let mut probes = Vec::from(baseline_exact_probes(model_path));
    probes.extend(model_regression_probes(model_path));
    probes.extend(implicit_numeric_candidate_probes(model_path));
    probes.extend(literal_hiragana_probes(model_path));
    probes.extend(contextual_single_kana_probes(model_path));
    probes.extend(dictionary_base_fallback_probes(model_path));
    probes.extend(lambda_fallback_probes(model_path));
    probes.extend(long_context_exact_probes(model_path));
    probes.extend(katakana_checkpoint_probes(model_path));
    probes.extend(lexicalized_hiragana_boundary_probes(model_path));
    probes.extend(reopened_literal_target_probes(model_path));
    for (name, _, probe) in &probes {
        println!("{name}_probe={probe:?}");
    }
    probes
        .iter()
        .all(|probe| exact_probe_passed(probe.2.as_ref(), probe.1))
}

fn model_regression_probes(model_path: &[u8]) -> [NamedExactProbe; 3] {
    [
        (
            "contextual_explanation_reference",
            "施設の解説を参照",
            exact_probe_with_context(
                model_path,
                "shisetsunokaisetsuwosanshou",
                "施設の解説を参照",
                "は各",
            ),
        ),
        (
            "consistent_frontier_course",
            "一貫・フロンティアコースでは",
            exact_probe(
                model_path,
                "いっかん・ふろんてぃあこーすでは",
                "一貫・フロンティアコースでは",
            ),
        ),
        (
            "recombined_segment_cross_product",
            "作品によっては光線銃や熱線銃でも",
            exact_probe(
                model_path,
                "さくひんによってはこうせんじゅうやねっせんじゅうでも",
                "作品によっては光線銃や熱線銃でも",
            ),
        ),
    ]
}

fn implicit_numeric_candidate_probes(model_path: &[u8]) -> [NamedExactProbe; 1] {
    [(
        "implicit_numeric_fixed_segment_repair",
        "に黄金色の均一なヨウ化銀の膜を形成",
        exact_probe(
            model_path,
            "にこがねいろのきんいつなようかぎんのまくをけいせい",
            "に黄金色の均一なヨウ化銀の膜を形成",
        ),
    )]
}

fn contextual_single_kana_probes(model_path: &[u8]) -> [NamedExactProbe; 8] {
    [
        (
            "contextual_single_era",
            "期",
            exact_probe_with_context(model_path, "き", "期", "大正"),
        ),
        (
            "contextual_single_department",
            "科",
            exact_probe_with_context(model_path, "か", "科", "精神"),
        ),
        (
            "contextual_single_aircraft",
            "機",
            exact_probe_with_context(model_path, "き", "機", "ヨーロッパ各国の航空"),
        ),
        (
            "contextual_single_believer",
            "徒",
            exact_probe_with_context(model_path, "と", "徒", "正教"),
        ),
        (
            "contextual_single_topic_particle",
            "は",
            exact_probe_with_context(model_path, "は", "は", "ラーメン"),
        ),
        (
            "contextual_single_location_particle",
            "に",
            exact_probe_with_context(model_path, "に", "に", "東京"),
        ),
        (
            "contextual_single_object_particle",
            "を",
            exact_probe_with_context(model_path, "を", "を", "本"),
        ),
        (
            "contextual_single_subject_particle",
            "が",
            exact_probe_with_context(model_path, "が", "が", "これ"),
        ),
    ]
}

fn dictionary_base_fallback_probes(model_path: &[u8]) -> [NamedExactProbe; 11] {
    [
        (
            "long_dictionary_base_selection_material",
            "遊技台選択時の判断材料として",
            exact_probe(
                model_path,
                "ゆうぎだいせんたくじのはんだんざいりょうとして",
                "遊技台選択時の判断材料として",
            ),
        ),
        (
            "long_dictionary_base_source_reference",
            "公開する情報としては、初版で記載した公式ページの出典以外に知りえません",
            exact_probe(
                model_path,
                "こうかいするじょうほうとしては、しょはんできさいしたこうしきぺーじのしゅってんいがいにしりえません",
                "公開する情報としては、初版で記載した公式ページの出典以外に知りえません",
            ),
        ),
        (
            "inflected_literal_base_repair",
            "書ける",
            exact_probe_with_context(model_path, "かける", "書ける", "是非"),
        ),
        (
            "compressed_inflected_literal_preserved",
            "うかがえる",
            exact_probe_with_context(
                model_path,
                "うかがえる",
                "うかがえる",
                "人使いの上手い人物であることが",
            ),
        ),
        (
            "short_contextual_court_official",
            "官",
            exact_probe_with_context(model_path, "かん", "官", "全ては裁判"),
        ),
        (
            "short_contextual_newspaper_company",
            "社",
            exact_probe_with_context(model_path, "しゃ", "社", "中日新聞"),
        ),
        (
            "lexicalized_particle_looking_surname",
            "浅野",
            exact_probe_with_context(
                model_path,
                "あさの",
                "浅野",
                "同社の不燃木材は浅野木材工業の",
            ),
        ),
        (
            "shifted_boundary_literal_target_repair",
            "白に近い色で脆く、牛の乳",
            exact_probe(
                model_path,
                "しろにちかいいろでもろく、うしのちち",
                "白に近い色で脆く、牛の乳",
            ),
        ),
        (
            "shifted_particle_compound_repair",
            "バスケット、積載性に優れる荷台が装着",
            exact_probe_with_context(
                model_path,
                "ばすけっと、せきさいせいにすぐれるにだいがそうちゃく",
                "バスケット、積載性に優れる荷台が装着",
                "三段変速機、大型で頑丈なスチール",
            ),
        ),
        (
            "one_segment_kanji_length_repair",
            "させ、圧力隔壁下部を損傷するなどし",
            exact_probe(
                model_path,
                "させ、あつりょくかくへきかぶをそんしょうするなどし",
                "させ、圧力隔壁下部を損傷するなどし",
            ),
        ),
        (
            "particle_inside_kanji_word_repair",
            "国公立大学輩出者数は五十歩百歩",
            exact_probe(
                model_path,
                "こっこうりつだいがくはいしゅつしゃすうはごじゅっぽひゃっぽ",
                "国公立大学輩出者数は五十歩百歩",
            ),
        ),
    ]
}

fn lambda_fallback_probes(model_path: &[u8]) -> [NamedExactProbe; 2] {
    [
        (
            "long_lambda_fallback_host_duty",
            "司会を務め、自身のブログでもその釣果",
            exact_probe_with_context(
                model_path,
                "しかいをつとめ、じしんのぶろぐでもそのちょうか",
                "司会を務め、自身のブログでもその釣果",
                "釣りビジョンの『五畳半の狼』では",
            ),
        ),
        (
            "long_lambda_fallback_authority_resides",
            "権力行為を正当付ける権威は国民に存するという",
            exact_probe(
                model_path,
                "けんりょくこういをせいとうづけるけんいはこくみんにそんするという",
                "権力行為を正当付ける権威は国民に存するという",
            ),
        ),
    ]
}

fn literal_hiragana_probes(model_path: &[u8]) -> [NamedExactProbe; 8] {
    [
        (
            "literal_colloquial_hiragana_preserved",
            "ラッシュフォードはいたらおもろいんよな",
            exact_probe(
                model_path,
                "らっしゅふぉーどはいたらおもろいんよな",
                "ラッシュフォードはいたらおもろいんよな",
            ),
        ),
        (
            "literal_short_toka_hiragana_preserved",
            "とか",
            exact_probe(model_path, "とか", "とか"),
        ),
        (
            "literal_omoroi_hiragana_preserved",
            "おもろい",
            exact_probe(model_path, "おもろい", "おもろい"),
        ),
        (
            "literal_contextual_omoroi_hiragana_preserved",
            "ラッシュフォードはいたらおもろいんよな",
            exact_probe(
                model_path,
                "らっしゅふぉーどはいたらおもろいんよな",
                "ラッシュフォードはいたらおもろいんよな",
            ),
        ),
        (
            "literal_naru_hiragana_preserved",
            "なるんですか",
            exact_probe(model_path, "なるんですか", "なるんですか"),
        ),
        (
            "literal_toka_naru_hiragana_preserved",
            "なんでとかになるんですか",
            exact_probe(
                model_path,
                "なんでとかになるんですか",
                "なんでとかになるんですか",
            ),
        ),
        (
            "literal_mechakucha_hiragana_preserved",
            "とにかくめちゃくちゃになってます",
            exact_probe(
                model_path,
                "とにかくめちゃくちゃになってます",
                "とにかくめちゃくちゃになってます",
            ),
        ),
        (
            "converted_phrase_hiragana_tail",
            "戦後裸一貫から",
            exact_probe(model_path, "せんごはだかいっかんから", "戦後裸一貫から"),
        ),
    ]
}

fn long_context_exact_probes(
    model_path: &[u8],
) -> [(&'static str, &'static str, Option<ExactProbe>); 10] {
    [
        (
            "stable_prefix_confidence_repair",
            "ラーメンには自信があるが",
            exact_probe(
                model_path,
                "らーめんにはじしんがあるが",
                "ラーメンには自信があるが",
            ),
        ),
        (
            "topic_homophone_race_repair",
            "この競走はハンデキャップで行われ",
            exact_probe(
                model_path,
                "このきょうそうははんできゃっぷでおこなわれ",
                "この競走はハンデキャップで行われ",
            ),
        ),
        (
            "topic_homophone_ship_repair",
            "まで、イスラエルの艦船はスエズ運河を通行",
            exact_probe(
                model_path,
                "まで、いすらえるのかんせんはすえずうんがをつうこう",
                "まで、イスラエルの艦船はスエズ運河を通行",
            ),
        ),
        (
            "long_cost_gap_detention_repair",
            "また、別件逮捕は被疑者を早期に勾留するために",
            exact_probe(
                model_path,
                "また、べっけんたいほはひぎしゃをそうきにこうりゅうするために",
                "また、別件逮捕は被疑者を早期に勾留するために",
            ),
        ),
        (
            "long_cost_gap_school_repair",
            "旧制宮城県佐沼中学校",
            exact_probe(
                model_path,
                "きゅうせいみやぎけんさぬまちゅうがっこう",
                "旧制宮城県佐沼中学校",
            ),
        ),
        (
            "long_length_change_medical_departments",
            "内科・アレルギー科・リウマチ科・呼吸器内科・消化器内科・皮膚科",
            exact_probe(
                model_path,
                "ないか・あれるぎーか・りうまちか・こきゅうきないか・しょうかきないか・ひふか",
                "内科・アレルギー科・リウマチ科・呼吸器内科・消化器内科・皮膚科",
            ),
        ),
        (
            "long_length_change_hair_damage",
            "塩素で髪が茶色くボサボサに傷んでしまい",
            exact_probe(
                model_path,
                "えんそでかみがちゃいろくぼさぼさにいたんでしまい",
                "塩素で髪が茶色くボサボサに傷んでしまい",
            ),
        ),
        (
            "reopened_scope_performance_repair",
            "大ファンであり、武道館公演で",
            exact_probe_with_context(
                model_path,
                "だいふぁんであり、ぶどうかんこうえんで",
                "大ファンであり、武道館公演で",
                "レッド・ツェッペリンの",
            ),
        ),
        (
            "reopened_scope_museum_repair",
            "設立関連事業として「電話網の中の見えないミュージアム」が電話網の中に開設さ",
            exact_probe(
                model_path,
                "せつりつかんれんじぎょうとして「でんわもうのなかのみえないみゅーじあむ」がでんわもうのなかにかいせつさ",
                "設立関連事業として「電話網の中の見えないミュージアム」が電話網の中に開設さ",
            ),
        ),
        (
            "katakana_list_tail_repair",
            "以上で登場。最後の難関にふさわしく、ファンタ、ゲロッパ",
            exact_probe(
                model_path,
                "いじょうでとうじょう。さいごのなんかんにふさわしく、ふぁんた、げろっぱ",
                "以上で登場。最後の難関にふさわしく、ファンタ、ゲロッパ",
            ),
        ),
    ]
}

fn katakana_checkpoint_probes(model_path: &[u8]) -> [NamedExactProbe; 6] {
    [
        (
            "katakana_checkpoint_homophone_repair",
            "マンネルヘイム十字勲章受賞",
            exact_probe(
                model_path,
                "まんねるへいむじゅうじくんしょうじゅしょう",
                "マンネルヘイム十字勲章受賞",
            ),
        ),
        (
            "katakana_stable_boundary_voldemort_repair",
            "復活したヴォルデモートと死喰い人の脅威が高まると",
            exact_probe(
                model_path,
                "ふっかつしたゔぉるでもーととしくいびとのきょういがたかまると",
                "復活したヴォルデモートと死喰い人の脅威が高まると",
            ),
        ),
        (
            "katakana_stable_boundary_gundam_repair",
            "ウエポンシステム）と呼ばれるガンダムの改良プランに則っており",
            exact_probe(
                model_path,
                "うえぽんしすてむ）とよばれるがんだむのかいりょうぷらんにのっとっており",
                "ウエポンシステム）と呼ばれるガンダムの改良プランに則っており",
            ),
        ),
        (
            "katakana_stable_boundary_fair_trade_repair",
            "環境保全、フェアトレードなどといった社会的責任を追求する経営を貫くため",
            exact_probe(
                model_path,
                "かんきょうほぜん、ふぇあとれーどなどといったしゃかいてきせきにんをついきゅうするけいえいをつらぬくため",
                "環境保全、フェアトレードなどといった社会的責任を追求する経営を貫くため",
            ),
        ),
        (
            "katakana_stable_boundary_hard_energy_repair",
            "主にハードエナジー、トランスコアを制作している",
            exact_probe(
                model_path,
                "おもにはーどえなじー、とらんすこあをせいさくしている",
                "主にハードエナジー、トランスコアを制作している",
            ),
        ),
        (
            "katakana_stable_boundary_hyper_mall_repair",
            "ハイパーモールメルクス、千葉県国際総合水泳場",
            exact_probe(
                model_path,
                "はいぱーもーるめるくす、ちばけんこくさいそうごうすいえいじょう",
                "ハイパーモールメルクス、千葉県国際総合水泳場",
            ),
        ),
    ]
}

fn lexicalized_hiragana_boundary_probes(model_path: &[u8]) -> [NamedExactProbe; 8] {
    [
        (
            "lexicalized_dekiru_boundary_repair",
            "登山口近くで利用できる公共交通機関はなく",
            exact_probe(
                model_path,
                "とざんぐちちかくでりようできるこうきょうこうつうきかんはなく",
                "登山口近くで利用できる公共交通機関はなく",
            ),
        ),
        (
            "lexicalized_toiu_boundary_repair",
            "転覆した豪華客船から脱出する」という舞台設定だけであり",
            exact_probe(
                model_path,
                "てんぷくしたごうかきゃくせんからだっしゅつする」というぶたいせっていだけであり",
                "転覆した豪華客船から脱出する」という舞台設定だけであり",
            ),
        ),
        (
            "lexicalized_toiu_long_boundary_repair",
            "落下などで偶然そうなっただけというものは、確率分布であるのでプロファイ",
            exact_probe(
                model_path,
                "らっかなどでぐうぜんそうなっただけというものは、かくりつぶんぷであるのでぷろふぁい",
                "落下などで偶然そうなっただけというものは、確率分布であるのでプロファイ",
            ),
        ),
        (
            "reopened_two_kana_inflected_repair",
            "初めてロジャーズに会い",
            exact_probe(
                model_path,
                "はじめてろじゃーずにあい",
                "初めてロジャーズに会い",
            ),
        ),
        (
            "full_lattice_fragmented_suffix_repair",
            "供養塔を荒らした何者かの正体は",
            exact_probe(
                model_path,
                "くようとうをあらしたなにものかのしょうたいは",
                "供養塔を荒らした何者かの正体は",
            ),
        ),
        (
            "full_lattice_compound_suffix_repair",
            "繁殖期は野生下では春と秋であるが",
            exact_probe(
                model_path,
                "はんしょくきはやせいかでははるとあきであるが",
                "繁殖期は野生下では春と秋であるが",
            ),
        ),
        (
            "full_lattice_phrase_boundary_repair",
            "しかしレースは、いつもの逃げに精彩を欠いたキーストンが最後",
            exact_probe(
                model_path,
                "しかしれーすは、いつものにげにせいさいをかいたきーすとんがさいご",
                "しかしレースは、いつもの逃げに精彩を欠いたキーストンが最後",
            ),
        ),
        (
            "ambiguous_toru_stable_scope_repair",
            "挑発的な態度を取る相手に啖呵を切ることも珍しくない",
            exact_probe(
                model_path,
                "ちょうはつてきなたいどをとるあいてにたんかをきることもめずらしくない",
                "挑発的な態度を取る相手に啖呵を切ることも珍しくない",
            ),
        ),
    ]
}

fn reopened_literal_target_probes(model_path: &[u8]) -> [NamedExactProbe; 2] {
    [
        (
            "reopened_scope_literal_target_repair",
            "配置し、それを拡散板にてパネル上に拡散して直接照射",
            exact_probe(
                model_path,
                "はいちし、それをかくさんばんにてぱねるじょうにかくさんしてちょくせつしょうしゃ",
                "配置し、それを拡散板にてパネル上に拡散して直接照射",
            ),
        ),
        (
            "reopened_scope_suffix_retry",
            "不敵でしたたかな実力者で",
            exact_probe(
                model_path,
                "ふてきでしたたかなじつりょくしゃで",
                "不敵でしたたかな実力者で",
            ),
        ),
    ]
}

fn exact_probe_with_context(
    model_path: &[u8],
    input: &str,
    expected: &str,
    left_context: &str,
) -> Option<ExactProbe> {
    let handle = configured_engine(model_path, left_context).ok()?;
    let initial_response = type_text_final_response(handle, input);
    let already_expected = initial_response.contains(&format!(
        "{{\"type\":\"update_preedit\",\"text\":\"{expected}\"}}"
    ));
    // SAFETY: Snapshot creation reads the live handle synchronously.
    let task = unsafe {
        slime_live_neural_task_create_v2(
            handle,
            LIVE_MINIMUM_SWITCH_MARGIN,
            LIVE_LONG_READING_MINIMUM_SWITCH_MARGIN,
            LIVE_NUMERIC_BASE_SWITCH_MARGIN,
            LIVE_LONG_READING_LAMBDA,
        )
    };
    if task.is_null() {
        // SAFETY: The handle remains live and uniquely owned.
        unsafe { slime_destroy(handle) };
        return None;
    }
    let worker_status = run_on_worker(task);
    let mut capture = Capture::default();
    // SAFETY: The completed task and engine are live and exclusively accessed.
    let apply_status = unsafe {
        slime_live_neural_task_apply_actions_v2(
            handle,
            task,
            (&raw mut capture).cast(),
            Some(capture_action),
        )
    };
    // SAFETY: The task and handle are no longer used and are destroyed once.
    unsafe {
        slime_live_neural_task_destroy(task);
        slime_destroy(handle);
    }
    Some((
        worker_status,
        apply_status,
        capture.preedit,
        already_expected,
    ))
}

fn exact_probe_passed(probe: Option<&ExactProbe>, expected: &str) -> bool {
    probe.is_some_and(|(worker_status, apply_status, preedit, already_expected)| {
        if let Some(preedit) = preedit {
            *worker_status == STATUS_OK && *apply_status == STATUS_OK && preedit == expected
        } else {
            *already_expected
        }
    })
}

fn configured_engine(
    model_path: &[u8],
    left_context: &str,
) -> Result<*mut slime_ffi::SlimeHandle, String> {
    let handle = slime_create();
    if handle.is_null() {
        return Err("failed to create engine".to_owned());
    }
    // SAFETY: The handle and path remain live and exclusively accessed.
    let load_status = unsafe {
        slime_enable_neural_reranker_with_cost_gap(
            handle,
            model_path.as_ptr(),
            model_path.len(),
            0.2,
            1_000,
        )
    };
    if load_status != STATUS_OK {
        // SAFETY: The handle is live and uniquely owned.
        unsafe { slime_destroy(handle) };
        return Err(format!("failed to load model: status={load_status}"));
    }
    // SAFETY: The probe owns the handle exclusively and exercises delayed
    // LIVE ranking by construction.
    let live_status = unsafe { slime_set_live_neural_ranking_enabled(handle, true) };
    if live_status != STATUS_OK {
        // SAFETY: The handle is live and uniquely owned.
        unsafe { slime_destroy(handle) };
        return Err(format!(
            "failed to enable delayed LIVE ranking: status={live_status}"
        ));
    }
    // SAFETY: The live handle is exclusively accessed and the returned buffer
    // is destroyed exactly once.
    unsafe { slime_buffer_destroy(slime_set_options(handle, true, false)) };
    // SAFETY: The context bytes and handle remain live for this synchronous call.
    let context_status = unsafe {
        slime_set_external_left_context(handle, left_context.as_ptr(), left_context.len())
    };
    if context_status != STATUS_OK {
        // SAFETY: The handle is live and uniquely owned.
        unsafe { slime_destroy(handle) };
        return Err(format!("failed to set context: status={context_status}"));
    }
    Ok(handle)
}

fn benchmark_tasks(
    handle: *mut slime_ffi::SlimeHandle,
    sample_count: usize,
) -> Option<(Vec<f64>, Vec<f64>)> {
    let mut create_samples = Vec::with_capacity(sample_count);
    let mut run_samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        // SAFETY: Clear the prior composition on the exclusively owned handle.
        let clear = unsafe { slime_process(handle, EVENT_ENTER, 0) };
        // SAFETY: The returned buffer is destroyed exactly once.
        unsafe { slime_buffer_destroy(clear) };
        // SAFETY: Context bytes and handle are live for this synchronous call.
        if unsafe {
            slime_set_external_left_context(handle, LEFT_CONTEXT.as_ptr(), LEFT_CONTEXT.len())
        } != STATUS_OK
        {
            return None;
        }
        type_text(handle, INPUT);
        let create_started = Instant::now();
        // SAFETY: Snapshot creation reads the handle synchronously.
        let task = unsafe {
            slime_live_neural_task_create_v2(
                handle,
                LIVE_MINIMUM_SWITCH_MARGIN,
                LIVE_LONG_READING_MINIMUM_SWITCH_MARGIN,
                LIVE_NUMERIC_BASE_SWITCH_MARGIN,
                LIVE_LONG_READING_LAMBDA,
            )
        };
        create_samples.push(create_started.elapsed().as_secs_f64() * 1_000.0);
        if task.is_null() {
            return None;
        }
        let run_started = Instant::now();
        // SAFETY: This loop has exclusive task ownership. The production path
        // uses a persistent worker queue; direct execution avoids timing OS
        // thread creation rather than changing the inference workload.
        let status = unsafe { slime_live_neural_task_run(task) };
        run_samples.push(run_started.elapsed().as_secs_f64() * 1_000.0);
        // SAFETY: The completed task is destroyed exactly once.
        unsafe { slime_live_neural_task_destroy(task) };
        if status != STATUS_OK {
            return None;
        }
    }
    Some((create_samples, run_samples))
}

fn percentile(samples: &[f64], percentile: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[rank.min(sorted.len() - 1)]
}

fn run_on_worker(task: *mut SlimeLiveNeuralTask) -> u32 {
    if task.is_null() {
        return u32::MAX;
    }
    let address = task as usize;
    thread::spawn(move || {
        // SAFETY: Ownership of this independent task is transferred to this
        // worker and returned only after `join` completes.
        unsafe { slime_live_neural_task_run(address as *mut SlimeLiveNeuralTask) }
    })
    .join()
    .unwrap_or(u32::MAX)
}

fn type_text(handle: *mut slime_ffi::SlimeHandle, input: &str) {
    for character in input.chars() {
        let _ = process_character(handle, character);
    }
}

fn type_text_final_response(handle: *mut slime_ffi::SlimeHandle, input: &str) -> String {
    let mut response = String::new();
    for character in input.chars() {
        response = process_character(handle, character);
    }
    response
}

fn process_character(handle: *mut slime_ffi::SlimeHandle, character: char) -> String {
    // SAFETY: The caller retains a live, exclusively accessed handle.
    let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, u32::from(character)) };
    // SAFETY: The buffer is live for this copy and destroyed exactly once.
    let response = unsafe { copy_buffer(&buffer) };
    // SAFETY: The returned buffer is destroyed exactly once.
    unsafe { slime_buffer_destroy(buffer) };
    response
}

unsafe extern "C" fn capture_action(context: *mut c_void, action: *const SlimeActionViewV2) {
    // SAFETY: The caller supplies live pointers for this synchronous callback.
    let capture = unsafe { &mut *context.cast::<Capture>() };
    // SAFETY: The callback contract supplies a live action view.
    let action = unsafe { &*action };
    if action.kind == ACTION_UPDATE_PREEDIT {
        // SAFETY: The action text is a borrowed UTF-8 view for this callback.
        let bytes = unsafe { std::slice::from_raw_parts(action.text.data, action.text.len) };
        capture.preedit = Some(String::from_utf8_lossy(bytes).into_owned());
    }
}

unsafe fn copy_buffer(buffer: &SlimeBuffer) -> String {
    // SAFETY: The caller guarantees that the buffer is live for this read.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) };
    String::from_utf8_lossy(bytes).into_owned()
}
