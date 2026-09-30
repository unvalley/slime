//! Trace actual delayed worker scopes while replaying every input character.
use slime_ffi::{
    ACTION_COMMIT, ACTION_UPDATE_PREEDIT, EVENT_CHARACTER, STATUS_OK, SlimeActionViewV2,
    SlimeLiveNeuralTask, slime_buffer_destroy, slime_create, slime_destroy,
    slime_enable_neural_reranker_with_cost_gap, slime_live_neural_task_apply_actions_v2,
    slime_live_neural_task_create_v2, slime_live_neural_task_destroy, slime_live_neural_task_run,
    slime_process_actions_v2, slime_set_external_left_context,
    slime_set_live_neural_ranking_enabled, slime_set_options_v5,
};
use std::{env, ffi::c_void, process::ExitCode};

#[derive(Default)]
struct Capture {
    preedit: String,
    committed: bool,
}

unsafe extern "C" fn capture_action(context: *mut c_void, action: *const SlimeActionViewV2) {
    // SAFETY: Both pointers are borrowed from the synchronous FFI call below.
    let capture = unsafe { &mut *context.cast::<Capture>() };
    let action = unsafe { &*action };
    if action.kind == ACTION_COMMIT {
        capture.committed = true;
    }
    if action.kind == ACTION_UPDATE_PREEDIT {
        // SAFETY: Text is valid for this callback and copied before returning.
        let bytes = unsafe { std::slice::from_raw_parts(action.text.data, action.text.len) };
        capture.preedit = String::from_utf8_lossy(bytes).into_owned();
    }
}

fn main() -> ExitCode {
    let mut args: Vec<_> = env::args().skip(1).collect();
    let capture_scores = args.last().is_some_and(|arg| arg == "--scores");
    if capture_scores {
        args.pop();
    }
    if !(2..=3).contains(&args.len()) {
        eprintln!("usage: live_neural_trace MODEL INPUT [LEFT_CONTEXT] [--scores]");
        return ExitCode::FAILURE;
    }
    let handle = slime_create();
    assert!(!handle.is_null());
    let context = args.get(2).map_or("", String::as_str);
    let mut capture = Capture::default();
    let mut rows = Vec::new();
    // SAFETY: This process owns the engine and every task exclusively. All
    // borrowed paths, context and callback data outlive their synchronous calls.
    unsafe {
        assert_eq!(
            slime_enable_neural_reranker_with_cost_gap(
                handle,
                args[0].as_ptr(),
                args[0].len(),
                0.2,
                1_000,
            ),
            STATUS_OK
        );
        slime_buffer_destroy(slime_set_options_v5(
            handle,
            true,
            false,
            false,
            0,
            false,
            slime_core::ALL_DATE_FORMATS,
        ));
        assert_eq!(
            slime_set_external_left_context(handle, context.as_ptr(), context.len()),
            STATUS_OK
        );
        assert_eq!(
            slime_set_live_neural_ranking_enabled(handle, true),
            STATUS_OK
        );
        for (position, character) in args[1].chars().enumerate() {
            assert_eq!(
                slime_process_actions_v2(
                    handle,
                    EVENT_CHARACTER,
                    u32::from(character),
                    (&raw mut capture).cast(),
                    Some(capture_action),
                ),
                STATUS_OK
            );
            assert!(!capture.committed, "unexpected commit during input");
            let before = capture.preedit.clone();
            let task = slime_live_neural_task_create_v2(handle, 0.2, 0.3, 0.1, 0.6);
            let mut row = serde_json::json!({
                "position": position + 1, "key": character.to_string(), "before": before,
            });
            if !task.is_null() {
                row["reading"] = serde_json::json!((*task).evaluation_request_reading());
                row["base"] = serde_json::json!((*task).evaluation_base_target_surface());
                row["has_stable_prefix"] =
                    serde_json::json!((*task).evaluation_has_stable_prefix());
                row["reopens"] = serde_json::json!((*task).evaluation_reopens_stable_prefix());
                row["candidates"] = serde_json::json!((*task).evaluation_rankable_surfaces());
                let status = slime_live_neural_task_run(task);
                row["status"] = serde_json::json!(status);
                record_scoring_details(&*task, &mut row, capture_scores);
                if status == STATUS_OK {
                    assert_eq!(
                        slime_live_neural_task_apply_actions_v2(
                            handle,
                            task,
                            (&raw mut capture).cast(),
                            Some(capture_action),
                        ),
                        STATUS_OK
                    );
                }
                assert!(!capture.committed, "unexpected commit during ranking");
                slime_live_neural_task_destroy(task);
            }
            row["after"] = serde_json::json!(capture.preedit);
            rows.push(row);
        }
        slime_destroy(handle);
    }
    println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    ExitCode::SUCCESS
}

fn record_scoring_details(
    task: &SlimeLiveNeuralTask,
    row: &mut serde_json::Value,
    capture_scores: bool,
) {
    row["ranked"] = serde_json::json!(task.evaluation_ranked_surfaces());
    row["dictionary_base_is_safe"] = serde_json::json!(task.evaluation_dictionary_base_is_safe());
    if capture_scores {
        row["logliks"] = serde_json::json!(task.evaluation_logliks());
    }
    if let Some(request) = task.evaluation_request() {
        row["request"] = serde_json::json!({
            "reading": request.reading,
            "left_context": request.left_context,
            "candidates": request.candidates.iter().map(|candidate| {
                serde_json::json!({"surface": candidate.surface, "cost": candidate.cost})
            }).collect::<Vec<_>>(),
        });
    }
}
