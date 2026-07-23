//! C ABI for native platform adapters.
//!
//! The first version returns a compact JSON action list. This keeps Swift-side
//! integration simple while the action schema is still evolving.

use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use slime_core::{EnginePreferences, InputEvent, SlimeAction, SlimeEngine, UserData};

pub const EVENT_CHARACTER: u32 = 0;
pub const EVENT_SPACE: u32 = 1;
pub const EVENT_ENTER: u32 = 2;
pub const EVENT_ESCAPE: u32 = 3;
pub const EVENT_BACKSPACE: u32 = 4;
pub const EVENT_NEXT_CANDIDATE: u32 = 5;
pub const EVENT_PREVIOUS_CANDIDATE: u32 = 6;
pub const EVENT_SELECT_CANDIDATE: u32 = 7;
pub const EVENT_ACCEPT_CANDIDATE: u32 = 8;

pub struct SlimeHandle {
    engine: SlimeEngine,
}

#[repr(C)]
#[derive(Debug)]
pub struct SlimeBuffer {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl SlimeBuffer {
    fn from_string(value: String) -> Self {
        let mut bytes = value.into_bytes();
        let buffer = Self {
            data: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        };
        std::mem::forget(bytes);
        buffer
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn slime_create() -> *mut SlimeHandle {
    match catch_unwind(|| SlimeHandle {
        engine: SlimeEngine::bundled(),
    }) {
        Ok(handle) => Box::into_raw(Box::new(handle)),
        Err(_) => ptr::null_mut(),
    }
}

/// Creates an engine backed by user dictionary and history files in `data_dir`.
///
/// # Safety
///
/// `data_dir` must point to `data_dir_len` readable UTF-8 bytes for the duration
/// of this call. A null pointer is accepted only when the length is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_create_with_data_dir(
    data_dir: *const u8,
    data_dir_len: usize,
) -> *mut SlimeHandle {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if data_dir.is_null() && data_dir_len != 0 {
            return None;
        }
        let bytes = if data_dir_len == 0 {
            &[]
        } else {
            // SAFETY: The caller promises a readable byte slice for this call.
            unsafe { std::slice::from_raw_parts(data_dir, data_dir_len) }
        };
        let path = std::str::from_utf8(bytes).ok()?;
        Some(SlimeHandle {
            engine: SlimeEngine::bundled_with_user_data(UserData::load(path)),
        })
    }));

    match result {
        Ok(Some(handle)) => Box::into_raw(Box::new(handle)),
        Ok(None) | Err(_) => ptr::null_mut(),
    }
}

/// Destroys a handle returned by [`slime_create`].
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by [`slime_create`]. It must
/// not be used again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_destroy(handle: *mut SlimeHandle) {
    if !handle.is_null() {
        // SAFETY: The caller promises ownership of a live `slime_create` pointer.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Processes one input event and returns a UTF-8 JSON action list.
///
/// `value` is a Unicode scalar for [`EVENT_CHARACTER`] and a zero-based index
/// for [`EVENT_SELECT_CANDIDATE`]. It is ignored for other events. The returned
/// buffer must be released with [`slime_buffer_destroy`].
///
/// # Safety
///
/// `handle` must be null or a live, exclusively accessed pointer returned by
/// [`slime_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_process(
    handle: *mut SlimeHandle,
    event_kind: u32,
    value: u32,
) -> SlimeBuffer {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return error_response("null_handle");
        }

        let event = match decode_event(event_kind, value) {
            Ok(event) => event,
            Err(error) => return error_response(error),
        };

        // SAFETY: The caller promises a live, exclusively accessed handle.
        let handle = unsafe { &mut *handle };
        let actions = handle.engine.handle(event);
        success_response(&actions)
    }));

    SlimeBuffer::from_string(match result {
        Ok(response) => response,
        Err(_) => error_response("panic"),
    })
}

/// Updates runtime options and returns any resulting preedit/candidate actions.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_options(
    handle: *mut SlimeHandle,
    live_conversion: bool,
    history_completion: bool,
) -> SlimeBuffer {
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe {
        engine_control(handle, |engine| {
            engine.set_preferences(EnginePreferences {
                live_conversion,
                history_completion,
                history_learning: history_completion,
                dictionary_packs: 0,
            })
        })
    }
}

/// Updates runtime options, including the enabled domain dictionary bit mask.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_options_v2(
    handle: *mut SlimeHandle,
    live_conversion: bool,
    history_completion: bool,
    dictionary_packs: u32,
) -> SlimeBuffer {
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe {
        engine_control(handle, |engine| {
            engine.set_preferences(EnginePreferences {
                live_conversion,
                history_completion,
                history_learning: history_completion,
                dictionary_packs,
            })
        })
    }
}

/// Updates runtime options, separating history suggestions from new learning.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_options_v3(
    handle: *mut SlimeHandle,
    live_conversion: bool,
    history_completion: bool,
    history_learning: bool,
    dictionary_packs: u32,
) -> SlimeBuffer {
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe {
        engine_control(handle, |engine| {
            engine.set_preferences(EnginePreferences {
                live_conversion,
                history_completion,
                history_learning,
                dictionary_packs,
            })
        })
    }
}

/// Reloads user dictionary and history files from the configured data folder.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_reload_user_data(handle: *mut SlimeHandle) -> SlimeBuffer {
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe { engine_control(handle, SlimeEngine::reload_user_data) }
}

/// Returns the bundled domain dictionary words for `mask` as UTF-8 JSON.
///
/// The mask uses the same bits as the `dictionary_packs` option. The returned
/// buffer must be released with [`slime_buffer_destroy`].
#[unsafe(no_mangle)]
pub extern "C" fn slime_domain_dictionary_words(mask: u32) -> SlimeBuffer {
    let result = catch_unwind(|| {
        let mut output = String::from("{\"ok\":true,\"words\":[");
        for (index, (reading, surface)) in slime_core::domain_dictionary_words(mask)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"reading\":");
            write_json_string(&mut output, reading);
            output.push_str(",\"surface\":");
            write_json_string(&mut output, surface);
            output.push('}');
        }
        output.push_str("]}");
        output
    });
    SlimeBuffer::from_string(result.unwrap_or_else(|_| error_response("panic")))
}

/// Releases a buffer returned by [`slime_process`].
///
/// # Safety
///
/// `buffer` must be an unmodified value returned by [`slime_process`] and may be
/// released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_buffer_destroy(buffer: SlimeBuffer) {
    if buffer.data.is_null() {
        return;
    }

    // SAFETY: The caller promises this is the original allocation triple.
    drop(unsafe { Vec::from_raw_parts(buffer.data, buffer.len, buffer.capacity) });
}

fn decode_event(event_kind: u32, value: u32) -> Result<InputEvent, &'static str> {
    match event_kind {
        EVENT_CHARACTER => char::from_u32(value)
            .map(InputEvent::Character)
            .ok_or("invalid_unicode_scalar"),
        EVENT_SPACE => Ok(InputEvent::Space),
        EVENT_ENTER => Ok(InputEvent::Enter),
        EVENT_ESCAPE => Ok(InputEvent::Escape),
        EVENT_BACKSPACE => Ok(InputEvent::Backspace),
        EVENT_NEXT_CANDIDATE => Ok(InputEvent::NextCandidate),
        EVENT_PREVIOUS_CANDIDATE => Ok(InputEvent::PreviousCandidate),
        EVENT_SELECT_CANDIDATE => Ok(InputEvent::SelectCandidate(value)),
        EVENT_ACCEPT_CANDIDATE => Ok(InputEvent::AcceptCandidate),
        _ => Err("invalid_event_kind"),
    }
}

unsafe fn engine_control(
    handle: *mut SlimeHandle,
    operation: impl FnOnce(&mut SlimeEngine) -> Vec<SlimeAction>,
) -> SlimeBuffer {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return error_response("null_handle");
        }
        // SAFETY: The caller-facing functions require exclusive live access.
        let handle = unsafe { &mut *handle };
        success_response(&operation(&mut handle.engine))
    }));
    SlimeBuffer::from_string(match result {
        Ok(response) => response,
        Err(_) => error_response("panic"),
    })
}

fn success_response(actions: &[SlimeAction]) -> String {
    let mut output = String::from("{\"ok\":true,\"actions\":[");
    for (index, action) in actions.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write_action(&mut output, action);
    }
    output.push_str("]}");
    output
}

fn error_response(error: &str) -> String {
    let mut output = String::from("{\"ok\":false,\"error\":");
    write_json_string(&mut output, error);
    output.push('}');
    output
}

fn write_action(output: &mut String, action: &SlimeAction) {
    match action {
        SlimeAction::UpdatePreedit(text) => {
            output.push_str("{\"type\":\"update_preedit\",\"text\":");
            write_json_string(output, text);
            output.push('}');
        }
        SlimeAction::ShowCandidates {
            candidates,
            selected,
        } => {
            output.push_str("{\"type\":\"show_candidates\",\"selected\":");
            write!(output, "{selected}").expect("writing to String cannot fail");
            output.push_str(",\"candidates\":[");
            for (index, candidate) in candidates.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_string(output, candidate);
            }
            output.push_str("]}");
        }
        SlimeAction::HideCandidates => output.push_str("{\"type\":\"hide_candidates\"}"),
        SlimeAction::Commit(text) => {
            output.push_str("{\"type\":\"commit\",\"text\":");
            write_json_string(output, text);
            output.push('}');
        }
        SlimeAction::Clear => output.push_str("{\"type\":\"clear\"}"),
        SlimeAction::ForwardKey => output.push_str("{\"type\":\"forward_key\"}"),
    }
}

fn write_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", u32::from(character))
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_CHARACTER, EVENT_ENTER, EVENT_SPACE, SlimeBuffer, slime_buffer_destroy, slime_create,
        slime_create_with_data_dir, slime_destroy, slime_domain_dictionary_words, slime_process,
        slime_set_options, slime_set_options_v2, slime_set_options_v3,
    };
    use std::fs;

    unsafe fn copy_buffer(buffer: &SlimeBuffer) -> String {
        // SAFETY: Tests read a live buffer before handing it back to its destructor.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) };
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn ffi_round_trip_returns_utf8_actions() {
        let handle = slime_create();
        assert!(!handle.is_null());

        for character in "nihon".chars() {
            // SAFETY: `handle` is live and accessed serially in this test.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: `buffer` is live until the destroy call below.
            let json = unsafe { copy_buffer(&buffer) };
            assert!(json.contains("\"ok\":true"));
            // SAFETY: `buffer` has not previously been released.
            unsafe { slime_buffer_destroy(buffer) };
        }

        // SAFETY: `handle` is live and accessed serially in this test.
        let buffer = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: `buffer` is live until the destroy call below.
        let json = unsafe { copy_buffer(&buffer) };
        assert!(json.contains("日本"));
        assert!(json.contains("show_candidates"));

        // SAFETY: Resources are live and each is destroyed exactly once.
        unsafe {
            slime_buffer_destroy(buffer);
            slime_destroy(handle);
        }
    }

    #[test]
    fn invalid_event_is_reported_without_panicking() {
        let handle = slime_create();
        // SAFETY: `handle` is live and accessed serially in this test.
        let buffer = unsafe { slime_process(handle, 999, 0) };
        // SAFETY: `buffer` is live until the destroy call below.
        let json = unsafe { copy_buffer(&buffer) };

        assert_eq!(json, "{\"ok\":false,\"error\":\"invalid_event_kind\"}");

        // SAFETY: Resources are live and each is destroyed exactly once.
        unsafe {
            slime_buffer_destroy(buffer);
            slime_destroy(handle);
        }
    }

    #[test]
    fn null_handle_is_an_error() {
        // SAFETY: A null handle is explicitly accepted and reported as an error.
        let buffer = unsafe { slime_process(std::ptr::null_mut(), EVENT_SPACE, 0) };
        // SAFETY: `buffer` is live until the destroy call below.
        let json = unsafe { copy_buffer(&buffer) };
        assert_eq!(json, "{\"ok\":false,\"error\":\"null_handle\"}");
        // SAFETY: `buffer` has not previously been released.
        unsafe { slime_buffer_destroy(buffer) };
    }

    #[test]
    fn data_directory_and_options_enable_history_completion() {
        let directory = std::env::temp_dir().join(format!("slime-ffi-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("history.tsv"),
            "# slime-history-v1\nぱふぉーまんす\tパフォーマンス\t5\t10\n",
        )
        .unwrap();
        let path = directory.to_string_lossy();
        // SAFETY: `path` remains readable for the duration of the creation call.
        let handle = unsafe { slime_create_with_data_dir(path.as_ptr(), path.len()) };
        assert!(!handle.is_null());

        // SAFETY: `handle` is live and exclusively accessed in this test.
        let options = unsafe { slime_set_options(handle, false, true) };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };
        let mut latest = String::new();
        for character in "pafo".chars() {
            // SAFETY: `handle` is live and exclusively accessed in this test.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: The buffer remains live until the destroy call below.
            latest = unsafe { copy_buffer(&buffer) };
            // SAFETY: `buffer` is the original live buffer.
            unsafe { slime_buffer_destroy(buffer) };
        }
        assert!(latest.contains("パフォーマンス"));

        // SAFETY: `handle` is live and has not previously been released.
        unsafe { slime_destroy(handle) };
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn v2_options_enable_domain_dictionary() {
        let handle = slime_create();
        assert!(!handle.is_null());

        // SAFETY: `handle` is live and exclusively accessed in this test.
        let options = unsafe { slime_set_options_v2(handle, false, false, 1) };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };
        for character in "suwifutoyu-ai".chars() {
            // SAFETY: `handle` is live and exclusively accessed in this test.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: `buffer` is the original live buffer.
            unsafe { slime_buffer_destroy(buffer) };
        }
        // SAFETY: `handle` is live and exclusively accessed in this test.
        let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: `conversion` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&conversion) };
        assert!(json.contains("SwiftUI"), "{json}");
        // SAFETY: `conversion` is the original live buffer.
        unsafe { slime_buffer_destroy(conversion) };

        // SAFETY: `handle` is live and has not previously been released.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn domain_dictionary_words_are_exposed_as_json() {
        let buffer = slime_domain_dictionary_words(1);
        // SAFETY: `buffer` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&buffer) };
        assert!(json.starts_with("{\"ok\":true,\"words\":["), "{json}");
        assert!(json.contains("\"reading\":"), "{json}");
        assert!(json.contains("\"surface\":"), "{json}");
        // SAFETY: `buffer` is the original live buffer.
        unsafe { slime_buffer_destroy(buffer) };

        let empty = slime_domain_dictionary_words(0);
        // SAFETY: `empty` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&empty) };
        assert_eq!(json, "{\"ok\":true,\"words\":[]}");
        // SAFETY: `empty` is the original live buffer.
        unsafe { slime_buffer_destroy(empty) };
    }

    #[test]
    fn v3_options_can_use_history_without_learning() {
        let directory =
            std::env::temp_dir().join(format!("slime-ffi-learning-paused-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let history_path = directory.join("history.tsv");
        let original = "# slime-history-v1\nかんじ\t感じ\t2\t10\n";
        fs::write(&history_path, original).unwrap();
        let path = directory.to_string_lossy();
        // SAFETY: `path` remains readable for the duration of the creation call.
        let handle = unsafe { slime_create_with_data_dir(path.as_ptr(), path.len()) };
        assert!(!handle.is_null());

        // SAFETY: `handle` is live and exclusively accessed in this test.
        let options = unsafe { slime_set_options_v3(handle, false, true, false, 0) };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };
        for character in "kanji".chars() {
            // SAFETY: `handle` is live and exclusively accessed in this test.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: `buffer` is the original live buffer.
            unsafe { slime_buffer_destroy(buffer) };
        }
        // SAFETY: `handle` is live and exclusively accessed in this test.
        let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: `conversion` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&conversion) };
        assert!(json.contains("感じ"), "{json}");
        // SAFETY: buffers are released exactly once.
        unsafe {
            slime_buffer_destroy(conversion);
            slime_buffer_destroy(slime_process(handle, EVENT_ENTER, 0));
            slime_destroy(handle);
        }
        assert_eq!(fs::read(&history_path).unwrap(), original.as_bytes());
        fs::remove_dir_all(directory).unwrap();
    }
}
