//! Local end-to-end latency probe for the optional first-Space neural ranker.

use std::env;
use std::process::ExitCode;
use std::time::Instant;

use slime_ffi::{
    EVENT_CHARACTER, EVENT_ENTER, EVENT_SPACE, STATUS_OK, SlimeBuffer, slime_buffer_destroy,
    slime_create, slime_destroy, slime_enable_neural_reranker_with_cost_gap, slime_process,
};

const PROBE_INPUT: &str = "soshiteryoushin'nisaikaishitainda";
const PROBE_EXPECTED: &str = "そして両親に再会したいんだ";

fn main() -> ExitCode {
    let Some(model_path) = env::args().nth(1) else {
        eprintln!("usage: neural_probe MODEL.gguf");
        return ExitCode::FAILURE;
    };
    let model_path = model_path.into_bytes();
    let handle = slime_create();
    if handle.is_null() {
        eprintln!("failed to create engine");
        return ExitCode::FAILURE;
    }

    let load_started = Instant::now();
    // SAFETY: The handle and byte slice remain live and exclusively accessed.
    let status = unsafe {
        slime_enable_neural_reranker_with_cost_gap(
            handle,
            model_path.as_ptr(),
            model_path.len(),
            0.2,
            1_000,
        )
    };
    let load_elapsed = load_started.elapsed();
    if status != STATUS_OK {
        eprintln!("failed to load model: status={status}");
        // SAFETY: The handle remains live and exclusively owned.
        unsafe { slime_destroy(handle) };
        return ExitCode::FAILURE;
    }

    let second_handle = slime_create();
    if second_handle.is_null() {
        eprintln!("failed to create second engine");
        // SAFETY: The first handle remains live and exclusively owned.
        unsafe { slime_destroy(handle) };
        return ExitCode::FAILURE;
    }
    let second_attach_started = Instant::now();
    // SAFETY: The second handle and model path remain live and exclusively accessed.
    let second_status = unsafe {
        slime_enable_neural_reranker_with_cost_gap(
            second_handle,
            model_path.as_ptr(),
            model_path.len(),
            0.2,
            1_000,
        )
    };
    let second_attach_elapsed = second_attach_started.elapsed();
    if second_status != STATUS_OK {
        eprintln!("failed to attach shared model: status={second_status}");
        // SAFETY: Both handles remain live and exclusively owned.
        unsafe {
            slime_destroy(second_handle);
            slime_destroy(handle);
        }
        return ExitCode::FAILURE;
    }

    type_text(handle, PROBE_INPUT);
    let space_started = Instant::now();
    // SAFETY: The handle remains live and exclusively accessed.
    let buffer = unsafe { slime_process(handle, EVENT_SPACE, 0) };
    let space_elapsed = space_started.elapsed();
    // SAFETY: The buffer contains a live UTF-8 response until destruction.
    let response = unsafe { copy_buffer(&buffer) };
    // SAFETY: The handle remains live and exclusively accessed.
    unsafe { slime_buffer_destroy(buffer) };
    // SAFETY: The handle remains live and exclusively accessed.
    let enter = unsafe { slime_process(handle, EVENT_ENTER, 0) };
    // SAFETY: The returned buffer is destroyed exactly once.
    unsafe { slime_buffer_destroy(enter) };
    type_text(handle, "nihongo");
    let warm_space_started = Instant::now();
    // SAFETY: The handle remains live and exclusively accessed.
    let warm_space = unsafe { slime_process(handle, EVENT_SPACE, 0) };
    let warm_space_elapsed = warm_space_started.elapsed();
    // SAFETY: The buffer and handle are each destroyed exactly once.
    unsafe {
        slime_buffer_destroy(warm_space);
        slime_destroy(second_handle);
        slime_destroy(handle);
    }

    println!("model_load_ms={:.3}", load_elapsed.as_secs_f64() * 1_000.0);
    println!(
        "second_model_attach_ms={:.3}",
        second_attach_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "first_space_ms={:.3}",
        space_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "warm_space_ms={:.3}",
        warm_space_elapsed.as_secs_f64() * 1_000.0
    );
    let selected_expected = response.contains(&format!(
        "{{\"type\":\"update_preedit\",\"text\":\"{PROBE_EXPECTED}\"}}"
    ));
    println!("selected_expected={selected_expected}");
    if selected_expected {
        ExitCode::SUCCESS
    } else {
        eprintln!("unexpected response: {response}");
        ExitCode::FAILURE
    }
}

fn type_text(handle: *mut slime_ffi::SlimeHandle, input: &str) {
    for character in input.chars() {
        // SAFETY: The caller retains a live, exclusively accessed handle.
        let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, u32::from(character)) };
        // SAFETY: Each returned buffer is destroyed exactly once.
        unsafe { slime_buffer_destroy(buffer) };
    }
}

unsafe fn copy_buffer(buffer: &SlimeBuffer) -> String {
    // SAFETY: The caller guarantees that the buffer is live for this read.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) };
    String::from_utf8_lossy(bytes).into_owned()
}
