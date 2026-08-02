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
pub const EVENT_TRANSFORM_HIRAGANA: u32 = 9;
pub const EVENT_TRANSFORM_FULL_KATAKANA: u32 = 10;
pub const EVENT_TRANSFORM_HALF_KATAKANA: u32 = 11;
pub const EVENT_TRANSFORM_FULL_ALPHANUMERIC: u32 = 12;
pub const EVENT_TRANSFORM_HALF_ALPHANUMERIC: u32 = 13;
pub const EVENT_NEXT_SEGMENT: u32 = 14;
pub const EVENT_PREVIOUS_SEGMENT: u32 = 15;
pub const EVENT_EXPAND_SEGMENT: u32 = 16;
pub const EVENT_SHRINK_SEGMENT: u32 = 17;

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
                private_mode: false,
                date_format_mask: slime_core::ALL_DATE_FORMATS,
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
                private_mode: false,
                date_format_mask: slime_core::ALL_DATE_FORMATS,
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
                private_mode: false,
                date_format_mask: slime_core::ALL_DATE_FORMATS,
            })
        })
    }
}

/// Updates runtime options, including process-local private mode.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_options_v4(
    handle: *mut SlimeHandle,
    live_conversion: bool,
    history_completion: bool,
    history_learning: bool,
    dictionary_packs: u32,
    private_mode: bool,
) -> SlimeBuffer {
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe {
        engine_control(handle, |engine| {
            engine.set_preferences(EnginePreferences {
                live_conversion,
                history_completion,
                history_learning,
                dictionary_packs,
                private_mode,
                date_format_mask: slime_core::ALL_DATE_FORMATS,
            })
        })
    }
}

/// Updates runtime options, including the enabled date candidate formats.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_options_v5(
    handle: *mut SlimeHandle,
    live_conversion: bool,
    history_completion: bool,
    history_learning: bool,
    dictionary_packs: u32,
    private_mode: bool,
    date_format_mask: u32,
) -> SlimeBuffer {
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe {
        engine_control(handle, |engine| {
            engine.set_preferences(EnginePreferences {
                live_conversion,
                history_completion,
                history_learning,
                dictionary_packs,
                private_mode,
                date_format_mask,
            })
        })
    }
}

/// Starts explicit reconversion of a selected committed UTF-8 surface.
///
/// # Safety
///
/// `handle` must be live and `surface` must point to `surface_len` readable
/// bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_begin_reconversion(
    handle: *mut SlimeHandle,
    surface: *const u8,
    surface_len: usize,
) -> SlimeBuffer {
    if handle.is_null() {
        return SlimeBuffer::from_string(error_response("null_handle"));
    }
    // SAFETY: The caller promises readable bytes for the duration of the call.
    let Some(surface) = (unsafe { decode_utf8_argument(surface, surface_len) }) else {
        return SlimeBuffer::from_string(error_response("invalid_surface"));
    };
    // SAFETY: This function's contract requires a live, exclusive handle.
    unsafe { engine_control(handle, |engine| engine.begin_reconversion(surface)) }
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

/// Returns metadata for external dictionary packs loaded by `handle`.
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by an IME creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_installed_dictionary_packs(
    handle: *const SlimeHandle,
) -> SlimeBuffer {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return error_response("null_handle");
        }
        // SAFETY: The caller promises a live handle. This function only reads it.
        let engine = unsafe { &(*handle).engine };
        let mut output = String::from("{\"ok\":true,\"packs\":[");
        for (index, pack) in engine.installed_dictionary_packs().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"id\":");
            write_json_string(&mut output, &pack.id);
            write!(output, ",\"formatVersion\":{}", pack.format_version)
                .expect("writing to String cannot fail");
            output.push_str(",\"name\":");
            write_json_string(&mut output, &pack.name);
            output.push_str(",\"version\":");
            write_json_string(&mut output, &pack.version);
            output.push_str(",\"license\":");
            write_json_string(&mut output, &pack.license);
            output.push_str(",\"minimumSlimeVersion\":");
            write_optional_json_string(&mut output, pack.minimum_slime_version.as_deref());
            output.push_str(",\"publishedAt\":");
            write_optional_json_string(&mut output, pack.published_at.as_deref());
            output.push_str(",\"provenance\":");
            write_optional_json_string(&mut output, pack.provenance.as_deref());
            output.push_str(",\"entriesSHA256\":");
            write_optional_json_string(&mut output, pack.entries_sha256.as_deref());
            write!(output, ",\"entryCount\":{}}}", pack.entry_count)
                .expect("writing to String cannot fail");
        }
        output.push_str("],\"errors\":[");
        for (index, error) in engine.dictionary_pack_load_errors().iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"file\":");
            write_json_string(&mut output, &error.file);
            output.push_str(",\"message\":");
            write_json_string(&mut output, &error.message);
            output.push('}');
        }
        output.push_str("]}");
        output
    }));
    SlimeBuffer::from_string(result.unwrap_or_else(|_| error_response("panic")))
}

/// Returns the words of one external dictionary pack loaded by `handle`.
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by an IME creation function.
/// `pack_id` must point to `pack_id_len` readable UTF-8 bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_installed_dictionary_pack_words(
    handle: *const SlimeHandle,
    pack_id: *const u8,
    pack_id_len: usize,
) -> SlimeBuffer {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return error_response("null_handle");
        }
        // SAFETY: This FFI function requires the argument bytes to remain
        // readable for the duration of the call.
        let Some(id) = (unsafe { decode_utf8_argument(pack_id, pack_id_len) }) else {
            return error_response("invalid_dictionary_pack_id");
        };
        // SAFETY: The caller promises a live handle. This function only reads it.
        let engine = unsafe { &(*handle).engine };
        let Some(words) = engine.installed_dictionary_pack_words(id) else {
            return error_response("unknown_dictionary_pack");
        };
        let mut output = String::from("{\"ok\":true,\"words\":[");
        for (index, word) in words.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str("{\"reading\":");
            write_json_string(&mut output, &word.reading);
            output.push_str(",\"surface\":");
            write_json_string(&mut output, &word.surface);
            output.push('}');
        }
        output.push_str("]}");
        output
    }));
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
        EVENT_TRANSFORM_HIRAGANA => Ok(InputEvent::TransformHiragana),
        EVENT_TRANSFORM_FULL_KATAKANA => Ok(InputEvent::TransformFullKatakana),
        EVENT_TRANSFORM_HALF_KATAKANA => Ok(InputEvent::TransformHalfKatakana),
        EVENT_TRANSFORM_FULL_ALPHANUMERIC => Ok(InputEvent::TransformFullAlphanumeric),
        EVENT_TRANSFORM_HALF_ALPHANUMERIC => Ok(InputEvent::TransformHalfAlphanumeric),
        EVENT_NEXT_SEGMENT => Ok(InputEvent::NextSegment),
        EVENT_PREVIOUS_SEGMENT => Ok(InputEvent::PreviousSegment),
        EVENT_EXPAND_SEGMENT => Ok(InputEvent::ExpandSegment),
        EVENT_SHRINK_SEGMENT => Ok(InputEvent::ShrinkSegment),
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
        SlimeAction::UpdateSegmentedPreedit {
            text,
            selection_start,
            selection_length,
        } => {
            output.push_str("{\"type\":\"update_preedit\",\"text\":");
            write_json_string(output, text);
            write!(
                output,
                ",\"selectedStart\":{selection_start},\"selectedLength\":{selection_length}}}"
            )
            .expect("writing to String cannot fail");
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

unsafe fn decode_utf8_argument<'a>(pointer: *const u8, length: usize) -> Option<&'a str> {
    if pointer.is_null() {
        return (length == 0).then_some("");
    }
    // SAFETY: Callers of the FFI functions using this helper promise a readable
    // byte slice for the duration of the call.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer, length) }).ok()
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

fn write_optional_json_string(output: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_json_string(output, value);
    } else {
        output.push_str("null");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_CHARACTER, EVENT_ENTER, EVENT_PREVIOUS_SEGMENT, EVENT_SPACE, SlimeBuffer,
        slime_begin_reconversion, slime_buffer_destroy, slime_create, slime_create_with_data_dir,
        slime_destroy, slime_domain_dictionary_words, slime_installed_dictionary_pack_words,
        slime_installed_dictionary_packs, slime_process, slime_set_options, slime_set_options_v2,
        slime_set_options_v3, slime_set_options_v4, slime_set_options_v5,
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
    fn reconversion_and_segment_selection_cross_the_c_boundary() {
        let handle = slime_create();
        let surface = "日本";
        // SAFETY: `handle` and `surface` remain live and are accessed serially.
        let reconversion =
            unsafe { slime_begin_reconversion(handle, surface.as_ptr(), surface.len()) };
        // SAFETY: `reconversion` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&reconversion) };
        assert!(
            json.contains("show_candidates") && json.contains("日本"),
            "{json}"
        );
        // SAFETY: `reconversion` is the original live buffer.
        unsafe { slime_buffer_destroy(reconversion) };

        // Commit the reconversion before starting a separate phrase.
        // SAFETY: `handle` is live and exclusively accessed.
        let committed = unsafe { slime_process(handle, EVENT_ENTER, 0) };
        // SAFETY: `committed` is the original live buffer.
        unsafe { slime_buffer_destroy(committed) };
        for character in "watashihanihon".chars() {
            // SAFETY: `handle` is live and exclusively accessed.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: `buffer` is the original live buffer.
            unsafe { slime_buffer_destroy(buffer) };
        }
        // SAFETY: `handle` is live and exclusively accessed.
        let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: `conversion` is the original live buffer.
        unsafe { slime_buffer_destroy(conversion) };
        // SAFETY: `handle` is live and exclusively accessed.
        let segmented = unsafe { slime_process(handle, EVENT_PREVIOUS_SEGMENT, 0) };
        // SAFETY: `segmented` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&segmented) };
        assert!(
            json.contains("selectedStart") && json.contains("selectedLength"),
            "{json}"
        );

        // SAFETY: Resources are live and each is destroyed exactly once.
        unsafe {
            slime_buffer_destroy(segmented);
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
    fn v4_private_mode_hides_history_and_prevents_learning() {
        let directory =
            std::env::temp_dir().join(format!("slime-ffi-private-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let original = "# slime-history-v1\nぱふぉーまんす\tパフォーマンス履歴\t5\t10\n";
        fs::write(directory.join("history.tsv"), original).unwrap();
        let path = directory.to_string_lossy();
        // SAFETY: `path` remains readable for the duration of the creation call.
        let handle = unsafe { slime_create_with_data_dir(path.as_ptr(), path.len()) };
        assert!(!handle.is_null());

        // SAFETY: `handle` is live and exclusively accessed in this test.
        let options = unsafe { slime_set_options_v4(handle, false, true, true, 0, true) };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };
        let mut latest = String::new();
        for character in "pafomansu".chars() {
            // SAFETY: `handle` is live and exclusively accessed in this test.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: `buffer` remains live until the destroy call below.
            latest = unsafe { copy_buffer(&buffer) };
            // SAFETY: `buffer` is the original live buffer.
            unsafe { slime_buffer_destroy(buffer) };
        }
        assert!(!latest.contains("パフォーマンス履歴"), "{latest}");
        // SAFETY: `handle` is live and exclusively accessed in this test.
        let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: `conversion` is the original live buffer.
        unsafe { slime_buffer_destroy(conversion) };
        // SAFETY: `handle` is live and exclusively accessed in this test.
        let commit = unsafe { slime_process(handle, EVENT_ENTER, 0) };
        // SAFETY: `commit` is the original live buffer.
        unsafe { slime_buffer_destroy(commit) };

        assert_eq!(
            fs::read_to_string(directory.join("history.tsv")).unwrap(),
            original
        );
        // SAFETY: `handle` is live and has not previously been released.
        unsafe { slime_destroy(handle) };
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn v5_limits_date_candidate_formats() {
        let handle = slime_create();
        assert!(!handle.is_null());
        // SAFETY: `handle` is live and exclusively accessed in this test.
        let options =
            unsafe { slime_set_options_v5(handle, false, false, false, 0, false, 1 << 5) };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };
        for character in "kyou".chars() {
            // SAFETY: `handle` is live and exclusively accessed.
            let buffer = unsafe { slime_process(handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: `buffer` is the original live buffer.
            unsafe { slime_buffer_destroy(buffer) };
        }
        // SAFETY: `handle` is live and exclusively accessed.
        let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: `conversion` remains live until the destroy call below.
        let json = unsafe { copy_buffer(&conversion) };
        assert!(json.contains("\"R") && json.contains('/'), "{json}");

        // SAFETY: Resources are live and each is destroyed exactly once.
        unsafe {
            slime_buffer_destroy(conversion);
            slime_destroy(handle);
        }
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
    fn installed_dictionary_packs_cross_the_c_boundary() {
        let directory =
            std::env::temp_dir().join(format!("slime-ffi-packs-{}", std::process::id()));
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        fs::write(
            pack_directory.join("sample.slime-dict"),
            "\
# slime-dictionary-pack-v1
# id: sample-pro
# name: サンプル Pro
# version: 2026.07.1
# license: Proprietary
すらいむぷろ\tSlime Pro
",
        )
        .unwrap();
        let path = directory.to_string_lossy();
        // SAFETY: `path` remains readable for the duration of the creation call.
        let handle = unsafe { slime_create_with_data_dir(path.as_ptr(), path.len()) };

        // SAFETY: `handle` is live and read serially.
        let catalog = unsafe { slime_installed_dictionary_packs(handle) };
        // SAFETY: `catalog` is live until the destroy call below.
        let json = unsafe { copy_buffer(&catalog) };
        assert!(json.contains("\"id\":\"sample-pro\""), "{json}");
        assert!(json.contains("\"formatVersion\":1"), "{json}");
        assert!(json.contains("\"provenance\":null"), "{json}");
        assert!(json.contains("\"entryCount\":1"), "{json}");
        // SAFETY: `catalog` has not previously been released.
        unsafe { slime_buffer_destroy(catalog) };

        let id = b"sample-pro";
        // SAFETY: `handle` and `id` are live and readable for this call.
        let words = unsafe { slime_installed_dictionary_pack_words(handle, id.as_ptr(), id.len()) };
        // SAFETY: `words` is live until the destroy call below.
        let json = unsafe { copy_buffer(&words) };
        assert!(json.contains("Slime Pro"), "{json}");
        // SAFETY: Resources are live and each is destroyed exactly once.
        unsafe {
            slime_buffer_destroy(words);
            slime_destroy(handle);
        }
        fs::remove_dir_all(directory).unwrap();
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
