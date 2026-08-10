//! C ABI for native platform adapters.
//!
//! The first version returns a compact JSON action list. This keeps Swift-side
//! integration simple while the action schema is still evolving.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(feature = "neural")]
use std::path::{Path, PathBuf};
use std::ptr;
#[cfg(feature = "neural")]
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use slime_core::{
    CandidateAnnotation, DictionaryPackTrust, DictionaryPackVerificationKey,
    DictionaryPackVersionFloor, EnginePreferences, InputEvent, SlimeAction, SlimeEngine, UserData,
};

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

pub const ACTION_UPDATE_PREEDIT: u32 = 0;
pub const ACTION_SHOW_CANDIDATES: u32 = 1;
pub const ACTION_HIDE_CANDIDATES: u32 = 2;
pub const ACTION_COMMIT: u32 = 3;
pub const ACTION_CLEAR: u32 = 4;
pub const ACTION_FORWARD_KEY: u32 = 5;

pub const CANDIDATE_ANNOTATION_NONE: u32 = CandidateAnnotation::None as u32;
pub const CANDIDATE_ANNOTATION_USER_DICTIONARY: u32 = CandidateAnnotation::UserDictionary as u32;
pub const CANDIDATE_ANNOTATION_HISTORY: u32 = CandidateAnnotation::History as u32;
pub const CANDIDATE_ANNOTATION_CORRECTION: u32 = CandidateAnnotation::Correction as u32;
pub const CANDIDATE_ANNOTATION_COMPLETION: u32 = CandidateAnnotation::Completion as u32;
pub const CANDIDATE_ANNOTATION_DATE_TIME: u32 = CandidateAnnotation::DateTime as u32;
pub const CANDIDATE_ANNOTATION_NUMBER: u32 = CandidateAnnotation::Number as u32;
pub const CANDIDATE_ANNOTATION_CONTEXT: u32 = CandidateAnnotation::Context as u32;

pub const STATUS_OK: u32 = 0;
pub const STATUS_NULL_HANDLE: u32 = 1;
pub const STATUS_INVALID_EVENT: u32 = 2;
pub const STATUS_NULL_CALLBACK: u32 = 3;
pub const STATUS_PANIC: u32 = 4;
pub const STATUS_INVALID_UTF8: u32 = 5;
pub const STATUS_INVALID_CANDIDATE: u32 = 6;
pub const STATUS_NEURAL_UNAVAILABLE: u32 = 7;
pub const STATUS_NEURAL_LOAD_FAILED: u32 = 8;
pub const STATUS_NEURAL_MODEL_CONFLICT: u32 = 9;
pub const STATUS_NEURAL_PREPARING: u32 = 10;
pub const STATUS_INVALID_NEURAL_PROFILE: u32 = 11;

pub const NEURAL_PROFILE_BALANCED: u32 = 0;
pub const NEURAL_PROFILE_HIGH_ACCURACY: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SlimeStringView {
    pub data: *const u8,
    pub len: usize,
}

impl SlimeStringView {
    fn new(value: &str) -> Self {
        Self {
            data: value.as_ptr(),
            len: value.len(),
        }
    }

    const fn empty() -> Self {
        Self {
            data: ptr::null(),
            len: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct SlimeActionView {
    pub kind: u32,
    pub text: SlimeStringView,
    pub candidates: *const SlimeStringView,
    pub candidate_count: usize,
    pub selected: usize,
    pub selection_start: usize,
    pub selection_length: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SlimeCandidateViewV2 {
    pub value: SlimeStringView,
    pub display: SlimeStringView,
    pub annotation: u32,
    pub detail: SlimeStringView,
}

#[repr(C)]
#[derive(Debug)]
pub struct SlimeActionViewV2 {
    pub kind: u32,
    pub text: SlimeStringView,
    pub candidates: *const SlimeCandidateViewV2,
    pub candidate_count: usize,
    pub selected: usize,
    pub selection_start: usize,
    pub selection_length: usize,
}

pub type SlimeActionCallback = unsafe extern "C" fn(*mut c_void, *const SlimeActionView);
pub type SlimeActionCallbackV2 = unsafe extern "C" fn(*mut c_void, *const SlimeActionViewV2);
pub type SlimeStringCallback = unsafe extern "C" fn(*mut c_void, SlimeStringView);

pub struct SlimeHandle {
    engine: SlimeEngine,
    neural_enabled: bool,
    #[cfg(feature = "neural")]
    neural_candidate_weight: f64,
    #[cfg(feature = "neural")]
    neural_right_context_minimum_score_margin: f64,
}

#[cfg(feature = "neural")]
struct NeuralService {
    rescorer: slime_neural::Rescorer,
}

#[cfg(feature = "neural")]
static NEURAL_SERVICE: OnceLock<Mutex<Option<NeuralService>>> = OnceLock::new();
#[cfg(feature = "neural")]
static NEURAL_MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();
#[cfg(feature = "neural")]
static NEURAL_LOAD_FAILED: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "neural")]
const NEURAL_MAX_PARALLEL_CANDIDATES: usize = 16;
#[cfg(feature = "neural")]
static NEURAL_EXIT_HANDLER: OnceLock<()> = OnceLock::new();

#[cfg(feature = "neural")]
unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

fn new_handle(engine: SlimeEngine) -> SlimeHandle {
    SlimeHandle {
        engine,
        neural_enabled: false,
        #[cfg(feature = "neural")]
        neural_candidate_weight: 0.7,
        #[cfg(feature = "neural")]
        neural_right_context_minimum_score_margin: 0.1,
    }
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
    match catch_unwind(|| new_handle(SlimeEngine::bundled())) {
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
        Some(new_handle(SlimeEngine::bundled_with_user_data(
            UserData::load(path),
        )))
    }));

    match result {
        Ok(Some(handle)) => Box::into_raw(Box::new(handle)),
        Ok(None) | Err(_) => ptr::null_mut(),
    }
}

/// Creates an engine that rejects every installed dictionary pack without a
/// valid signature from one of the supplied Ed25519 public keys.
///
/// `verification_keys` is UTF-8 with one
/// `lowercase-key-id<TAB>64-lowercase-hex-public-key` row per trusted key.
///
/// # Safety
///
/// Both pointers must reference readable byte slices of their corresponding
/// lengths for this call. A null pointer is accepted only when its length is
/// zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_create_with_signed_data_dir(
    data_dir: *const u8,
    data_dir_len: usize,
    verification_keys: *const u8,
    verification_keys_len: usize,
) -> *mut SlimeHandle {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let data_dir = unsafe { utf8_from_raw_parts(data_dir, data_dir_len) }?;
        let verification_keys =
            unsafe { utf8_from_raw_parts(verification_keys, verification_keys_len) }?;
        let trust = parse_dictionary_pack_trust(verification_keys)?;
        Some(new_handle(
            SlimeEngine::bundled_with_user_data_and_pack_trust(UserData::load(data_dir), trust),
        ))
    }));

    match result {
        Ok(Some(handle)) => Box::into_raw(Box::new(handle)),
        Ok(None) | Err(_) => ptr::null_mut(),
    }
}

/// Creates a signed-pack engine that also rejects configured pack IDs below
/// their minimum accepted versions.
///
/// `version_floors` is UTF-8 with one
/// `lowercase-pack-id<TAB>MAJOR.MINOR.PATCH` row per protected pack.
///
/// # Safety
///
/// All pointers must reference readable byte slices of their corresponding
/// lengths for this call. A null pointer is accepted only when its length is
/// zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_create_with_signed_data_dir_and_version_floors(
    data_dir: *const u8,
    data_dir_len: usize,
    verification_keys: *const u8,
    verification_keys_len: usize,
    version_floors: *const u8,
    version_floors_len: usize,
) -> *mut SlimeHandle {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let data_dir = unsafe { utf8_from_raw_parts(data_dir, data_dir_len) }?;
        let verification_keys =
            unsafe { utf8_from_raw_parts(verification_keys, verification_keys_len) }?;
        let version_floors = unsafe { utf8_from_raw_parts(version_floors, version_floors_len) }?;
        let keys = parse_dictionary_pack_verification_keys(verification_keys)?;
        let floors = parse_dictionary_pack_version_floors(version_floors)?;
        let trust = DictionaryPackTrust::signed_only_with_version_floors(keys, floors).ok()?;
        Some(new_handle(
            SlimeEngine::bundled_with_user_data_and_pack_trust(UserData::load(data_dir), trust),
        ))
    }));

    match result {
        Ok(Some(handle)) => Box::into_raw(Box::new(handle)),
        Ok(None) | Err(_) => ptr::null_mut(),
    }
}

unsafe fn utf8_from_raw_parts<'a>(data: *const u8, len: usize) -> Option<&'a str> {
    if data.is_null() && len != 0 {
        return None;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        // SAFETY: The caller promises a readable byte slice for this call.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    std::str::from_utf8(bytes).ok()
}

fn parse_dictionary_pack_trust(source: &str) -> Option<DictionaryPackTrust> {
    DictionaryPackTrust::signed_only(parse_dictionary_pack_verification_keys(source)?).ok()
}

fn parse_dictionary_pack_verification_keys(
    source: &str,
) -> Option<Vec<DictionaryPackVerificationKey>> {
    let mut keys = Vec::new();
    for line in source.lines() {
        let (id, encoded_key) = line.split_once('\t')?;
        if encoded_key.contains('\t') {
            return None;
        }
        keys.push(DictionaryPackVerificationKey::from_lower_hex(id, encoded_key).ok()?);
    }
    Some(keys)
}

fn parse_dictionary_pack_version_floors(source: &str) -> Option<Vec<DictionaryPackVersionFloor>> {
    if source.is_empty() {
        return None;
    }
    let mut floors = Vec::new();
    for line in source.lines() {
        let (id, minimum_version) = line.split_once('\t')?;
        if minimum_version.contains('\t') {
            return None;
        }
        floors.push(DictionaryPackVersionFloor::new(id, minimum_version).ok()?);
    }
    Some(floors)
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

/// Enables process-wide neural candidate rescoring for this handle.
///
/// The first successful call starts loading one model shared by every engine
/// in the input-method process. Loading happens off the caller's thread; base
/// ranking remains active until the service is ready. Later calls must name
/// the same path. Default builds return [`STATUS_NEURAL_UNAVAILABLE`].
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed IME handle. `model_path`
/// must reference readable UTF-8 bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_enable_neural_rescoring(
    handle: *mut SlimeHandle,
    model_path: *const u8,
    model_path_len: usize,
) -> u32 {
    // SAFETY: This function forwards the caller's pointer contract unchanged.
    unsafe {
        slime_enable_neural_rescoring_with_profile(
            handle,
            model_path,
            model_path_len,
            NEURAL_PROFILE_BALANCED,
        )
    }
}

/// Enables neural rescoring with a measured model-size profile.
///
/// `balanced` preserves the xsmall model's established interpolation. The
/// `high-accuracy` profile uses the settings frozen for zenz-v3.2-small.
/// Keeping the choice explicit avoids guessing from a file name or size.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed IME handle. `model_path`
/// must reference readable UTF-8 bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_enable_neural_rescoring_with_profile(
    handle: *mut SlimeHandle,
    model_path: *const u8,
    model_path_len: usize,
    profile: u32,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        let Some(profile_parameters) = neural_profile_parameters(profile) else {
            return STATUS_INVALID_NEURAL_PROFILE;
        };
        // SAFETY: The caller promises readable bytes for this call.
        let Some(model_path) = (unsafe { utf8_from_raw_parts(model_path, model_path_len) }) else {
            return STATUS_INVALID_UTF8;
        };
        if model_path.is_empty() {
            return STATUS_INVALID_UTF8;
        }

        #[cfg(not(feature = "neural"))]
        {
            let _ = (model_path, profile_parameters);
            STATUS_NEURAL_UNAVAILABLE
        }

        #[cfg(feature = "neural")]
        {
            let (candidate_weight, right_context_minimum_score_margin) = profile_parameters;
            let model_path = Path::new(model_path);
            let status = prepare_neural_service(model_path);
            if status == STATUS_OK {
                // SAFETY: The caller promises a live, exclusively accessed handle.
                let handle = unsafe { &mut *handle };
                handle.neural_enabled = true;
                handle.neural_candidate_weight = candidate_weight;
                handle.neural_right_context_minimum_score_margin =
                    right_context_minimum_score_margin;
            }
            status
        }
    }));
    result.unwrap_or(STATUS_PANIC)
}

const fn neural_profile_parameters(profile: u32) -> Option<(f64, f64)> {
    match profile {
        NEURAL_PROFILE_BALANCED => Some((0.7, 0.1)),
        NEURAL_PROFILE_HIGH_ACCURACY => Some((0.8, 0.5)),
        _ => None,
    }
}

#[cfg(feature = "neural")]
fn prepare_neural_service(model_path: &Path) -> u32 {
    if let Some(configured_path) = NEURAL_MODEL_PATH.get() {
        return if configured_path == model_path {
            STATUS_OK
        } else {
            STATUS_NEURAL_MODEL_CONFLICT
        };
    }
    let owned_path = model_path.to_path_buf();
    if NEURAL_MODEL_PATH.set(owned_path.clone()).is_err() {
        return NEURAL_MODEL_PATH
            .get()
            .map_or(STATUS_NEURAL_LOAD_FAILED, |configured_path| {
                if configured_path == model_path {
                    STATUS_OK
                } else {
                    STATUS_NEURAL_MODEL_CONFLICT
                }
            });
    }
    let spawn = std::thread::Builder::new()
        .name("slime-neural-loader".to_owned())
        .spawn(move || load_neural_service(&owned_path));
    if spawn.is_err() {
        NEURAL_LOAD_FAILED.store(true, Ordering::Release);
        return STATUS_NEURAL_LOAD_FAILED;
    }
    STATUS_OK
}

#[cfg(feature = "neural")]
fn load_neural_service(model_path: &Path) {
    let Ok(rescorer) =
        slime_neural::Rescorer::load_bounded(model_path, NEURAL_MAX_PARALLEL_CANDIDATES, 1_024)
    else {
        NEURAL_LOAD_FAILED.store(true, Ordering::Release);
        return;
    };
    let service = NeuralService { rescorer };
    if !register_neural_exit_handler() {
        NEURAL_LOAD_FAILED.store(true, Ordering::Release);
        return;
    }
    let slot = NEURAL_SERVICE.get_or_init(|| Mutex::new(None));
    let Ok(mut slot) = slot.lock() else {
        NEURAL_LOAD_FAILED.store(true, Ordering::Release);
        return;
    };
    assert!(
        slot.replace(service).is_none(),
        "neural initialization lock permits only one writer"
    );
}

#[cfg(feature = "neural")]
fn register_neural_exit_handler() -> bool {
    if NEURAL_EXIT_HANDLER.get().is_some() {
        return true;
    }
    // llama.cpp registers its process-wide backend destructors while loading
    // the model. C exit handlers run in reverse registration order, so this
    // releases the model and backend before Metal destroys its global device.
    // SAFETY: `shutdown_neural_service` has C linkage, takes no arguments, and
    // remains valid for the lifetime of the process.
    if unsafe { atexit(shutdown_neural_service) } != 0 {
        return false;
    }
    NEURAL_EXIT_HANDLER.set(()).is_ok()
}

#[cfg(feature = "neural")]
extern "C" fn shutdown_neural_service() {
    let service = NEURAL_SERVICE.get().and_then(|slot| {
        let mut slot = slot.lock().ok()?;
        slot.take()
    });
    // Drop outside the mutex. `Rescorer` orders its fields so the model is
    // released before the process-wide llama backend.
    drop(service);
}

/// Reports whether the optional neural service is unavailable, preparing,
/// ready, or failed. The model path itself is never exposed.
#[unsafe(no_mangle)]
pub extern "C" fn slime_neural_rescoring_status() -> u32 {
    #[cfg(not(feature = "neural"))]
    {
        STATUS_NEURAL_UNAVAILABLE
    }
    #[cfg(feature = "neural")]
    {
        if NEURAL_SERVICE
            .get()
            .is_some_and(|slot| slot.lock().is_ok_and(|service| service.is_some()))
        {
            STATUS_OK
        } else if NEURAL_LOAD_FAILED.load(Ordering::Acquire) {
            STATUS_NEURAL_LOAD_FAILED
        } else if NEURAL_MODEL_PATH.get().is_some() {
            STATUS_NEURAL_PREPARING
        } else {
            STATUS_NEURAL_UNAVAILABLE
        }
    }
}

fn process_event(handle: &mut SlimeHandle, event: InputEvent) -> Vec<SlimeAction> {
    let actions = handle.engine.handle(event);
    if !handle.neural_enabled {
        return actions;
    }

    #[cfg(feature = "neural")]
    if let Some(service) = NEURAL_SERVICE.get() {
        handle.engine.prepare_extended_candidate_rescore();
        let Some(request) = handle.engine.candidate_rescore_request() else {
            return actions;
        };
        let Ok(service) = service.lock() else {
            return actions;
        };
        let Some(service) = service.as_ref() else {
            return actions;
        };
        let minimum_margin = if request.right_context.is_empty() {
            0.0
        } else {
            handle.neural_right_context_minimum_score_margin
        };
        let score_request = slime_neural::ScoreRequest {
            context: request.context,
            right_context: request.right_context,
            input_katakana: full_katakana(&request.reading),
            candidates: request.candidates,
        };
        if let Ok(scored) = service.rescorer.score_all(&[score_request])
            && let Some(scored) = scored.first()
            && let Some(rescored_actions) = handle.engine.apply_candidate_rescore(
                &scored.candidate_logliks,
                handle.neural_candidate_weight,
                minimum_margin,
            )
        {
            return rescored_actions;
        }
    }
    actions
}

#[cfg(feature = "neural")]
fn full_katakana(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\u{3041}'..='\u{3096}' | '\u{309d}'..='\u{309e}' => {
                char::from_u32(u32::from(character) + 0x60).expect("hiragana has katakana pair")
            }
            _ => character,
        })
        .collect()
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
        let actions = process_event(handle, event);
        success_response(&actions)
    }));

    SlimeBuffer::from_string(match result {
        Ok(response) => response,
        Err(_) => error_response("panic"),
    })
}

/// Processes one event and synchronously visits typed action views.
///
/// The action, text, and candidate pointers are borrowed and valid only for the
/// duration of each callback. This avoids JSON encoding and parsing in native
/// platform hot paths while keeping [`slime_process`] backward compatible.
///
/// # Safety
///
/// `handle` must be null or a live, exclusively accessed pointer returned by an
/// IME creation function. `callback` must not unwind, retain borrowed views, or
/// re-enter an API with the same handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_process_actions(
    handle: *mut SlimeHandle,
    event_kind: u32,
    value: u32,
    context: *mut c_void,
    callback: Option<SlimeActionCallback>,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        let Some(callback) = callback else {
            return STATUS_NULL_CALLBACK;
        };
        let Ok(event) = decode_event(event_kind, value) else {
            return STATUS_INVALID_EVENT;
        };

        // SAFETY: The caller promises a live, exclusively accessed handle.
        let actions = process_event(unsafe { &mut *handle }, event);
        for action in &actions {
            visit_action(action, context, callback);
        }
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Processes one event and visits typed actions with candidate metadata.
///
/// The v1 callback remains available for adapters that only need display
/// strings. v2 separates each candidate's committed value, legacy display,
/// semantic annotation, and optional detail so native UIs can localize labels
/// without parsing or modifying the committed text.
///
/// # Safety
///
/// The same borrowing and re-entry rules as [`slime_process_actions`] apply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_process_actions_v2(
    handle: *mut SlimeHandle,
    event_kind: u32,
    value: u32,
    context: *mut c_void,
    callback: Option<SlimeActionCallbackV2>,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        let Some(callback) = callback else {
            return STATUS_NULL_CALLBACK;
        };
        let Ok(event) = decode_event(event_kind, value) else {
            return STATUS_INVALID_EVENT;
        };

        // SAFETY: The caller promises a live, exclusively accessed handle.
        let actions = process_event(unsafe { &mut *handle }, event);
        for action in &actions {
            visit_action_v2(action, context, callback);
        }
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Enumerates conversion candidates without mutating the active composition.
///
/// # Safety
///
/// `handle` must be null or a live pointer returned by an IME creation
/// function. `reading` must point to `reading_len` readable UTF-8 bytes.
/// `callback` is invoked synchronously and must not retain the borrowed view,
/// unwind, or re-enter this handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_conversion_candidates(
    handle: *const SlimeHandle,
    reading: *const u8,
    reading_len: usize,
    context: *mut c_void,
    callback: Option<SlimeStringCallback>,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        let Some(callback) = callback else {
            return STATUS_NULL_CALLBACK;
        };
        // SAFETY: The caller promises readable bytes for this call.
        let Some(reading) = (unsafe { decode_utf8_argument(reading, reading_len) }) else {
            return STATUS_INVALID_UTF8;
        };
        // SAFETY: The caller promises a live handle. This operation only reads
        // engine data and cannot affect the active composition.
        let candidates = unsafe { &(*handle).engine }.conversion_candidates(reading);
        for candidate in &candidates {
            // SAFETY: The callback contract requires synchronous use only.
            unsafe { callback(context, SlimeStringView::new(candidate)) };
        }
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Records a candidate chosen by an external conversion consumer.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function. Both byte ranges must be readable UTF-8 for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_record_external_selection(
    handle: *mut SlimeHandle,
    reading: *const u8,
    reading_len: usize,
    surface: *const u8,
    surface_len: usize,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        // SAFETY: The caller promises readable bytes for this call.
        let Some(reading) = (unsafe { decode_utf8_argument(reading, reading_len) }) else {
            return STATUS_INVALID_UTF8;
        };
        // SAFETY: The caller promises readable bytes for this call.
        let Some(surface) = (unsafe { decode_utf8_argument(surface, surface_len) }) else {
            return STATUS_INVALID_UTF8;
        };
        // SAFETY: The caller promises a live, exclusively accessed handle.
        if unsafe { &mut (*handle).engine }.record_external_selection(reading, surface) {
            STATUS_OK
        } else {
            STATUS_INVALID_CANDIDATE
        }
    }));
    result.unwrap_or(STATUS_PANIC)
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

/// Breaks transient document context after an external caret, document, or input
/// client boundary without deleting persisted history.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_reset_context(handle: *mut SlimeHandle) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        // SAFETY: The caller promises a live, exclusively accessed handle.
        unsafe { &mut (*handle).engine }.reset_context();
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Supplies bounded committed text immediately before the platform caret.
///
/// The context is transient, is not persisted, and cannot create a learned
/// contextual-history edge because the platform does not know its reading.
/// Private mode discards it.
///
/// # Safety
///
/// `handle` must be live and exclusively accessed. `surface` must point to
/// `surface_len` readable bytes for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_external_left_context(
    handle: *mut SlimeHandle,
    surface: *const u8,
    surface_len: usize,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        // SAFETY: The caller promises readable bytes for the duration of the call.
        let Some(surface) = (unsafe { decode_utf8_argument(surface, surface_len) }) else {
            return STATUS_INVALID_UTF8;
        };
        // SAFETY: This function's contract requires a live, exclusive handle.
        unsafe { &mut (*handle).engine }.set_external_left_context(surface);
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Supplies bounded committed text on both sides of the platform caret.
///
/// The context is transient, never persisted, and ignored in private mode.
/// Left context also participates in deterministic lattice ranking; right
/// context is reserved for optional local candidate scoring.
///
/// # Safety
///
/// `handle` must be live and exclusively accessed. Each non-null pointer must
/// address its declared readable byte length for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_external_context(
    handle: *mut SlimeHandle,
    left_surface: *const u8,
    left_surface_len: usize,
    right_surface: *const u8,
    right_surface_len: usize,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        // SAFETY: The caller promises readable bytes for the duration of the call.
        let Some(left_surface) = (unsafe { decode_utf8_argument(left_surface, left_surface_len) })
        else {
            return STATUS_INVALID_UTF8;
        };
        // SAFETY: The caller promises readable bytes for the duration of the call.
        let Some(right_surface) =
            (unsafe { decode_utf8_argument(right_surface, right_surface_len) })
        else {
            return STATUS_INVALID_UTF8;
        };
        // SAFETY: This function's contract requires a live, exclusive handle.
        unsafe { &mut (*handle).engine }.set_external_context(left_surface, right_surface);
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
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
            output.push_str(",\"payloadSHA256\":");
            write_optional_json_string(&mut output, pack.payload_sha256.as_deref());
            output.push_str(",\"packSHA256\":");
            write_json_string(&mut output, &pack.pack_sha256);
            write!(
                output,
                ",\"entryCount\":{},\"contextRuleCount\":{}}}",
                pack.entry_count, pack.context_rule_count
            )
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

fn visit_action(action: &SlimeAction, context: *mut c_void, callback: SlimeActionCallback) {
    let mut candidate_views = Vec::new();
    let view = match action {
        SlimeAction::UpdatePreedit(text) => SlimeActionView {
            kind: ACTION_UPDATE_PREEDIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: usize::MAX,
            selection_start: usize::MAX,
            selection_length: 0,
        },
        SlimeAction::UpdateSegmentedPreedit {
            text,
            selection_start,
            selection_length,
        } => SlimeActionView {
            kind: ACTION_UPDATE_PREEDIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: usize::MAX,
            selection_start: *selection_start,
            selection_length: *selection_length,
        },
        SlimeAction::ShowCandidates {
            candidates,
            selected,
            ..
        } => {
            candidate_views.extend(
                candidates
                    .iter()
                    .map(|candidate| SlimeStringView::new(candidate)),
            );
            SlimeActionView {
                kind: ACTION_SHOW_CANDIDATES,
                text: SlimeStringView::empty(),
                candidates: candidate_views.as_ptr(),
                candidate_count: candidate_views.len(),
                selected: *selected,
                selection_start: usize::MAX,
                selection_length: 0,
            }
        }
        SlimeAction::HideCandidates => action_without_payload(ACTION_HIDE_CANDIDATES),
        SlimeAction::Commit(text) => SlimeActionView {
            kind: ACTION_COMMIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: usize::MAX,
            selection_start: usize::MAX,
            selection_length: 0,
        },
        SlimeAction::Clear => action_without_payload(ACTION_CLEAR),
        SlimeAction::ForwardKey => action_without_payload(ACTION_FORWARD_KEY),
    };

    // SAFETY: The public function contract requires a callback that remains
    // valid for the call and does not retain the borrowed view.
    unsafe { callback(context, &raw const view) };
}

fn visit_action_v2(action: &SlimeAction, context: *mut c_void, callback: SlimeActionCallbackV2) {
    let mut candidate_views = Vec::new();
    let view = match action {
        SlimeAction::UpdatePreedit(text) => SlimeActionViewV2 {
            kind: ACTION_UPDATE_PREEDIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: usize::MAX,
            selection_start: usize::MAX,
            selection_length: 0,
        },
        SlimeAction::UpdateSegmentedPreedit {
            text,
            selection_start,
            selection_length,
        } => SlimeActionViewV2 {
            kind: ACTION_UPDATE_PREEDIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: usize::MAX,
            selection_start: *selection_start,
            selection_length: *selection_length,
        },
        SlimeAction::ShowCandidates {
            candidates,
            details,
            selected,
        } => {
            debug_assert_eq!(candidates.len(), details.len());
            candidate_views.extend(candidates.iter().zip(details).map(|(display, detail)| {
                SlimeCandidateViewV2 {
                    value: SlimeStringView::new(&detail.value),
                    display: SlimeStringView::new(display),
                    annotation: detail.annotation as u32,
                    detail: detail
                        .detail
                        .as_deref()
                        .map_or_else(SlimeStringView::empty, SlimeStringView::new),
                }
            }));
            SlimeActionViewV2 {
                kind: ACTION_SHOW_CANDIDATES,
                text: SlimeStringView::empty(),
                candidates: candidate_views.as_ptr(),
                candidate_count: candidate_views.len(),
                selected: *selected,
                selection_start: usize::MAX,
                selection_length: 0,
            }
        }
        SlimeAction::HideCandidates => action_without_payload_v2(ACTION_HIDE_CANDIDATES),
        SlimeAction::Commit(text) => SlimeActionViewV2 {
            kind: ACTION_COMMIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: usize::MAX,
            selection_start: usize::MAX,
            selection_length: 0,
        },
        SlimeAction::Clear => action_without_payload_v2(ACTION_CLEAR),
        SlimeAction::ForwardKey => action_without_payload_v2(ACTION_FORWARD_KEY),
    };

    // SAFETY: The public function contract requires a callback that remains
    // valid for the call and does not retain the borrowed view.
    unsafe { callback(context, &raw const view) };
}

const fn action_without_payload(kind: u32) -> SlimeActionView {
    SlimeActionView {
        kind,
        text: SlimeStringView::empty(),
        candidates: ptr::null(),
        candidate_count: 0,
        selected: usize::MAX,
        selection_start: usize::MAX,
        selection_length: 0,
    }
}

const fn action_without_payload_v2(kind: u32) -> SlimeActionViewV2 {
    SlimeActionViewV2 {
        kind,
        text: SlimeStringView::empty(),
        candidates: ptr::null(),
        candidate_count: 0,
        selected: usize::MAX,
        selection_start: usize::MAX,
        selection_length: 0,
    }
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
            details,
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
            output.push_str("],\"candidateDetails\":[");
            for (index, detail) in details.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str("{\"value\":");
                write_json_string(output, &detail.value);
                output.push_str(",\"annotation\":");
                write!(output, "{}", detail.annotation as u32)
                    .expect("writing to String cannot fail");
                output.push_str(",\"detail\":");
                write_optional_json_string(output, detail.detail.as_deref());
                output.push('}');
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
    #[cfg(not(feature = "neural"))]
    use super::slime_enable_neural_rescoring;
    use super::{
        ACTION_COMMIT, ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT,
        CANDIDATE_ANNOTATION_CORRECTION, CANDIDATE_ANNOTATION_NUMBER, EVENT_ACCEPT_CANDIDATE,
        EVENT_CHARACTER, EVENT_ENTER, EVENT_NEXT_CANDIDATE, EVENT_PREVIOUS_SEGMENT,
        EVENT_SELECT_CANDIDATE, EVENT_SPACE, STATUS_INVALID_CANDIDATE, STATUS_INVALID_UTF8,
        STATUS_NULL_HANDLE, STATUS_OK, SlimeActionView, SlimeActionViewV2, SlimeBuffer,
        SlimeStringView, slime_begin_reconversion, slime_buffer_destroy,
        slime_conversion_candidates, slime_create, slime_create_with_data_dir,
        slime_create_with_signed_data_dir, slime_create_with_signed_data_dir_and_version_floors,
        slime_destroy, slime_domain_dictionary_words, slime_enable_neural_rescoring_with_profile,
        slime_installed_dictionary_pack_words, slime_installed_dictionary_packs,
        slime_neural_rescoring_status, slime_process, slime_process_actions,
        slime_process_actions_v2, slime_record_external_selection, slime_reset_context,
        slime_set_external_context, slime_set_external_left_context, slime_set_options,
        slime_set_options_v2, slime_set_options_v3, slime_set_options_v4, slime_set_options_v5,
    };
    use std::ffi::c_void;
    use std::fs;

    #[test]
    fn neural_profiles_keep_measured_parameters_explicit() {
        assert_eq!(
            super::neural_profile_parameters(super::NEURAL_PROFILE_BALANCED),
            Some((0.7, 0.1))
        );
        assert_eq!(
            super::neural_profile_parameters(super::NEURAL_PROFILE_HIGH_ACCURACY),
            Some((0.8, 0.5))
        );
        assert_eq!(super::neural_profile_parameters(u32::MAX), None);
    }

    #[test]
    fn invalid_neural_profile_is_rejected_before_loading() {
        let handle = slime_create();
        let path = b"model.gguf";
        // SAFETY: The handle and path are live for this synchronous call.
        let status = unsafe {
            slime_enable_neural_rescoring_with_profile(handle, path.as_ptr(), path.len(), u32::MAX)
        };
        assert_eq!(status, super::STATUS_INVALID_NEURAL_PROFILE);
        // SAFETY: The handle is released exactly once.
        unsafe { slime_destroy(handle) };
    }

    unsafe fn copy_buffer(buffer: &SlimeBuffer) -> String {
        // SAFETY: Tests read a live buffer before handing it back to its destructor.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) };
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[cfg(not(feature = "neural"))]
    #[test]
    fn default_build_reports_neural_rescoring_as_unavailable() {
        let handle = slime_create();
        let path = b"model.gguf";
        // SAFETY: The handle and path are live for this synchronous call.
        let status = unsafe { slime_enable_neural_rescoring(handle, path.as_ptr(), path.len()) };
        assert_eq!(status, super::STATUS_NEURAL_UNAVAILABLE);
        assert_eq!(
            slime_neural_rescoring_status(),
            super::STATUS_NEURAL_UNAVAILABLE
        );
        // SAFETY: The handle is released exactly once.
        unsafe { slime_destroy(handle) };
    }

    #[cfg(feature = "neural")]
    unsafe fn enable_high_accuracy_neural(handle: *mut super::SlimeHandle, model: &str) -> u32 {
        // SAFETY: The caller passes a live handle and model string for this synchronous call.
        unsafe {
            slime_enable_neural_rescoring_with_profile(
                handle,
                model.as_ptr(),
                model.len(),
                super::NEURAL_PROFILE_HIGH_ACCURACY,
            )
        }
    }

    #[cfg(feature = "neural")]
    #[test]
    #[ignore = "requires SLIME_NEURAL_TEST_MODEL"]
    fn neural_feature_loads_and_processes_an_explicit_conversion() {
        let model = std::env::var("SLIME_NEURAL_TEST_MODEL")
            .expect("SLIME_NEURAL_TEST_MODEL must name a compatible GGUF model");
        let handle = slime_create();
        // SAFETY: The handle and model path are live for this synchronous call.
        let status = unsafe { enable_high_accuracy_neural(handle, &model) };
        assert_eq!(status, STATUS_OK);
        // A cold llama.cpp/Metal initialization can include shader compilation
        // and exceed 30 seconds on an otherwise busy development machine.
        let deadline = std::time::Instant::now() + std::time::Duration::from_mins(2);
        while slime_neural_rescoring_status() == super::STATUS_NEURAL_PREPARING {
            assert!(
                std::time::Instant::now() < deadline,
                "neural model did not finish loading"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(slime_neural_rescoring_status(), STATUS_OK);
        let expanded_candidate_latency = {
            let service = super::NEURAL_SERVICE
                .get()
                .expect("neural service should be initialized")
                .lock()
                .expect("neural service lock should not be poisoned");
            let service = service.as_ref().expect("neural service should be loaded");
            let request = slime_neural::ScoreRequest {
                context: String::new(),
                right_context: String::new(),
                input_katakana: "チョウブンショウニ".to_owned(),
                candidates: (0..super::NEURAL_MAX_PARALLEL_CANDIDATES)
                    .map(|index| format!("長文候補{index}"))
                    .collect(),
            };
            let scored = service
                .rescorer
                .score_all(&[request])
                .expect("the product runtime bound should score the expanded pool");
            assert_eq!(
                scored[0].candidate_logliks.len(),
                super::NEURAL_MAX_PARALLEL_CANDIDATES
            );
            scored[0].latency
        };
        eprintln!(
            "ffi neural expanded-candidate conversion: {:.3}ms",
            expanded_candidate_latency.as_secs_f64() * 1_000.0
        );
        let left_context = "文章の途中で";
        let right_context = "を編集する";
        // SAFETY: The handle and both UTF-8 strings are live for this call.
        assert_eq!(
            unsafe {
                slime_set_external_context(
                    handle,
                    left_context.as_ptr(),
                    left_context.len(),
                    right_context.as_ptr(),
                    right_context.len(),
                )
            },
            STATUS_OK
        );
        for character in "koutei".chars() {
            // SAFETY: The handle is live and each buffer is released once.
            unsafe {
                slime_buffer_destroy(slime_process(handle, EVENT_CHARACTER, character.into()));
            };
        }
        // SAFETY: The handle is live and each buffer is released once.
        let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
        // SAFETY: The conversion buffer remains live here.
        let json = unsafe { copy_buffer(&conversion) };
        assert!(json.contains("show_candidates"), "{json}");
        assert!(json.contains("\"candidates\":["), "{json}");
        unsafe {
            slime_buffer_destroy(conversion);
            slime_buffer_destroy(slime_process(handle, EVENT_ENTER, 0));
        }

        let mut latencies = Vec::with_capacity(50);
        for _ in 0..50 {
            for character in "koutei".chars() {
                // SAFETY: The handle is live and each buffer is released once.
                unsafe {
                    slime_buffer_destroy(slime_process(handle, EVENT_CHARACTER, character.into()));
                };
            }
            let started = std::time::Instant::now();
            // SAFETY: The handle is live and each buffer is released once.
            let conversion = unsafe { slime_process(handle, EVENT_SPACE, 0) };
            latencies.push(started.elapsed());
            // SAFETY: The handle is live and each buffer is released once.
            unsafe {
                slime_buffer_destroy(conversion);
                slime_buffer_destroy(slime_process(handle, EVENT_ENTER, 0));
            }
        }
        latencies.sort_unstable();
        eprintln!(
            "ffi neural explicit conversion: p50={:.3}ms p95={:.3}ms max={:.3}ms",
            latencies[25].as_secs_f64() * 1_000.0,
            latencies[47].as_secs_f64() * 1_000.0,
            latencies[49].as_secs_f64() * 1_000.0
        );
        // SAFETY: The handle is released exactly once.
        unsafe {
            slime_destroy(handle);
        }
    }

    #[derive(Default)]
    struct TypedCapture {
        last_preedit: String,
        last_commit: String,
        candidate_count: usize,
        candidates: Vec<String>,
        selected: usize,
    }

    #[derive(Default)]
    struct TypedCaptureV2 {
        values: Vec<String>,
        displays: Vec<String>,
        annotations: Vec<u32>,
        details: Vec<Option<String>>,
    }

    unsafe fn copy_view(value: SlimeStringView) -> String {
        // SAFETY: The caller passes a borrowed callback view containing UTF-8.
        let bytes = unsafe { std::slice::from_raw_parts(value.data, value.len) };
        std::str::from_utf8(bytes).unwrap().to_owned()
    }

    unsafe extern "C" fn collect_typed_action_v2(
        context: *mut c_void,
        action: *const SlimeActionViewV2,
    ) {
        // SAFETY: The test passes live pointers for the synchronous callback.
        let capture = unsafe { &mut *context.cast::<TypedCaptureV2>() };
        // SAFETY: The callback contract supplies a live action view.
        let action = unsafe { &*action };
        if action.kind != ACTION_SHOW_CANDIDATES {
            return;
        }
        capture.values.clear();
        capture.displays.clear();
        capture.annotations.clear();
        capture.details.clear();
        for index in 0..action.candidate_count {
            // SAFETY: The action exposes `candidate_count` borrowed views.
            let candidate = unsafe { &*action.candidates.add(index) };
            // SAFETY: Candidate value and display are valid UTF-8 callback views.
            capture.values.push(unsafe { copy_view(candidate.value) });
            // SAFETY: Candidate value and display are valid UTF-8 callback views.
            capture
                .displays
                .push(unsafe { copy_view(candidate.display) });
            capture.annotations.push(candidate.annotation);
            capture.details.push((candidate.detail.len > 0).then(|| {
                // SAFETY: A non-empty detail is a valid UTF-8 callback view.
                unsafe { copy_view(candidate.detail) }
            }));
        }
    }

    unsafe extern "C" fn collect_typed_action(
        context: *mut c_void,
        action: *const SlimeActionView,
    ) {
        // SAFETY: The test passes live pointers for the synchronous callback.
        let capture = unsafe { &mut *context.cast::<TypedCapture>() };
        // SAFETY: The callback contract supplies a live action view.
        let action = unsafe { &*action };
        if action.kind == ACTION_UPDATE_PREEDIT {
            // SAFETY: Text is borrowed for the callback and contains UTF-8.
            let bytes = unsafe { std::slice::from_raw_parts(action.text.data, action.text.len) };
            capture.last_preedit = std::str::from_utf8(bytes).unwrap().to_owned();
        } else if action.kind == ACTION_COMMIT {
            // SAFETY: Text is borrowed for the callback and contains UTF-8.
            let bytes = unsafe { std::slice::from_raw_parts(action.text.data, action.text.len) };
            capture.last_commit = std::str::from_utf8(bytes).unwrap().to_owned();
        } else if action.kind == ACTION_SHOW_CANDIDATES {
            capture.candidate_count = action.candidate_count;
            capture.selected = action.selected;
            capture.candidates.clear();
            for index in 0..action.candidate_count {
                // SAFETY: The action view exposes `candidate_count` live views
                // for the duration of this callback.
                let candidate = unsafe { &*action.candidates.add(index) };
                // SAFETY: Candidate text is valid borrowed UTF-8.
                let bytes = unsafe { std::slice::from_raw_parts(candidate.data, candidate.len) };
                capture
                    .candidates
                    .push(std::str::from_utf8(bytes).unwrap().to_owned());
            }
        }
    }

    unsafe extern "C" fn collect_string(context: *mut c_void, value: SlimeStringView) {
        // SAFETY: The test passes a live vector and the callback view is valid
        // for this synchronous invocation.
        let candidates = unsafe { &mut *context.cast::<Vec<String>>() };
        // SAFETY: The callback contract guarantees readable UTF-8 bytes.
        let bytes = unsafe { std::slice::from_raw_parts(value.data, value.len) };
        candidates.push(std::str::from_utf8(bytes).unwrap().to_owned());
    }

    fn process_typed_event(
        handle: *mut super::SlimeHandle,
        event: u32,
        value: u32,
        capture: &mut TypedCapture,
    ) {
        // SAFETY: The handle and capture remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    event,
                    value,
                    std::ptr::from_mut::<TypedCapture>(capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
    }

    fn type_ascii(handle: *mut super::SlimeHandle, input: &str, capture: &mut TypedCapture) {
        for character in input.chars() {
            process_typed_event(handle, EVENT_CHARACTER, character.into(), capture);
        }
    }

    fn convert_and_accept_typed(
        handle: *mut super::SlimeHandle,
        input: &str,
        expected_surface: &str,
        capture: &mut TypedCapture,
    ) {
        capture.last_commit.clear();
        type_ascii(handle, input, capture);
        process_typed_event(handle, EVENT_SPACE, 0, capture);
        let index = capture
            .candidates
            .iter()
            .position(|candidate| candidate == expected_surface)
            .unwrap_or_else(|| {
                panic!(
                    "typed candidates for {input:?} should contain {expected_surface:?}: {:?}",
                    capture.candidates
                )
            });
        process_typed_event(
            handle,
            EVENT_SELECT_CANDIDATE,
            u32::try_from(index).unwrap(),
            capture,
        );
        assert_eq!(capture.last_preedit, expected_surface);
        process_typed_event(handle, EVENT_ACCEPT_CANDIDATE, 0, capture);
        assert_eq!(capture.last_commit, expected_surface);
    }

    #[test]
    fn read_only_candidates_and_external_selection_cross_the_c_boundary() {
        let handle = slime_create();
        assert!(!handle.is_null());
        let reading = "にほん";
        let mut candidates = Vec::<String>::new();
        // SAFETY: All pointers remain live for the synchronous call.
        let status = unsafe {
            slime_conversion_candidates(
                handle,
                reading.as_ptr(),
                reading.len(),
                (&raw mut candidates).cast(),
                Some(collect_string),
            )
        };
        assert_eq!(status, STATUS_OK);
        assert!(candidates.iter().any(|candidate| candidate == "日本"));

        let surface = "日本";
        // SAFETY: All pointers are live and the handle is exclusively accessed.
        assert_eq!(
            unsafe {
                slime_record_external_selection(
                    handle,
                    reading.as_ptr(),
                    reading.len(),
                    surface.as_ptr(),
                    surface.len(),
                )
            },
            STATUS_OK
        );
        let invalid = "無関係";
        // SAFETY: All pointers are live and the handle is exclusively accessed.
        assert_eq!(
            unsafe {
                slime_record_external_selection(
                    handle,
                    reading.as_ptr(),
                    reading.len(),
                    invalid.as_ptr(),
                    invalid.len(),
                )
            },
            STATUS_INVALID_CANDIDATE
        );
        let invalid_utf8 = [0xff];
        // SAFETY: The byte is readable but deliberately invalid UTF-8.
        assert_eq!(
            unsafe {
                slime_conversion_candidates(
                    handle,
                    invalid_utf8.as_ptr(),
                    invalid_utf8.len(),
                    (&raw mut candidates).cast(),
                    Some(collect_string),
                )
            },
            STATUS_INVALID_UTF8
        );
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(handle) };
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
    fn typed_actions_cover_live_preedit_and_candidates_without_json() {
        let handle = slime_create();
        assert!(!handle.is_null());
        // SAFETY: `handle` is live and exclusively accessed.
        let options = unsafe { slime_set_options(handle, true, false) };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };
        let mut capture = TypedCapture::default();
        for character in "raibuhenkannno".chars() {
            // SAFETY: Pointers are live for the synchronous callback.
            let status = unsafe {
                slime_process_actions(
                    handle,
                    EVENT_CHARACTER,
                    character.into(),
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            };
            assert_eq!(status, STATUS_OK);
        }
        assert_eq!(capture.last_preedit, "ライブ変換の");

        // SAFETY: Pointers are live for the synchronous callback.
        let status = unsafe {
            slime_process_actions(
                handle,
                EVENT_SPACE,
                0,
                (&raw mut capture).cast(),
                Some(collect_typed_action),
            )
        };
        assert_eq!(status, STATUS_OK);
        assert!(capture.candidate_count > 0);

        // SAFETY: `handle` is live and has not previously been released.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn typo_annotation_crosses_json_and_typed_action_boundaries() {
        let json_handle = slime_create();
        for character in "nihpn".chars() {
            // SAFETY: The handle is live and used serially.
            let buffer = unsafe { slime_process(json_handle, EVENT_CHARACTER, character.into()) };
            // SAFETY: The returned buffer is released exactly once.
            unsafe { slime_buffer_destroy(buffer) };
        }
        // SAFETY: The handle is live and used serially.
        let buffer = unsafe { slime_process(json_handle, EVENT_SPACE, 0) };
        // SAFETY: The buffer remains live while copied.
        let json = unsafe { copy_buffer(&buffer) };
        assert!(json.contains("日本　（にほんに訂正）"), "{json}");
        assert!(
            json.contains("{\"value\":\"日本\",\"annotation\":3,\"detail\":\"にほん\"}"),
            "{json}"
        );
        // SAFETY: Both resources are live and released exactly once.
        unsafe {
            slime_buffer_destroy(buffer);
            slime_destroy(json_handle);
        }

        let typed_handle = slime_create();
        let mut capture = TypedCapture::default();
        for character in "nihpn".chars() {
            // SAFETY: Pointers remain live for the synchronous callback.
            let status = unsafe {
                slime_process_actions(
                    typed_handle,
                    EVENT_CHARACTER,
                    character.into(),
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            };
            assert_eq!(status, STATUS_OK);
        }
        // SAFETY: Pointers remain live for the synchronous callback.
        let status = unsafe {
            slime_process_actions(
                typed_handle,
                EVENT_SPACE,
                0,
                (&raw mut capture).cast(),
                Some(collect_typed_action),
            )
        };
        assert_eq!(status, STATUS_OK);
        assert!(
            capture
                .candidates
                .iter()
                .any(|candidate| candidate == "日本　（にほんに訂正）")
        );
        let corrected_index = capture
            .candidates
            .iter()
            .position(|candidate| candidate == "日本　（にほんに訂正）")
            .expect("typed candidates should contain the correction label");
        // SAFETY: Pointers remain live for the synchronous callback.
        let status = unsafe {
            slime_process_actions(
                typed_handle,
                EVENT_SELECT_CANDIDATE,
                u32::try_from(corrected_index).unwrap(),
                (&raw mut capture).cast(),
                Some(collect_typed_action),
            )
        };
        assert_eq!(status, STATUS_OK);
        assert_eq!(capture.last_preedit, "日本");
        assert_eq!(capture.selected, corrected_index);
        assert_eq!(
            capture.candidates[corrected_index],
            "日本　（にほんに訂正）"
        );
        // SAFETY: Pointers remain live for the synchronous callback.
        let status = unsafe {
            slime_process_actions(
                typed_handle,
                EVENT_ACCEPT_CANDIDATE,
                0,
                (&raw mut capture).cast(),
                Some(collect_typed_action),
            )
        };
        assert_eq!(status, STATUS_OK);
        assert_eq!(capture.last_commit, "日本");
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(typed_handle) };
    }

    #[test]
    fn correction_metadata_crosses_v2_typed_actions() {
        let handle = slime_create();
        let mut capture = TypedCaptureV2::default();
        for character in "nihpn".chars() {
            // SAFETY: Pointers remain live for the synchronous callback.
            assert_eq!(
                unsafe {
                    slime_process_actions_v2(
                        handle,
                        EVENT_CHARACTER,
                        character.into(),
                        (&raw mut capture).cast(),
                        Some(collect_typed_action_v2),
                    )
                },
                STATUS_OK
            );
        }
        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions_v2(
                    handle,
                    EVENT_SPACE,
                    0,
                    (&raw mut capture).cast(),
                    Some(collect_typed_action_v2),
                )
            },
            STATUS_OK
        );
        let index = capture
            .values
            .iter()
            .position(|value| value == "日本")
            .expect("v2 correction value");
        assert_eq!(capture.displays[index], "日本　（にほんに訂正）");
        assert_eq!(capture.annotations[index], CANDIDATE_ANNOTATION_CORRECTION);
        assert_eq!(capture.details[index].as_deref(), Some("にほん"));
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn numeric_candidate_annotation_crosses_v2_typed_actions() {
        let handle = slime_create();
        let mut capture = TypedCaptureV2::default();
        for character in "senkyuuhyakukyuujuuichi".chars() {
            // SAFETY: Pointers remain live for the synchronous callback.
            assert_eq!(
                unsafe {
                    slime_process_actions_v2(
                        handle,
                        EVENT_CHARACTER,
                        character.into(),
                        (&raw mut capture).cast(),
                        Some(collect_typed_action_v2),
                    )
                },
                STATUS_OK
            );
        }
        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions_v2(
                    handle,
                    EVENT_SPACE,
                    0,
                    (&raw mut capture).cast(),
                    Some(collect_typed_action_v2),
                )
            },
            STATUS_OK
        );
        let index = capture
            .values
            .iter()
            .position(|value| value == "1991")
            .expect("numeric candidate");
        assert_eq!(capture.annotations[index], CANDIDATE_ANNOTATION_NUMBER);
        assert_eq!(capture.details[index], None);
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn expanded_recall_candidate_crosses_typed_actions_and_commits_by_index() {
        let handle = slime_create();
        let mut capture = TypedCapture::default();
        for character in "asairi".chars() {
            // SAFETY: Pointers remain live for the synchronous callback.
            assert_eq!(
                unsafe {
                    slime_process_actions(
                        handle,
                        EVENT_CHARACTER,
                        character.into(),
                        (&raw mut capture).cast(),
                        Some(collect_typed_action),
                    )
                },
                STATUS_OK
            );
        }
        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    EVENT_SPACE,
                    0,
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
        let initial_count = capture.candidate_count;
        assert!(
            !capture
                .candidates
                .iter()
                .any(|candidate| candidate == "浅煎り")
        );

        for _ in 0..initial_count {
            // SAFETY: Pointers remain live for the synchronous callback.
            assert_eq!(
                unsafe {
                    slime_process_actions(
                        handle,
                        EVENT_NEXT_CANDIDATE,
                        0,
                        (&raw mut capture).cast(),
                        Some(collect_typed_action),
                    )
                },
                STATUS_OK
            );
        }
        let expanded_index = capture
            .candidates
            .iter()
            .position(|candidate| candidate == "浅煎り")
            .expect("typed candidates should expose expanded recall");
        assert!(capture.candidate_count > initial_count);

        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    EVENT_SELECT_CANDIDATE,
                    u32::try_from(expanded_index).unwrap(),
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
        assert_eq!(capture.selected, expanded_index);
        assert_eq!(capture.last_preedit, "浅煎り");
        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    EVENT_ACCEPT_CANDIDATE,
                    0,
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
        assert_eq!(capture.last_commit, "浅煎り");
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn long_expanded_recall_crosses_typed_actions_and_commits_by_index() {
        let handle = slime_create();
        let mut capture = TypedCapture::default();
        for character in "watashihanihonjin".chars() {
            // SAFETY: Pointers remain live for the synchronous callback.
            assert_eq!(
                unsafe {
                    slime_process_actions(
                        handle,
                        EVENT_CHARACTER,
                        character.into(),
                        (&raw mut capture).cast(),
                        Some(collect_typed_action),
                    )
                },
                STATUS_OK
            );
        }
        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    EVENT_SPACE,
                    0,
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
        let initial_count = capture.candidate_count;

        for _ in 0..initial_count {
            // SAFETY: Pointers remain live for the synchronous callback.
            assert_eq!(
                unsafe {
                    slime_process_actions(
                        handle,
                        EVENT_NEXT_CANDIDATE,
                        0,
                        (&raw mut capture).cast(),
                        Some(collect_typed_action),
                    )
                },
                STATUS_OK
            );
        }
        assert!(capture.candidate_count > initial_count);
        let expanded = capture.candidates[initial_count].clone();

        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    EVENT_SELECT_CANDIDATE,
                    u32::try_from(initial_count).unwrap(),
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
        assert_eq!(capture.last_preedit, expanded);
        // SAFETY: Pointers remain live for the synchronous callback.
        assert_eq!(
            unsafe {
                slime_process_actions(
                    handle,
                    EVENT_ACCEPT_CANDIDATE,
                    0,
                    (&raw mut capture).cast(),
                    Some(collect_typed_action),
                )
            },
            STATUS_OK
        );
        assert_eq!(capture.last_commit, expanded);
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn short_left_context_learning_crosses_typed_actions_and_persists() {
        let directory =
            std::env::temp_dir().join(format!("slime-ffi-short-context-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.to_string_lossy();
        // SAFETY: `path` remains readable for the duration of the creation call.
        let handle = unsafe { slime_create_with_data_dir(path.as_ptr(), path.len()) };
        assert!(!handle.is_null());
        // SAFETY: `handle` is live and exclusively accessed in this test.
        let options = unsafe {
            slime_set_options_v5(
                handle,
                false,
                true,
                true,
                0,
                false,
                slime_core::ALL_DATE_FORMATS,
            )
        };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };

        let mut capture = TypedCapture::default();
        for _ in 0..2 {
            convert_and_accept_typed(handle, "heya", "部屋", &mut capture);
            convert_and_accept_typed(handle, "shoumei", "照明", &mut capture);
            convert_and_accept_typed(handle, "hon'nin", "本人", &mut capture);
            convert_and_accept_typed(handle, "shoumei", "証明", &mut capture);
        }
        // SAFETY: The handle is live and released once before reloading its data.
        unsafe { slime_destroy(handle) };

        // SAFETY: `path` remains readable for the duration of the creation call.
        let reloaded = unsafe { slime_create_with_data_dir(path.as_ptr(), path.len()) };
        assert!(!reloaded.is_null());
        // SAFETY: `reloaded` is live and exclusively accessed in this test.
        let options = unsafe {
            slime_set_options_v5(
                reloaded,
                false,
                true,
                true,
                0,
                false,
                slime_core::ALL_DATE_FORMATS,
            )
        };
        // SAFETY: `options` is the original live buffer.
        unsafe { slime_buffer_destroy(options) };

        convert_and_accept_typed(reloaded, "heya", "部屋", &mut capture);
        type_ascii(reloaded, "shoumei", &mut capture);
        process_typed_event(reloaded, EVENT_SPACE, 0, &mut capture);
        assert_eq!(capture.candidates.first().map(String::as_str), Some("照明"));
        assert_eq!(capture.last_preedit, "照明");

        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(reloaded) };
        fs::remove_dir_all(directory).unwrap();
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
    fn signed_data_directory_constructor_requires_valid_trusted_keys() {
        let directory = "/tmp/slime-signed-data-dir-constructor-fixture";
        let keys =
            "fixture-2026-a\td75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a\n";
        // SAFETY: Both UTF-8 byte slices remain readable for this call.
        let handle = unsafe {
            slime_create_with_signed_data_dir(
                directory.as_ptr(),
                directory.len(),
                keys.as_ptr(),
                keys.len(),
            )
        };
        assert!(!handle.is_null());
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(handle) };

        // SAFETY: Empty key input is a valid byte slice but not a valid policy.
        let invalid = unsafe {
            slime_create_with_signed_data_dir(
                directory.as_ptr(),
                directory.len(),
                std::ptr::null(),
                0,
            )
        };
        assert!(invalid.is_null());

        let floors = "sample-general\t2026.08.1\n";
        // SAFETY: Every UTF-8 byte slice remains readable for this call.
        let rollback_protected = unsafe {
            slime_create_with_signed_data_dir_and_version_floors(
                directory.as_ptr(),
                directory.len(),
                keys.as_ptr(),
                keys.len(),
                floors.as_ptr(),
                floors.len(),
            )
        };
        assert!(!rollback_protected.is_null());
        // SAFETY: The handle is live and released once.
        unsafe { slime_destroy(rollback_protected) };

        let invalid_floor = "sample-general\t2026.08\n";
        // SAFETY: Every UTF-8 byte slice remains readable for this call.
        let invalid = unsafe {
            slime_create_with_signed_data_dir_and_version_floors(
                directory.as_ptr(),
                directory.len(),
                keys.as_ptr(),
                keys.len(),
                invalid_floor.as_ptr(),
                invalid_floor.len(),
            )
        };
        assert!(invalid.is_null());
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
    fn context_reset_reports_live_and_null_handles() {
        let handle = slime_create();
        // SAFETY: `handle` is live and exclusively accessed.
        assert_eq!(unsafe { slime_reset_context(handle) }, STATUS_OK);
        // SAFETY: A null handle is explicitly accepted and reported as an error.
        assert_eq!(
            unsafe { slime_reset_context(std::ptr::null_mut()) },
            STATUS_NULL_HANDLE
        );
        // SAFETY: `handle` is live and has not previously been released.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn external_left_context_reports_invalid_inputs_without_panicking() {
        let handle = slime_create();
        let context = "直前の文章";
        // SAFETY: The UTF-8 bytes and handle remain live for the call.
        assert_eq!(
            unsafe { slime_set_external_left_context(handle, context.as_ptr(), context.len(),) },
            STATUS_OK
        );
        let invalid = [0xff];
        // SAFETY: The byte is readable but deliberately invalid UTF-8.
        assert_eq!(
            unsafe { slime_set_external_left_context(handle, invalid.as_ptr(), invalid.len()) },
            STATUS_INVALID_UTF8
        );
        // SAFETY: A null handle is explicitly accepted and reported.
        assert_eq!(
            unsafe {
                slime_set_external_left_context(
                    std::ptr::null_mut(),
                    context.as_ptr(),
                    context.len(),
                )
            },
            STATUS_NULL_HANDLE
        );
        // SAFETY: The handle is live and has not previously been released.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn bidirectional_context_reports_invalid_inputs_without_panicking() {
        let handle = slime_create();
        let left = "直前の文章";
        let right = "後続の文章";
        // SAFETY: Both UTF-8 byte ranges and the handle remain live for the call.
        assert_eq!(
            unsafe {
                slime_set_external_context(
                    handle,
                    left.as_ptr(),
                    left.len(),
                    right.as_ptr(),
                    right.len(),
                )
            },
            STATUS_OK
        );
        let invalid = [0xff];
        // SAFETY: The right byte is readable but deliberately invalid UTF-8.
        assert_eq!(
            unsafe {
                slime_set_external_context(
                    handle,
                    left.as_ptr(),
                    left.len(),
                    invalid.as_ptr(),
                    invalid.len(),
                )
            },
            STATUS_INVALID_UTF8
        );
        // SAFETY: A null handle is explicitly accepted and reported.
        assert_eq!(
            unsafe {
                slime_set_external_context(
                    std::ptr::null_mut(),
                    left.as_ptr(),
                    left.len(),
                    right.as_ptr(),
                    right.len(),
                )
            },
            STATUS_NULL_HANDLE
        );
        // SAFETY: The handle is live and has not previously been released.
        unsafe { slime_destroy(handle) };
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
# slime-dictionary-pack-v3
# id: sample-general
# name: 一般語彙サンプル
# version: 2026.08.1
# license: Example-Test-Only
# minimum-slime-version: 0.1.0
# published-at: 2026-08-08
# provenance: fixture/generated/sample-general
# payload-sha256: dba7dcf657c74cd788ee904f95b5d2dd54d6fd16925e2ec88c96a13d19e4a0b6
# entries
てすとようご\t試験用語
# context-rules
文章\tかんじ\t漢字\t0
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
        assert!(json.contains("\"id\":\"sample-general\""), "{json}");
        assert!(json.contains("\"formatVersion\":3"), "{json}");
        assert!(
            json.contains("\"provenance\":\"fixture/generated/sample-general\""),
            "{json}"
        );
        assert!(
            json.contains(
                "\"payloadSHA256\":\"dba7dcf657c74cd788ee904f95b5d2dd54d6fd16925e2ec88c96a13d19e4a0b6\""
            ),
            "{json}"
        );
        assert!(json.contains("\"entryCount\":1"), "{json}");
        assert!(json.contains("\"contextRuleCount\":1"), "{json}");
        assert!(json.contains("\"packSHA256\":"), "{json}");
        // SAFETY: `catalog` has not previously been released.
        unsafe { slime_buffer_destroy(catalog) };

        let id = b"sample-general";
        // SAFETY: `handle` and `id` are live and readable for this call.
        let words = unsafe { slime_installed_dictionary_pack_words(handle, id.as_ptr(), id.len()) };
        // SAFETY: `words` is live until the destroy call below.
        let json = unsafe { copy_buffer(&words) };
        assert!(json.contains("試験用語"), "{json}");
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
